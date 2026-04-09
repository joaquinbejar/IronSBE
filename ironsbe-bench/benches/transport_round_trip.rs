//! End-to-end transport round-trip benchmark.
//!
//! Measures the time to send a small SBE message from the client to the
//! server and receive an echo back.  Compares the multi-threaded
//! `tcp-tokio` backend against the Linux-only `tcp-uring` backend (when
//! the `tcp-uring` feature is enabled).
//!
//! Run with:
//!
//! ```sh
//! # tcp-tokio only:
//! cargo bench -p ironsbe-bench --bench transport_round_trip
//!
//! # both backends (Linux >= 5.10):
//! cargo bench -p ironsbe-bench --bench transport_round_trip --features tcp-uring
//! ```

use criterion::{Criterion, criterion_group, criterion_main};
use ironsbe_transport::tcp::{TcpClientConfig, TcpServerConfig, TokioTcpTransport};
use ironsbe_transport::traits::{Connection, Transport};
use std::hint::black_box;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

const PAYLOAD_LEN: usize = 64;
const ITERATIONS_PER_BATCH: usize = 32;

/// Builds a deterministic payload of the given length so the benchmark
/// is reproducible across runs.
fn make_payload() -> Vec<u8> {
    (0..PAYLOAD_LEN).map(|i| (i & 0xff) as u8).collect()
}

// =====================================================================
// tcp-tokio path
// =====================================================================

struct TokioFixture {
    runtime: Arc<Runtime>,
    server_addr: SocketAddr,
    _server_thread: thread::JoinHandle<()>,
}

fn tokio_fixture() -> &'static TokioFixture {
    static FIXTURE: OnceLock<TokioFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("build tokio runtime"),
        );
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
        let (addr_tx, addr_rx) = oneshot::channel();
        let rt_clone = Arc::clone(&runtime);
        let server_thread = thread::spawn(move || {
            rt_clone.block_on(async move {
                let mut listener = TokioTcpTransport::bind_with(TcpServerConfig::new(bind_addr))
                    .await
                    .expect("bind");
                let listen_addr = listener.local_addr().expect("local_addr");
                let _ = addr_tx.send(listen_addr);
                loop {
                    let conn = listener.accept().await.expect("accept");
                    tokio::spawn(echo_loop_tokio(conn));
                }
            });
        });
        let server_addr = runtime.block_on(addr_rx).expect("addr");
        TokioFixture {
            runtime,
            server_addr,
            _server_thread: server_thread,
        }
    })
}

async fn echo_loop_tokio<C: Connection>(mut conn: C) {
    while let Ok(Some(msg)) = conn.recv().await {
        if conn.send(&msg).await.is_err() {
            break;
        }
    }
}

fn bench_tcp_tokio_round_trip(c: &mut Criterion) {
    let fixture = tokio_fixture();
    let runtime = Arc::clone(&fixture.runtime);
    let server_addr = fixture.server_addr;

    let mut group = c.benchmark_group("transport_round_trip/tcp_tokio");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(50);
    group.bench_function("payload_64B", |b| {
        // Connect once, reuse the connection across iterations to keep
        // the bench measuring round-trip latency, not connect cost.
        let payload = make_payload();
        let mut client = runtime.block_on(async {
            TokioTcpTransport::connect_with(TcpClientConfig::new(server_addr))
                .await
                .expect("connect")
        });
        b.iter(|| {
            runtime.block_on(async {
                for _ in 0..ITERATIONS_PER_BATCH {
                    client.send(&payload).await.expect("send");
                    let echo = client.recv().await.expect("recv").expect("frame");
                    black_box(echo);
                }
            });
        });
    });
    group.finish();
}

// =====================================================================
// tcp-uring path (Linux only)
// =====================================================================

#[cfg(all(feature = "tcp-uring", target_os = "linux"))]
mod uring {
    use super::*;
    use ironsbe_transport::tcp_uring::{UringClientConfig, UringServerConfig, UringTcpTransport};
    use ironsbe_transport::traits::{LocalConnection, LocalListener, LocalTransport};
    use std::sync::Mutex;
    use std::sync::mpsc as std_mpsc;

    pub(super) struct UringFixture {
        pub(super) server_addr: SocketAddr,
        pub(super) command_tx: std_mpsc::SyncSender<UringCommand>,
        // Receivers from std::mpsc are `!Sync`, so wrap in a Mutex to
        // satisfy the `OnceLock` static-sharing bound.  The bench thread
        // is the only consumer so contention is zero in practice.
        pub(super) reply_rx: Mutex<std_mpsc::Receiver<UringReply>>,
        _runtime_thread: thread::JoinHandle<()>,
    }

    pub(super) enum UringCommand {
        Send(Vec<u8>),
        Shutdown,
    }

    pub(super) enum UringReply {
        Echo(Vec<u8>),
    }

    pub(super) fn uring_fixture() -> &'static UringFixture {
        static FIXTURE: OnceLock<UringFixture> = OnceLock::new();
        FIXTURE.get_or_init(|| {
            let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
            let (addr_tx, addr_rx) = std_mpsc::sync_channel::<SocketAddr>(1);
            let (command_tx, command_rx) = std_mpsc::sync_channel::<UringCommand>(1024);
            let (reply_tx, reply_rx) = std_mpsc::sync_channel::<UringReply>(1024);

            let runtime_thread = thread::spawn(move || {
                tokio_uring::start(async move {
                    // Bind the server side of the loop.
                    let mut listener =
                        UringTcpTransport::bind_with(UringServerConfig::new(bind_addr))
                            .await
                            .expect("uring bind");
                    let listen_addr = listener.local_addr().expect("local_addr");
                    let _ = addr_tx.send(listen_addr);

                    // Spawn an echo task on the same local set so the
                    // bench client (also on this thread) has a peer.
                    tokio::task::spawn_local(async move {
                        let mut conn = listener.accept().await.expect("accept");
                        while let Ok(Some(msg)) = conn.recv().await {
                            if conn.send(&msg).await.is_err() {
                                break;
                            }
                        }
                    });

                    // Connect the bench client and run the
                    // command-driven send/recv loop until shutdown.
                    let mut client =
                        UringTcpTransport::connect_with(UringClientConfig::new(listen_addr))
                            .await
                            .expect("uring connect");
                    while let Ok(cmd) = command_rx.recv() {
                        match cmd {
                            UringCommand::Send(payload) => {
                                client.send(&payload).await.expect("uring send");
                                let echo = client.recv().await.expect("uring recv").expect("frame");
                                let _ = reply_tx.send(UringReply::Echo(echo.to_vec()));
                            }
                            UringCommand::Shutdown => break,
                        }
                    }
                });
            });
            // The runtime thread sends the bound addr exactly once via
            // `addr_tx`; we receive it here on the criterion thread.
            let server_addr = addr_rx.recv().expect("addr");
            UringFixture {
                server_addr,
                command_tx,
                reply_rx: Mutex::new(reply_rx),
                _runtime_thread: runtime_thread,
            }
        })
    }
}

#[cfg(all(feature = "tcp-uring", target_os = "linux"))]
fn bench_tcp_uring_round_trip(c: &mut Criterion) {
    let fixture = uring::uring_fixture();
    let _ = fixture.server_addr; // logged for debug

    let mut group = c.benchmark_group("transport_round_trip/tcp_uring");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(50);
    group.bench_function("payload_64B", |b| {
        let payload = make_payload();
        b.iter(|| {
            for _ in 0..ITERATIONS_PER_BATCH {
                fixture
                    .command_tx
                    .send(uring::UringCommand::Send(payload.clone()))
                    .expect("command send");
                let reply = fixture
                    .reply_rx
                    .lock()
                    .expect("reply mutex poisoned")
                    .recv()
                    .expect("reply recv");
                let uring::UringReply::Echo(echo) = reply;
                black_box(echo);
            }
        });
    });
    group.finish();
}

#[cfg(not(all(feature = "tcp-uring", target_os = "linux")))]
fn bench_tcp_uring_round_trip(_c: &mut Criterion) {
    // Stub when the feature is disabled.  Criterion will simply skip the
    // group; the bench binary still compiles everywhere.
}

criterion_group!(
    benches,
    bench_tcp_tokio_round_trip,
    bench_tcp_uring_round_trip
);
criterion_main!(benches);
