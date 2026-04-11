//! Tier 2 — session lifecycle integration tests.
//!
//! Verifies that `MessageHandler::on_session_start` /
//! `on_session_end` callbacks fire at the correct times and that the
//! corresponding `ServerEvent::SessionCreated` / `SessionClosed`
//! events are observable through `ServerHandle::poll_events`.

mod common;

use common::{
    DEFAULT_WAIT, build_and_start_client, build_and_start_server, wait_for_client_connected,
    wait_for_session_closed, wait_for_session_created,
};
use ironsbe_core::header::MessageHeader;
use ironsbe_server::{MessageHandler, Responder};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// Records every callback the handler receives so the test can assert
/// on the exact lifecycle observed for each session.
#[derive(Default)]
struct LifecycleHandler {
    started: Arc<AtomicUsize>,
    ended: Arc<AtomicUsize>,
    last_started_id: Arc<AtomicU64>,
    last_ended_id: Arc<AtomicU64>,
    started_ids: Arc<Mutex<Vec<u64>>>,
}

impl LifecycleHandler {
    fn new() -> Self {
        Self::default()
    }
}

impl MessageHandler for LifecycleHandler {
    fn on_message(
        &self,
        _session_id: u64,
        _header: &MessageHeader,
        _buffer: &[u8],
        _responder: &dyn Responder,
    ) {
    }

    fn on_session_start(&self, session_id: u64) {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.last_started_id.store(session_id, Ordering::SeqCst);
        if let Ok(mut ids) = self.started_ids.lock() {
            ids.push(session_id);
        }
    }

    fn on_session_end(&self, session_id: u64) {
        self.ended.fetch_add(1, Ordering::SeqCst);
        self.last_ended_id.store(session_id, Ordering::SeqCst);
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
async fn test_on_session_start_called_on_connect() {
    let outer = timeout(Duration::from_secs(5), async {
        let handler = LifecycleHandler::new();
        let started = Arc::clone(&handler.started);

        let (_server_handle, addr, server_task) = build_and_start_server(handler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );
        assert!(
            wait_for_counter(&started, 1, Instant::now() + DEFAULT_WAIT).await,
            "on_session_start was not called"
        );

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_on_session_end_called_on_client_disconnect() {
    let outer = timeout(Duration::from_secs(5), async {
        let handler = LifecycleHandler::new();
        let started = Arc::clone(&handler.started);
        let ended = Arc::clone(&handler.ended);

        let (_server_handle, addr, server_task) = build_and_start_server(handler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );
        assert!(
            wait_for_counter(&started, 1, Instant::now() + DEFAULT_WAIT).await,
            "on_session_start was not called"
        );

        client_handle.disconnect();
        let _ = client_task.await;

        assert!(
            wait_for_counter(&ended, 1, Instant::now() + DEFAULT_WAIT).await,
            "on_session_end was not called after disconnect"
        );

        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_session_created_event_observed_via_handle() {
    let outer = timeout(Duration::from_secs(5), async {
        let handler = LifecycleHandler::new();
        let (server_handle, addr, server_task) = build_and_start_server(handler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        let session_id = wait_for_session_created(&server_handle, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("SessionCreated not observed via handle");
        assert!(session_id >= 1, "session id should be monotonic");

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_session_closed_event_observed_via_handle() {
    let outer = timeout(Duration::from_secs(5), async {
        let handler = LifecycleHandler::new();
        let (server_handle, addr, server_task) = build_and_start_server(handler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        let session_id = wait_for_session_created(&server_handle, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("SessionCreated not observed via handle");

        client_handle.disconnect();
        let _ = client_task.await;

        assert!(
            wait_for_session_closed(&server_handle, session_id, Instant::now() + DEFAULT_WAIT)
                .await,
            "SessionClosed event not observed for session {session_id}"
        );

        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_sequential_session_ids_monotonic() {
    let outer = timeout(Duration::from_secs(10), async {
        let handler = LifecycleHandler::new();
        let started = Arc::clone(&handler.started);
        let started_ids = Arc::clone(&handler.started_ids);

        let (_server_handle, addr, server_task) = build_and_start_server(handler, 16).await;

        // Open three clients sequentially, each one disconnecting before
        // the next connects, so the run loop has time to drain CloseSession.
        for i in 0..3 {
            let (mut client_handle, client_task) =
                build_and_start_client(addr, Duration::from_secs(2), 0).await;
            assert!(
                wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
                "client {i} did not observe Connected"
            );
            assert!(
                wait_for_counter(&started, i + 1, Instant::now() + DEFAULT_WAIT).await,
                "on_session_start count did not reach {} for client {i}",
                i + 1
            );

            client_handle.disconnect();
            let _ = client_task.await;
        }

        let ids = started_ids.lock().expect("started_ids poisoned").clone();
        assert_eq!(ids.len(), 3, "expected 3 session_start callbacks");
        for window in ids.windows(2) {
            assert!(
                window[1] > window[0],
                "session ids must be strictly monotonic; got {ids:?}"
            );
        }

        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}
