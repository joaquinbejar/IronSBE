//! End-to-end transport round-trip benchmark.
//!
//! Measures the per-message round-trip latency of a 64-byte SBE payload
//! over a persistent connection, comparing the multi-threaded
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
//!
//! Output is a markdown table ready to paste into the PR description and
//! `docs/transport-backends.md`.  We deliberately do **not** use criterion's
//! `iter` harness because:
//!
//! 1. criterion's default reporter prints `[lower_CI mean upper_CI]` of the
//!    **mean**, not p50/p99/p99.9 — which is exactly what we need for a
//!    transport latency benchmark.
//! 2. We want all timing to happen on the runtime thread that owns the
//!    connection, with zero cross-thread plumbing in the hot loop.  Each
//!    backend's worker thread runs its own measurement loop with
//!    `Instant::now()` and feeds an `hdrhistogram::Histogram`, then
//!    returns the histogram to the main thread for reporting.
//!
//! This bench has `harness = false` in `Cargo.toml`, so it is just a
//! plain binary with `fn main`.

use hdrhistogram::Histogram;
use ironsbe_transport::tcp::{TcpClientConfig, TcpServerConfig, TokioTcpTransport};
use ironsbe_transport::traits::Transport;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const PAYLOAD_LEN: usize = 64;
const WARMUP_ITERS: usize = 10_000;
const MEASURE_ITERS: usize = 100_000;

/// Per-backend measurement result handed back to the main thread.
struct BenchResult {
    name: &'static str,
    histogram: Histogram<u64>,
}

/// Builds a deterministic 64-byte payload so the bench is reproducible.
fn make_payload() -> Vec<u8> {
    (0..PAYLOAD_LEN).map(|i| (i & 0xff) as u8).collect()
}

/// Renders a single result as one row of the markdown summary table.
///
/// Histogram values are stored in nanoseconds, so we format them as
/// `µs` with three decimals to keep the table readable for typical
/// loopback latencies.
fn render_row(result: &BenchResult) {
    let h = &result.histogram;
    let p50 = h.value_at_quantile(0.50) as f64 / 1_000.0;
    let p99 = h.value_at_quantile(0.99) as f64 / 1_000.0;
    let p999 = h.value_at_quantile(0.999) as f64 / 1_000.0;
    let mean_ns = h.mean();
    let throughput = if mean_ns > 0.0 {
        1e9 / mean_ns
    } else {
        f64::INFINITY
    };
    println!(
        "| `{}` | {:.3} µs | {:.3} µs | {:.3} µs | {:>7.0} msg/s |",
        result.name, p50, p99, p999, throughput,
    );
}

// =====================================================================
// tcp-tokio path
// =====================================================================

fn run_tcp_tokio_bench() -> BenchResult {
    let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
    let (addr_tx, addr_rx) = mpsc::sync_channel::<SocketAddr>(1);
    let (result_tx, result_rx) = mpsc::sync_channel::<Histogram<u64>>(1);

    // Spin up a dedicated multi-thread tokio runtime in its own thread.
    // The runtime owns both the echo server and the bench client; the
    // bench timing happens entirely on this thread so the criterion-side
    // mpsc plumbing is **not** in the timed section.
    let _runtime_thread = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build tokio runtime");
        runtime.block_on(async move {
            // Echo server.
            let mut listener = TokioTcpTransport::bind_with(TcpServerConfig::new(bind_addr))
                .await
                .expect("tokio bind");
            let listen_addr = listener.local_addr().expect("local_addr");
            let _ = addr_tx.send(listen_addr);

            tokio::spawn(async move {
                let mut conn = listener.accept().await.expect("accept");
                while let Ok(Some(msg)) = conn.recv().await {
                    if conn.send(&msg).await.is_err() {
                        break;
                    }
                }
            });

            // Bench client.
            let mut client = TokioTcpTransport::connect_with(TcpClientConfig::new(listen_addr))
                .await
                .expect("tokio connect");
            let payload = make_payload();

            // Warmup.
            for _ in 0..WARMUP_ITERS {
                client.send(&payload).await.expect("warmup send");
                let _ = client.recv().await.expect("warmup recv").expect("frame");
            }

            // Measurement.  Histogram tracks 1 ns .. 1 s, 3 sig figs.
            let mut hist = Histogram::<u64>::new_with_bounds(1, 1_000_000_000, 3)
                .expect("histogram bounds are valid");
            for _ in 0..MEASURE_ITERS {
                let start = Instant::now();
                client.send(&payload).await.expect("measure send");
                let _ = client.recv().await.expect("measure recv").expect("frame");
                let elapsed = start.elapsed().as_nanos() as u64;
                hist.record(elapsed.max(1)).expect("record");
            }
            let _ = result_tx.send(hist);
        });
    });

    let _ = addr_rx.recv().expect("addr");
    let histogram = result_rx
        .recv_timeout(Duration::from_secs(120))
        .expect("tcp-tokio bench timed out");
    BenchResult {
        name: "tcp-tokio",
        histogram,
    }
}

// =====================================================================
// tcp-uring path (Linux only)
// =====================================================================

#[cfg(all(feature = "tcp-uring", target_os = "linux"))]
fn run_tcp_uring_bench() -> Option<BenchResult> {
    use ironsbe_transport::tcp_uring::{UringClientConfig, UringServerConfig, UringTcpTransport};
    use ironsbe_transport::traits::{LocalConnection, LocalListener, LocalTransport};

    let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
    let (addr_tx, addr_rx) = mpsc::sync_channel::<SocketAddr>(1);
    let (result_tx, result_rx) = mpsc::sync_channel::<Histogram<u64>>(1);

    let _runtime_thread = thread::spawn(move || {
        tokio_uring::start(async move {
            // Echo server.
            let mut listener = UringTcpTransport::bind_with(UringServerConfig::new(bind_addr))
                .await
                .expect("uring bind");
            let listen_addr = listener.local_addr().expect("local_addr");
            let _ = addr_tx.send(listen_addr);

            tokio::task::spawn_local(async move {
                let mut conn = listener.accept().await.expect("accept");
                while let Ok(Some(msg)) = conn.recv().await {
                    if conn.send(&msg).await.is_err() {
                        break;
                    }
                }
            });

            // Bench client.
            let mut client = UringTcpTransport::connect_with(UringClientConfig::new(listen_addr))
                .await
                .expect("uring connect");
            let payload = make_payload();

            // Warmup.
            for _ in 0..WARMUP_ITERS {
                client.send(&payload).await.expect("warmup send");
                let _ = client.recv().await.expect("warmup recv").expect("frame");
            }

            // Measurement, single threaded, zero cross-thread overhead
            // in the hot loop.
            let mut hist = Histogram::<u64>::new_with_bounds(1, 1_000_000_000, 3)
                .expect("histogram bounds are valid");
            for _ in 0..MEASURE_ITERS {
                let start = Instant::now();
                client.send(&payload).await.expect("measure send");
                let _ = client.recv().await.expect("measure recv").expect("frame");
                let elapsed = start.elapsed().as_nanos() as u64;
                hist.record(elapsed.max(1)).expect("record");
            }
            let _ = result_tx.send(hist);
        });
    });

    let _ = addr_rx.recv().expect("addr");
    let histogram = result_rx
        .recv_timeout(Duration::from_secs(120))
        .expect("tcp-uring bench timed out");
    Some(BenchResult {
        name: "tcp-uring",
        histogram,
    })
}

#[cfg(not(all(feature = "tcp-uring", target_os = "linux")))]
fn run_tcp_uring_bench() -> Option<BenchResult> {
    None
}

// =====================================================================
// main
// =====================================================================

fn main() {
    println!(
        "Running transport_round_trip ({}-byte payload, {} warmup + {} measured iterations per backend)",
        PAYLOAD_LEN, WARMUP_ITERS, MEASURE_ITERS,
    );
    println!();

    let mut results = vec![run_tcp_tokio_bench()];
    if let Some(uring) = run_tcp_uring_bench() {
        results.push(uring);
    } else {
        println!(
            "(tcp-uring backend not built — enable with --features tcp-uring on Linux >= 5.10)\n"
        );
    }

    println!("| Backend     |       p50 |       p99 |     p99.9 |    Throughput |");
    println!("|-------------|-----------|-----------|-----------|---------------|");
    for result in &results {
        render_row(result);
    }
}
