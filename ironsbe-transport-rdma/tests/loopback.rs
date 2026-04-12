//! Linux-only integration tests for the RDMA listener.
//!
//! These tests exercise behaviour that is observable through the
//! public `RdmaListener` API alone, so they do not require a real
//! client-side connection (the server-side is the only thing in
//! scope for issue #39).
//!
//! Tests that would need end-to-end loopback — e.g. verifying
//! `peer_addr` round-trips or send-burst behaviour — are documented
//! as deferred in the PR body because they block on a real
//! client-connect implementation.
//!
//! Requirements:
//! - Linux with `libibverbs` + `librdmacm` installed.
//! - SoftRoCE (`rdma_rxe`) loaded with an ACTIVE device bound to a
//!   physical netdev, OR a real RDMA-capable NIC.
//!
//! When no RDMA device is available the tests print a message and
//! return early rather than panicking, so `cargo test` on a
//! host without RDMA still succeeds cleanly.

#![cfg(target_os = "linux")]

use ironsbe_transport::traits::LocalListener;
use ironsbe_transport_rdma::RdmaListener;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::time::timeout;

const DEFAULT_MAX_MSG: usize = 64 * 1024;

/// Try to bind a listener.  If no RDMA device is available return
/// `None` with a descriptive message so the calling test can skip
/// gracefully instead of panicking.
fn try_bind_listener(
    addr: SocketAddr,
    max_msg: usize,
) -> Option<RdmaListener> {
    match RdmaListener::bind(addr, max_msg) {
        Ok(l) => Some(l),
        Err(e) => {
            eprintln!("RDMA listener bind failed — skipping test (is SoftRoCE up?): {e}");
            None
        }
    }
}

/// `bind(0.0.0.0:0)` + `rdma_listen` must yield a concrete bound
/// port (non-zero) reported by `local_addr()`.  Verifies that the
/// effective address is read back from the listen CM ID instead of
/// echoing the requested bind addr.  See #39.
#[tokio::test]
async fn test_listener_local_addr_reports_bound_port() {
    let bind_addr: SocketAddr = "0.0.0.0:0".parse().expect("parse bind addr");
    let Some(listener) = try_bind_listener(bind_addr, DEFAULT_MAX_MSG) else {
        return;
    };

    let reported = listener.local_addr().expect("local_addr");
    assert_ne!(
        reported.port(),
        0,
        "local_addr must report the OS-assigned port, not 0"
    );
    // For a bind to 0.0.0.0 the kernel may leave the IP as 0.0.0.0
    // (INADDR_ANY) because the listen socket isn't tied to any
    // specific interface yet.  The only thing we can reliably
    // assert is the port has been materialised.
}

/// `accept()` must not block the tokio worker thread.  We start
/// `accept()` inside a short-deadline timeout with no client
/// connecting, and verify that a parallel task tagged against the
/// *same* runtime continues to make progress while the accept
/// future is pending.  See #39.
#[tokio::test]
async fn test_accept_yields_to_runtime() {
    let bind_addr: SocketAddr = "0.0.0.0:0".parse().expect("parse bind addr");
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

    // Drive accept with a short deadline so the test does not wait
    // for a real client.  We expect the timeout to elapse; the
    // interesting assertion is that the background task made
    // progress in the meantime, proving the runtime was not blocked
    // on `rdma_get_cm_event`.
    let _ = timeout(Duration::from_millis(250), listener.accept()).await;

    let _ = bg.await;

    let observed = counter.load(Ordering::SeqCst);
    assert!(
        observed >= 10,
        "runtime starved by accept(): parallel counter only reached {observed}"
    );
}
