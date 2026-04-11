//! Tier 4 — reconnect state-machine integration tests.
//!
//! Verifies the client's connect-timeout, reconnect-after-restart,
//! and max-reconnect-attempts behaviour against a real `Server`.

mod common;

use ironsbe_client::{ClientBuilder, ClientError, ClientEvent, ClientHandle};
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

/// Reserves a loopback port by binding a `std::net::TcpListener` and
/// dropping it immediately.
///
/// There is an inherent TOCTOU race: another process can reclaim the
/// port between the drop and the subsequent bind by the real server.
/// The alternative — using a routed TEST-NET-1 address — would fail
/// fast with ECONNREFUSED/ENETUNREACH on most CI runners instead of
/// exercising the reconnect path, so we accept the narrow race in
/// exchange for a realistic loopback target.  If the port is reclaimed
/// concurrently, the tests will simply surface a bind/connect error
/// instead of the reconnect event they expect — a loud failure, not a
/// silent corruption.
fn reserve_unused_port() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let addr = listener.local_addr().expect("local_addr");
    drop(listener);
    addr
}

/// Polls a `ClientHandle` until a `ClientEvent` matching `pred` is
/// observed or the deadline expires.  Used to drive the reconnect
/// tests off of real client state transitions instead of wall-clock
/// sleeps.
async fn wait_for_client_event<F>(handle: &mut ClientHandle, pred: F, deadline: Instant) -> bool
where
    F: Fn(&ClientEvent) -> bool,
{
    while Instant::now() < deadline {
        if let Some(event) = handle.poll()
            && pred(&event)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

/// Asserts that `Client::run` surfaces a failure (not a hang) when the
/// target address is not listening.  Loopback + closed port typically
/// RSTs immediately and maps to `ClientError::Io`; on hosts where the
/// OS instead drops the SYN silently we get `ClientError::ConnectTimeout`.
/// Both outcomes are valid for this test — the contract under test is
/// "connect fails within the deadline", not a specific error variant.
#[tokio::test]
async fn test_client_connect_failure_on_unreachable_addr() {
    let outer = timeout(Duration::from_secs(8), async {
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

        // Wait for the client to *actually fail* at least one connect
        // attempt so we know the reconnect loop is genuinely active
        // when we bring the server up.  The client emits `Disconnected`
        // on every failed attempt.
        assert!(
            wait_for_client_event(
                &mut client_handle,
                |e| matches!(e, ClientEvent::Disconnected),
                Instant::now() + Duration::from_secs(3),
            )
            .await,
            "client never observed a first failed attempt"
        );

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

        // The client's reconnect loop should pick up the new server
        // and emit a fresh `Connected` event.
        assert!(
            wait_for_client_event(
                &mut client_handle,
                |e| matches!(e, ClientEvent::Connected),
                Instant::now() + Duration::from_secs(8),
            )
            .await,
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
