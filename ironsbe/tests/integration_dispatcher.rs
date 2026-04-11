//! Tier 6 — `MessageDispatcher` routing integration tests.
//!
//! Verifies that `MessageDispatcher` routes incoming SBE messages to
//! the correct `TypedHandler` based on the wire-decoded `template_id`,
//! and that unknown template ids fall through to the configured
//! default handler.

mod common;

use common::{
    DEFAULT_WAIT, build_and_start_client, build_and_start_server, build_sbe_message,
    wait_for_client_connected,
};
use ironsbe_core::header::MessageHeader;
use ironsbe_server::handler::FnHandler;
use ironsbe_server::{MessageDispatcher, MessageHandler, Responder};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::time::timeout;

const TEMPLATE_A: u16 = 0x1001;
const TEMPLATE_B: u16 = 0x1002;
const TEMPLATE_UNKNOWN: u16 = 0xDEAD;

/// Default handler used by the dispatcher when no typed handler is
/// registered for the incoming template id.
struct FallbackHandler {
    fallback_count: Arc<AtomicUsize>,
}

impl MessageHandler for FallbackHandler {
    fn on_message(
        &self,
        _session_id: u64,
        _header: &MessageHeader,
        _buffer: &[u8],
        _responder: &dyn Responder,
    ) {
        self.fallback_count.fetch_add(1, Ordering::SeqCst);
    }
}

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
async fn test_dispatcher_routes_by_template_id() {
    let outer = timeout(Duration::from_secs(5), async {
        let count_a = Arc::new(AtomicUsize::new(0));
        let count_b = Arc::new(AtomicUsize::new(0));

        let mut dispatcher = MessageDispatcher::new();
        let count_a_clone = Arc::clone(&count_a);
        dispatcher.register(
            TEMPLATE_A,
            FnHandler::new(move |_session_id, _buffer, _responder| {
                count_a_clone.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let count_b_clone = Arc::clone(&count_b);
        dispatcher.register(
            TEMPLATE_B,
            FnHandler::new(move |_session_id, _buffer, _responder| {
                count_b_clone.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let (_server_handle, addr, server_task) = build_and_start_server(dispatcher, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        // Send three A frames and one B frame, then verify the
        // dispatcher routed each to the correct typed handler.
        for _ in 0..3 {
            client_handle
                .send(build_sbe_message(TEMPLATE_A, b"a-payload"))
                .expect("client send A failed");
        }
        client_handle
            .send(build_sbe_message(TEMPLATE_B, b"b-payload"))
            .expect("client send B failed");

        assert!(
            wait_for_counter(&count_a, 3, Instant::now() + DEFAULT_WAIT).await,
            "expected 3 A handler invocations, got {}",
            count_a.load(Ordering::SeqCst)
        );
        assert!(
            wait_for_counter(&count_b, 1, Instant::now() + DEFAULT_WAIT).await,
            "expected 1 B handler invocation, got {}",
            count_b.load(Ordering::SeqCst)
        );

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_dispatcher_unknown_template_falls_through() {
    let outer = timeout(Duration::from_secs(5), async {
        let known_count = Arc::new(AtomicUsize::new(0));
        let fallback_count = Arc::new(AtomicUsize::new(0));

        let mut dispatcher = MessageDispatcher::new();
        let known_clone = Arc::clone(&known_count);
        dispatcher.register(
            TEMPLATE_A,
            FnHandler::new(move |_session_id, _buffer, _responder| {
                known_clone.fetch_add(1, Ordering::SeqCst);
            }),
        );
        dispatcher.set_default(FallbackHandler {
            fallback_count: Arc::clone(&fallback_count),
        });

        let (_server_handle, addr, server_task) = build_and_start_server(dispatcher, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        // One known message, two unknown — fallback should fire twice
        // and the typed handler exactly once.
        client_handle
            .send(build_sbe_message(TEMPLATE_A, b"known"))
            .expect("client send known failed");
        client_handle
            .send(build_sbe_message(TEMPLATE_UNKNOWN, b"unknown-1"))
            .expect("client send unknown 1 failed");
        client_handle
            .send(build_sbe_message(TEMPLATE_UNKNOWN, b"unknown-2"))
            .expect("client send unknown 2 failed");

        assert!(
            wait_for_counter(&known_count, 1, Instant::now() + DEFAULT_WAIT).await,
            "expected 1 known invocation"
        );
        assert!(
            wait_for_counter(&fallback_count, 2, Instant::now() + DEFAULT_WAIT).await,
            "expected 2 fallback invocations, got {}",
            fallback_count.load(Ordering::SeqCst)
        );

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}
