//! Tier 3 — multi-client integration tests.
//!
//! Drives several `Client`s against a single `Server`, verifying
//! that distinct clients get distinct echoes, that the
//! `max_connections` cap is honoured, and that the server's
//! session-count signal (via `SessionCreated` / `SessionClosed`
//! events) tracks reality.

mod common;

use common::{
    DEFAULT_WAIT, build_and_start_client, build_and_start_server, build_sbe_message,
    wait_for_client_connected, wait_for_client_message,
};
use ironsbe_core::header::MessageHeader;
use ironsbe_server::{MessageHandler, Responder, ServerEvent};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// Echoes every received frame back to the sender.  Reused across
/// the multi-client tests.
struct EchoHandler {
    started: Arc<AtomicUsize>,
}

impl EchoHandler {
    fn new(started: Arc<AtomicUsize>) -> Self {
        Self { started }
    }
}

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

    fn on_session_start(&self, _session_id: u64) {
        self.started.fetch_add(1, Ordering::SeqCst);
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
async fn test_concurrent_clients_send_distinct_messages() {
    let outer = timeout(Duration::from_secs(10), async {
        const N: usize = 10;
        let started = Arc::new(AtomicUsize::new(0));
        let handler = EchoHandler::new(Arc::clone(&started));

        let (_server_handle, addr, server_task) = build_and_start_server(handler, 32).await;

        let mut tasks = Vec::with_capacity(N);
        for i in 0..N {
            tasks.push(tokio::spawn(async move {
                let (mut client_handle, client_task) =
                    build_and_start_client(addr, Duration::from_secs(2), 0).await;

                assert!(
                    wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT)
                        .await,
                    "client {i} did not observe Connected"
                );

                let payload = format!("client-{i:02}-payload");
                let frame = build_sbe_message(0xA000 + i as u16, payload.as_bytes());
                client_handle
                    .send(frame.clone())
                    .expect("client send failed");

                let echoed =
                    wait_for_client_message(&mut client_handle, Instant::now() + DEFAULT_WAIT)
                        .await
                        .unwrap_or_else(|| panic!("client {i} did not receive its echo"));
                assert_eq!(echoed, frame, "client {i} got the wrong echo");

                client_handle.disconnect();
                let _ = client_task.await;
            }));
        }

        for (i, task) in tasks.into_iter().enumerate() {
            task.await
                .unwrap_or_else(|e| panic!("client task {i} panicked: {e:?}"));
        }

        assert!(
            wait_for_counter(&started, N, Instant::now() + DEFAULT_WAIT).await,
            "expected {N} on_session_start calls, got {}",
            started.load(Ordering::SeqCst)
        );

        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_max_connections_rejects_over_limit() {
    let outer = timeout(Duration::from_secs(10), async {
        const CAP: usize = 3;
        const ATTEMPTS: usize = 5;
        let started = Arc::new(AtomicUsize::new(0));
        let handler = EchoHandler::new(Arc::clone(&started));

        let (_server_handle, addr, server_task) = build_and_start_server(handler, CAP).await;

        // Open ATTEMPTS clients and keep them all connected concurrently.
        let mut handles = Vec::with_capacity(ATTEMPTS);
        for _ in 0..ATTEMPTS {
            let (client_handle, client_task) =
                build_and_start_client(addr, Duration::from_secs(2), 0).await;
            handles.push((client_handle, client_task));
        }

        // Give the server enough time to process every accept.  We can't
        // poll on a "rejection" event because the server logs a warning
        // and silently drops the conn — so we let the run loop quiesce
        // and then assert on_session_start was called *at most* CAP times.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && started.load(Ordering::SeqCst) < CAP {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Drain a bit longer to allow any over-limit connections to be
        // accepted-then-rejected.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let observed = started.load(Ordering::SeqCst);
        assert_eq!(
            observed, CAP,
            "expected exactly {CAP} sessions to start, got {observed}"
        );

        for (mut handle, task) in handles {
            handle.disconnect();
            let _ = task.await;
        }
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_session_count_reported_correctly() {
    let outer = timeout(Duration::from_secs(10), async {
        let started = Arc::new(AtomicUsize::new(0));
        let handler = EchoHandler::new(Arc::clone(&started));

        let (server_handle, addr, server_task) = build_and_start_server(handler, 16).await;

        // Connect three clients concurrently.
        let mut clients = Vec::new();
        for _ in 0..3 {
            let (mut handle, task) = build_and_start_client(addr, Duration::from_secs(2), 0).await;
            assert!(
                wait_for_client_connected(&mut handle, Instant::now() + DEFAULT_WAIT).await,
                "client did not observe Connected"
            );
            clients.push((handle, task));
        }

        assert!(
            wait_for_counter(&started, 3, Instant::now() + DEFAULT_WAIT).await,
            "expected 3 session starts, got {}",
            started.load(Ordering::SeqCst)
        );

        // Drain whatever the server has emitted so far and replay it to
        // derive the live session count via SessionCreated/SessionClosed.
        let mut created = 0usize;
        let mut closed = 0usize;
        // Allow the server time to flush all create events.
        let deadline = Instant::now() + DEFAULT_WAIT;
        while Instant::now() < deadline && created < 3 {
            for ev in server_handle.poll_events() {
                match ev {
                    ServerEvent::SessionCreated(_, _) => created += 1,
                    ServerEvent::SessionClosed(_) => closed += 1,
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(created, 3, "expected 3 SessionCreated events");
        assert_eq!(closed, 0, "no sessions should be closed yet");

        // Disconnect one client and verify the count drops by exactly one.
        let (mut h0, t0) = clients.remove(0);
        h0.disconnect();
        let _ = t0.await;

        let deadline = Instant::now() + DEFAULT_WAIT;
        while Instant::now() < deadline && closed < 1 {
            for ev in server_handle.poll_events() {
                if let ServerEvent::SessionClosed(_) = ev {
                    closed += 1;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(closed, 1, "expected exactly 1 SessionClosed");
        let live = created.saturating_sub(closed);
        assert_eq!(live, 2, "expected 2 live sessions, got {live}");

        // Tear down the rest.
        for (mut h, t) in clients {
            h.disconnect();
            let _ = t.await;
        }
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}
