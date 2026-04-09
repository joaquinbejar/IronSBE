//! Regression test for #31.
//!
//! Validates that the multi-threaded `Server` actually frees its
//! `SessionManager` slots when peers disconnect.  Without the fix, the
//! per-session task only emits `ServerEvent::SessionClosed` and never
//! tells the run loop to call `SessionManager::close_session(id)`,
//! so `sessions.count()` grows monotonically and `max_connections`
//! eventually rejects every new connection.

#![cfg(feature = "tcp-tokio")]

use ironsbe_core::header::MessageHeader;
use ironsbe_server::{MessageHandler, Responder, ServerBuilder, ServerEvent, ServerHandle};
use ironsbe_transport::tcp::TcpServerConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Trivial echo handler — the test only cares about the connection
/// lifecycle, not the message body.
struct NoopHandler;

impl MessageHandler for NoopHandler {
    fn on_message(
        &self,
        _session_id: u64,
        _header: &MessageHeader,
        _buffer: &[u8],
        _responder: &dyn Responder,
    ) {
    }
}

const MAX_CONNECTIONS: usize = 4;
const TOTAL_CONNECTS: usize = 10;
const PER_CYCLE_TIMEOUT: Duration = Duration::from_secs(5);

/// Polls `handle.poll_events()` until a `SessionClosed` for the given
/// `session_id` is observed, or the deadline expires.
async fn wait_for_session_closed(
    handle: &Arc<ServerHandle>,
    expected: u64,
    deadline: Instant,
) -> bool {
    while Instant::now() < deadline {
        for event in handle.poll_events() {
            if matches!(event, ServerEvent::SessionClosed(id) if id == expected) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

/// Polls `handle.poll_events()` until a `SessionCreated` is observed
/// and returns its session id, or `None` on timeout.
async fn wait_for_session_created(handle: &Arc<ServerHandle>, deadline: Instant) -> Option<u64> {
    while Instant::now() < deadline {
        for event in handle.poll_events() {
            if let ServerEvent::SessionCreated(id, _) = event {
                return Some(id);
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    None
}

/// Polls `handle.poll_events()` until `Listening(addr)` is observed.
async fn wait_for_listening(handle: &Arc<ServerHandle>, deadline: Instant) -> Option<SocketAddr> {
    while Instant::now() < deadline {
        for event in handle.poll_events() {
            if let ServerEvent::Listening(addr) = event {
                return Some(addr);
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_server_releases_session_slots_after_disconnect() {
    let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
    let server_cfg = TcpServerConfig::new(bind_addr);

    let (mut server, handle) = ServerBuilder::<NoopHandler>::new()
        .bind_config(server_cfg)
        .handler(NoopHandler)
        .max_connections(MAX_CONNECTIONS)
        .build();
    let handle = Arc::new(handle);

    // Spawn the server on the runtime.
    let server_task = tokio::spawn(async move {
        let _ = server.run().await;
    });

    // Wait for ServerEvent::Listening so we know the effective addr.
    let listen_deadline = Instant::now() + Duration::from_secs(2);
    let server_addr = wait_for_listening(&handle, listen_deadline)
        .await
        .expect("server did not emit Listening within 2 s");

    // Open and close TOTAL_CONNECTS sessions sequentially.  Each cycle
    // waits for SessionCreated and SessionClosed before moving on so
    // the run loop has had a chance to drain the cleanup command.
    // Without the fix, the (MAX_CONNECTIONS + 1)-th cycle would hang
    // waiting for SessionCreated because the slot was never freed.
    for cycle in 0..TOTAL_CONNECTS {
        let mut stream = TcpStream::connect(server_addr)
            .await
            .unwrap_or_else(|e| panic!("connect cycle {cycle} failed: {e}"));

        // Wait for the server to register the new session.
        let session_id = wait_for_session_created(&handle, Instant::now() + PER_CYCLE_TIMEOUT)
            .await
            .unwrap_or_else(|| panic!("cycle {cycle} did not see SessionCreated within 5 s"));

        // Send one length-prefixed dummy frame so the session loop
        // exercises a recv at least once before close.  4-byte LE
        // length + zero-byte payload is a valid framed message.
        let frame = 0u32.to_le_bytes();
        stream.write_all(&frame).await.expect("write");
        stream.shutdown().await.expect("shutdown");

        // Drain any reply bytes the server might have queued (echo
        // handler is a no-op so there are none, but this lets us
        // observe EOF cleanly).
        let mut sink = Vec::new();
        let _ = stream.read_to_end(&mut sink).await;
        drop(stream);

        // Wait for the SessionClosed event paired with this session.
        let closed =
            wait_for_session_closed(&handle, session_id, Instant::now() + PER_CYCLE_TIMEOUT).await;
        assert!(
            closed,
            "cycle {cycle} session {session_id} did not emit SessionClosed within 5 s",
        );
    }

    handle.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
}
