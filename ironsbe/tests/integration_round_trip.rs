//! Tier 1 — round-trip integration tests for `Server` + `Client`.
//!
//! Each test spins up a real `Server` on `127.0.0.1:0`, drives a real
//! `Client` over the default `tcp-tokio` backend, and verifies the
//! full SBE message lifecycle through the high-level handles.

mod common;

use common::{
    DEFAULT_WAIT, build_and_start_client, build_and_start_server, build_sbe_message,
    wait_for_client_connected, wait_for_client_message,
};
use ironsbe_client::{ClientBuilder, ClientEvent};
use ironsbe_core::header::MessageHeader;
use ironsbe_server::{MessageHandler, Responder, ServerBuilder, ServerEvent, ServerHandle};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// Server-side handler that echoes every received message back to the
/// sender.  Used by the round-trip tests to verify the client observes
/// the bytes it just sent.
struct EchoHandler;

impl MessageHandler for EchoHandler {
    fn on_message(
        &self,
        _session_id: u64,
        _header: &MessageHeader,
        buffer: &[u8],
        responder: &dyn Responder,
    ) {
        let _ = responder.send(buffer);
    }
}

/// Server-side handler that counts decode errors and message arrivals
/// without sending any reply.  Used by the truncated-header test.
struct CountingHandler {
    received: Arc<AtomicUsize>,
    errors: Arc<AtomicUsize>,
}

impl MessageHandler for CountingHandler {
    fn on_message(
        &self,
        _session_id: u64,
        _header: &MessageHeader,
        _buffer: &[u8],
        _responder: &dyn Responder,
    ) {
        self.received.fetch_add(1, Ordering::SeqCst);
    }

    fn on_error(&self, _session_id: u64, _error: &str) {
        self.errors.fetch_add(1, Ordering::SeqCst);
    }
}

/// Polls a `ServerHandle` until a `SessionClosed` event is observed
/// (regardless of session id) or the deadline expires.
async fn wait_for_any_session_closed(handle: &Arc<ServerHandle>, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        for event in handle.poll_events() {
            if matches!(event, ServerEvent::SessionClosed(_)) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

/// Polls an atomic counter until it reaches `target` or the deadline
/// expires.
async fn wait_for_counter(counter: &Arc<AtomicUsize>, target: usize, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        if counter.load(Ordering::SeqCst) >= target {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    counter.load(Ordering::SeqCst) >= target
}

#[tokio::test]
async fn test_single_sbe_message_round_trip() {
    let outer = timeout(Duration::from_secs(5), async {
        let (_server_handle, addr, server_task) = build_and_start_server(EchoHandler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        let deadline = Instant::now() + DEFAULT_WAIT;
        assert!(
            wait_for_client_connected(&mut client_handle, deadline).await,
            "client did not observe Connected"
        );

        let payload = b"hello-sbe";
        let frame = build_sbe_message(42, payload);
        client_handle
            .send(frame.clone())
            .expect("client send failed");

        let echoed = wait_for_client_message(&mut client_handle, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("client did not receive echoed message");
        assert_eq!(echoed, frame, "echoed bytes must match what we sent");

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_many_messages_in_sequence() {
    let outer = timeout(Duration::from_secs(10), async {
        let (_server_handle, addr, server_task) = build_and_start_server(EchoHandler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        let deadline = Instant::now() + DEFAULT_WAIT;
        assert!(
            wait_for_client_connected(&mut client_handle, deadline).await,
            "client did not observe Connected"
        );

        const ITERATIONS: usize = 100;
        for i in 0..ITERATIONS {
            let payload = format!("msg-{i:04}");
            let frame = build_sbe_message(7, payload.as_bytes());
            client_handle
                .send(frame.clone())
                .expect("client send failed");

            let echoed = wait_for_client_message(&mut client_handle, Instant::now() + DEFAULT_WAIT)
                .await
                .unwrap_or_else(|| panic!("client missed echo {i}"));
            assert_eq!(echoed, frame, "echo {i} mismatch");
        }

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_large_message_within_default_frame_size() {
    let outer = timeout(Duration::from_secs(5), async {
        let (_server_handle, addr, server_task) = build_and_start_server(EchoHandler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        let deadline = Instant::now() + DEFAULT_WAIT;
        assert!(
            wait_for_client_connected(&mut client_handle, deadline).await,
            "client did not observe Connected"
        );

        // 60 KB total frame fits inside the default 64 KB max_frame_size.
        let payload = vec![0xABu8; 60 * 1024 - MessageHeader::ENCODED_LENGTH];
        let frame = build_sbe_message(99, &payload);
        client_handle
            .send(frame.clone())
            .expect("client send failed");

        let echoed = wait_for_client_message(&mut client_handle, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("client did not receive 60 KB echo");
        assert_eq!(echoed.len(), frame.len());
        assert_eq!(echoed, frame);

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_message_with_custom_max_frame_size_256kb() {
    let outer = timeout(Duration::from_secs(10), async {
        // Custom server: 256 KB frames.
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("hardcoded addr");
        let (mut server, handle): (_, _) = ServerBuilder::<EchoHandler>::new()
            .bind(bind_addr)
            .handler(EchoHandler)
            .max_connections(16)
            .max_frame_size(256 * 1024)
            .build();
        let handle = Arc::new(handle);

        let server_task = tokio::spawn(async move {
            let _ = server.run().await;
        });

        // Wait for the server to publish its effective bound address.
        let deadline = Instant::now() + DEFAULT_WAIT;
        let server_addr = common::wait_for_listening(&handle, deadline)
            .await
            .expect("server did not emit Listening");

        // Custom client: 256 KB frames, no reconnect.
        let (client, mut client_handle) = ClientBuilder::new(server_addr)
            .connect_timeout(Duration::from_secs(2))
            .reconnect(false)
            .max_frame_size(256 * 1024)
            .build();
        let mut client = client;
        let client_task = tokio::spawn(async move {
            let _ = client.run().await;
        });

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        // 200 KB payload, well above the 64 KB default but inside 256 KB.
        let payload = vec![0x5Au8; 200 * 1024 - MessageHeader::ENCODED_LENGTH];
        let frame = build_sbe_message(123, &payload);
        client_handle
            .send(frame.clone())
            .expect("client send failed");

        let echoed = wait_for_client_message(&mut client_handle, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("client did not receive 200 KB echo");
        assert_eq!(echoed.len(), frame.len(), "echoed length mismatch");
        assert_eq!(echoed, frame, "echoed bytes mismatch");

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_frame_exceeding_max_size_drops_connection() {
    let outer = timeout(Duration::from_secs(5), async {
        // Server keeps the default 64 KB frame ceiling.
        let (server_handle, addr, server_task) = build_and_start_server(EchoHandler, 16).await;

        // Client opts into a 1 MB frame size so it can encode something
        // the server is bound to reject.
        let (client, mut client_handle) = ClientBuilder::new(addr)
            .connect_timeout(Duration::from_secs(2))
            .reconnect(false)
            .max_frame_size(1024 * 1024)
            .build();
        let mut client = client;
        let client_task = tokio::spawn(async move {
            let _ = client.run().await;
        });

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        // 100 KB > server's 64 KB ceiling — server's decoder must error
        // and tear the session down.
        let payload = vec![0xCCu8; 100 * 1024];
        let frame = build_sbe_message(7, &payload);
        client_handle.send(frame).expect("client send failed");

        assert!(
            wait_for_any_session_closed(&server_handle, Instant::now() + DEFAULT_WAIT).await,
            "server did not close session after oversized frame"
        );

        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_truncated_header_reports_on_error() {
    let outer = timeout(Duration::from_secs(5), async {
        let received = Arc::new(AtomicUsize::new(0));
        let errors = Arc::new(AtomicUsize::new(0));
        let handler = CountingHandler {
            received: Arc::clone(&received),
            errors: Arc::clone(&errors),
        };

        let (_server_handle, addr, server_task) = build_and_start_server(handler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        // 4-byte payload — strictly shorter than the 8-byte SBE header,
        // so handle_session must dispatch on_error instead of on_message.
        client_handle
            .send(vec![0xDEu8, 0xAD, 0xBE, 0xEF])
            .expect("client send failed");

        assert!(
            wait_for_counter(&errors, 1, Instant::now() + DEFAULT_WAIT).await,
            "handler.on_error was not invoked for the truncated frame"
        );
        assert_eq!(
            received.load(Ordering::SeqCst),
            0,
            "on_message must not fire for a sub-header frame"
        );

        // Drain any spurious events without asserting.
        let _ = client_handle
            .poll()
            .filter(|e| !matches!(e, ClientEvent::Connected));

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}
