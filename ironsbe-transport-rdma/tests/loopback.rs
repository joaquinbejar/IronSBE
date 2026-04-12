//! Linux-only integration tests for the RDMA transport backend.
//!
//! The server runs in a dedicated OS thread with its own tokio
//! runtime because both ends need separate epoll reactors to handle
//! RDMA CM event channels on SoftRoCE.
//!
//! Every server thread wraps its work in `tokio::time::timeout` so
//! it cannot hang indefinitely if the client fails.  A `JoinGuard`
//! ensures the server thread is joined even if the client panics
//! during unwind.  See #50.
//!
//! When no RDMA device is available the tests print a message and
//! return early, so `cargo test` on a host without RDMA succeeds.

#![cfg(target_os = "linux")]

use ironsbe_transport::traits::{LocalConnection, LocalListener};
use ironsbe_transport_rdma::{RdmaConnection, RdmaListener};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::timeout;

const DEFAULT_MAX_MSG: usize = 64 * 1024;
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// RAII guard that joins a thread on drop — even during unwind.
/// Prevents detached server threads from leaking past the test scope.
struct JoinGuard(Option<std::thread::JoinHandle<()>>);

impl JoinGuard {
    fn new(handle: std::thread::JoinHandle<()>) -> Self {
        Self(Some(handle))
    }

    /// Consumes the guard, joins the thread, and propagates panics.
    fn join(mut self) {
        if let Some(h) = self.0.take() {
            h.join().expect("server thread panicked");
        }
    }
}

impl Drop for JoinGuard {
    fn drop(&mut self) {
        if let Some(h) = self.0.take() {
            let _ = h.join();
        }
    }
}

fn try_bind_listener(addr: SocketAddr, max_msg: usize) -> Option<RdmaListener> {
    match RdmaListener::bind(addr, max_msg) {
        Ok(l) => Some(l),
        Err(e) => {
            eprintln!("RDMA bind failed — skip (SoftRoCE up?): {e}");
            None
        }
    }
}

/// Discovers a non-loopback IPv4 address suitable for SoftRoCE.
///
/// Respects `IRONSBE_LOOPBACK_IPV4` env var as an explicit override.
/// Otherwise enumerates interfaces and prefers `eth*`/`en*`/`ib*`
/// names over Docker/bridge/veth virtual interfaces.
fn first_non_loopback_ipv4() -> Option<std::net::Ipv4Addr> {
    use std::ffi::CStr;
    use std::net::Ipv4Addr;

    if let Ok(val) = std::env::var("IRONSBE_LOOPBACK_IPV4")
        && let Ok(ip) = val.parse::<Ipv4Addr>()
        && !ip.is_loopback()
        && !ip.is_unspecified()
    {
        return Some(ip);
    }

    fn is_preferred(name: &str) -> bool {
        name.starts_with("eth") || name.starts_with("en") || name.starts_with("ib")
    }
    fn is_virtual(name: &str) -> bool {
        name.starts_with("docker")
            || name.starts_with("br-")
            || name.starts_with("virbr")
            || name.starts_with("veth")
            || name.starts_with("cni")
    }

    unsafe {
        let mut ifaddrs: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifaddrs) != 0 {
            return None;
        }
        let mut candidates: Vec<(String, Ipv4Addr)> = Vec::new();
        let mut cursor = ifaddrs;
        while !cursor.is_null() {
            let ifa = &*cursor;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family == libc::AF_INET as u16 {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if !ip.is_loopback() && !ip.is_unspecified() {
                    let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
                    candidates.push((name, ip));
                }
            }
            cursor = ifa.ifa_next;
        }
        libc::freeifaddrs(ifaddrs);

        candidates
            .iter()
            .find(|(n, _)| is_preferred(n))
            .or_else(|| candidates.iter().find(|(n, _)| !is_virtual(n)))
            .or(candidates.first())
            .map(|(_, ip)| *ip)
    }
}

/// Spawns a server on a dedicated OS thread with its own tokio
/// runtime.  The server binds, sends the port back, then runs
/// `server_body` inside `timeout(TEST_TIMEOUT, ...)`.
///
/// The returned [`JoinGuard`] joins the thread on drop (even during
/// unwind) so the server thread is never left detached.
///
/// Returns `None` (joining the thread on failure) if no RDMA device
/// or the port handshake fails.
fn spawn_server<F, Fut>(server_body: F) -> Option<(SocketAddr, JoinGuard)>
where
    F: FnOnce(RdmaListener) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    let host_ip = first_non_loopback_ipv4()?;
    let (port_tx, port_rx) = std::sync::mpsc::sync_channel::<Option<u16>>(1);

    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("server rt");
        rt.block_on(async {
            let bind: SocketAddr = "0.0.0.0:0".parse().expect("parse");
            let listener = match RdmaListener::bind(bind, DEFAULT_MAX_MSG) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("server bind: {e}");
                    let _ = port_tx.send(None);
                    return;
                }
            };
            let _ = port_tx.send(Some(listener.local_addr().expect("la").port()));

            if timeout(TEST_TIMEOUT, server_body(listener)).await.is_err() {
                eprintln!("server timed out (no client within {TEST_TIMEOUT:?})");
            }
        });
    });

    let guard = JoinGuard::new(handle);

    let port = match port_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Some(p)) => p,
        _ => {
            // Guard drops here → joins the thread before returning.
            drop(guard);
            return None;
        }
    };

    let addr = SocketAddr::new(std::net::IpAddr::V4(host_ip), port);
    Some((addr, guard))
}

// ── listener-only tests (from #39) ─────────────────────────────────

#[tokio::test]
async fn test_listener_local_addr_reports_bound_port() {
    let bind_addr: SocketAddr = "0.0.0.0:0".parse().expect("parse");
    let Some(listener) = try_bind_listener(bind_addr, DEFAULT_MAX_MSG) else {
        return;
    };
    let reported = listener.local_addr().expect("local_addr");
    assert_ne!(reported.port(), 0);
}

#[tokio::test]
async fn test_accept_yields_to_runtime() {
    let bind_addr: SocketAddr = "0.0.0.0:0".parse().expect("parse");
    let Some(mut listener) = try_bind_listener(bind_addr, DEFAULT_MAX_MSG) else {
        return;
    };
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_bg = Arc::clone(&counter);
    let bg = tokio::spawn(async move {
        for _ in 0..50 {
            counter_bg.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let _ = timeout(Duration::from_millis(250), listener.accept()).await;
    let _ = bg.await;
    assert!(counter.load(Ordering::SeqCst) >= 10);
}

// ── loopback tests (from #48, server timeout from #50) ─────────────

/// Full round-trip: client sends "hello", server echoes "world".
#[tokio::test(flavor = "current_thread")]
async fn test_loopback_round_trip() {
    let Some((addr, server)) = spawn_server(|mut listener| async move {
        let mut conn = listener.accept().await.expect("accept");
        let msg = conn.recv().await.expect("recv");
        assert_eq!(&msg.expect("some")[..], b"hello");
        conn.send(b"world").await.expect("send");
    }) else {
        return;
    };

    let client_result = timeout(TEST_TIMEOUT, async {
        let mut client = RdmaConnection::connect(addr, DEFAULT_MAX_MSG)
            .await
            .expect("connect");
        client.send(b"hello").await.expect("send");
        let reply = client.recv().await.expect("recv");
        assert_eq!(&reply.expect("some")[..], b"world");
    })
    .await;

    // JoinGuard ensures the thread is joined even if the assert
    // below panics; explicit join() propagates server panics.
    server.join();
    assert!(client_result.is_ok(), "test_loopback_round_trip timed out");
}

/// > CQ_CAPACITY back-to-back sends.
///
/// `#[ignore]`-gated: single `send_buf` design from #38 reuses the
/// same memory for every SEND WR — needs multi-buffer refactor.
#[tokio::test(flavor = "current_thread")]
#[ignore = "single send_buf design from #38 — needs multi-buffer refactor for burst"]
async fn test_send_burst_past_cq_capacity() {
    const BURST: usize = 48;

    let Some((addr, server)) = spawn_server(|mut listener| async move {
        let mut conn = listener.accept().await.expect("accept");
        for i in 0..BURST {
            let msg = conn
                .recv()
                .await
                .unwrap_or_else(|e| panic!("recv {i}: {e}"));
            assert!(msg.is_some(), "None on recv {i}");
        }
    }) else {
        return;
    };

    let client_result = timeout(TEST_TIMEOUT, async {
        let mut client = RdmaConnection::connect(addr, DEFAULT_MAX_MSG)
            .await
            .expect("connect");
        for i in 0..BURST {
            let payload = format!("burst-{i:04}");
            client
                .send(payload.as_bytes())
                .await
                .unwrap_or_else(|e| panic!("send {i}: {e}"));
        }
    })
    .await;

    server.join();
    assert!(
        client_result.is_ok(),
        "test_send_burst_past_cq_capacity timed out"
    );
}

/// `peer_addr()` on the server side must not be unspecified.
#[tokio::test(flavor = "current_thread")]
async fn test_accepted_connection_peer_addr() {
    let Some((addr, server)) = spawn_server(|mut listener| async move {
        let conn = listener.accept().await.expect("accept");
        let peer = conn.peer_addr().expect("peer_addr");
        assert_ne!(peer.port(), 0);
        assert!(!peer.ip().is_unspecified(), "peer IP unspecified: {peer}");
        eprintln!("[peer_addr] {peer}");
    }) else {
        return;
    };

    let client_result = timeout(TEST_TIMEOUT, async {
        let _client = RdmaConnection::connect(addr, DEFAULT_MAX_MSG)
            .await
            .expect("connect");
        tokio::time::sleep(Duration::from_millis(200)).await;
    })
    .await;

    server.join();
    assert!(
        client_result.is_ok(),
        "test_accepted_connection_peer_addr timed out"
    );
}
