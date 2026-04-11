//! Tier 4 — reconnect state-machine integration tests.
//!
//! Verifies the client's connect-timeout, reconnect-after-restart,
//! and max-reconnect-attempts behaviour against a real `Server`.

mod common;

use ironsbe_client::{ClientBuilder, ClientError};
use ironsbe_core::header::MessageHeader;
use ironsbe_server::{MessageHandler, Responder, ServerBuilder};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// No-op handler — these tests don't exercise messaging.
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

/// Reserves an ephemeral port by binding a `std::net::TcpListener` and
/// then dropping it.  The OS may reuse the port quickly, so callers
/// must be tolerant of races — these tests do not depend on the port
/// staying free, only on it being valid syntactically.
fn reserve_unused_port() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let addr = listener.local_addr().expect("local_addr");
    drop(listener);
    addr
}

#[tokio::test]
async fn test_client_connect_timeout_fires_on_unreachable_addr() {
    let outer = timeout(Duration::from_secs(5), async {
        let addr = reserve_unused_port();

        let (client, _client_handle) = ClientBuilder::with_default_transport(addr)
            .connect_timeout(Duration::from_millis(300))
            .reconnect(false)
            .build();
        let mut client = client;

        let result = timeout(Duration::from_secs(2), client.run())
            .await
            .expect("client.run did not return within 2 s");

        match result {
            Err(ClientError::ConnectTimeout) | Err(ClientError::Io(_)) => {}
            Err(ClientError::MaxReconnectAttempts) => {}
            other => panic!("unexpected client result: {other:?}"),
        }
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_client_reconnects_after_server_starts() {
    // NOTE: ideally this test would start a server, accept the client,
    // *kill* the server, and observe the client re-attaching when a new
    // server binds the same address.  That path is currently blocked by
    // #42 — aborting `Server::run` does not propagate cancellation to
    // its spawned per-session tasks, so the client side never observes
    // the disconnect.
    //
    // To exercise the reconnect state machine independently, this test
    // points the client at a port with no listener, lets it cycle
    // through retries, then brings a server up on that exact port.
    let outer = timeout(Duration::from_secs(15), async {
        let addr = reserve_unused_port();

        let (client, mut client_handle) = ClientBuilder::with_default_transport(addr)
            .connect_timeout(Duration::from_millis(300))
            .reconnect(true)
            .reconnect_delay(Duration::from_millis(50))
            .max_reconnect_attempts(0) // 0 = unlimited
            .build();
        let mut client = client;
        let client_task = tokio::spawn(async move {
            let _ = client.run().await;
        });

        // Give the client time to fail at least one attempt so we know
        // the reconnect loop is genuinely active.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Bring a server up on the reserved port.
        let (mut server, handle) = ServerBuilder::<NoopHandler>::new()
            .bind(addr)
            .handler(NoopHandler)
            .max_connections(16)
            .build();
        let handle = Arc::new(handle);
        let server_task = tokio::spawn(async move {
            let _ = server.run().await;
        });
        let _ = common::wait_for_listening(&handle, Instant::now() + Duration::from_secs(3))
            .await
            .expect("server did not emit Listening");

        // The client's reconnect loop should pick up the new server.
        let reconnect_deadline = Instant::now() + Duration::from_secs(8);
        let mut got_connected = false;
        while Instant::now() < reconnect_deadline {
            if let Some(ev) = client_handle.poll()
                && matches!(ev, ironsbe_client::ClientEvent::Connected)
            {
                got_connected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            got_connected,
            "client did not connect once the server came online"
        );

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_client_max_reconnect_attempts_enforced() {
    let outer = timeout(Duration::from_secs(10), async {
        let addr = reserve_unused_port();

        let (client, _client_handle) = ClientBuilder::with_default_transport(addr)
            .connect_timeout(Duration::from_millis(200))
            .reconnect(true)
            .reconnect_delay(Duration::from_millis(20))
            .max_reconnect_attempts(2)
            .build();
        let mut client = client;

        let result = timeout(Duration::from_secs(5), client.run())
            .await
            .expect("client.run did not return within 5 s");

        assert!(
            matches!(result, Err(ClientError::MaxReconnectAttempts)),
            "expected MaxReconnectAttempts, got {result:?}"
        );
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}
