//! Tier 5 — shutdown / handle-surface integration tests.
//!
//! Exercises every command-channel surface on `ServerHandle` and
//! `ClientHandle` against a real running server + client.
//!
//! Several tests in this file are gated `#[ignore]` because they
//! surface real bugs in the server's command-handling path.  Each
//! ignore reason links the GitHub issue tracking the fix; once the
//! fix lands the gate can be removed.

mod common;

use common::{
    DEFAULT_WAIT, build_and_start_client, build_and_start_server, build_sbe_message,
    wait_for_client_connected, wait_for_client_message, wait_for_session_created,
};
use ironsbe_client::ClientEvent;
use ironsbe_core::header::MessageHeader;
use ironsbe_server::{MessageHandler, Responder};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// Echo handler that mirrors every received frame back to its sender.
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

/// Counts session start/end transitions so the test can wait on the
/// exact lifecycle without polling the server's event channel.
#[derive(Default)]
struct LifecycleHandler {
    started: Arc<AtomicUsize>,
    ended: Arc<AtomicUsize>,
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

    fn on_session_start(&self, _session_id: u64) {
        self.started.fetch_add(1, Ordering::SeqCst);
    }

    fn on_session_end(&self, _session_id: u64) {
        self.ended.fetch_add(1, Ordering::SeqCst);
    }
}

/// Records the most recent session id that received a message and
/// echoes the bytes back via `responder.send_to(other_session, ...)`.
/// Used to exercise the cross-session routing surface.
struct CrossRouteHandler {
    last_session: Arc<AtomicU64>,
    target: Arc<AtomicU64>,
    routed_payloads: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl MessageHandler for CrossRouteHandler {
    fn on_message(
        &self,
        session_id: u64,
        _header: &MessageHeader,
        buffer: &[u8],
        responder: &dyn Responder,
    ) {
        self.last_session.store(session_id, Ordering::SeqCst);
        if let Ok(mut guard) = self.routed_payloads.lock() {
            guard.push(buffer.to_vec());
        }
        let target = self.target.load(Ordering::SeqCst);
        if target != 0 && target != session_id {
            let _ = responder.send_to(target, buffer);
        }
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
async fn test_server_handle_shutdown_stops_run_loop() {
    let outer = timeout(Duration::from_secs(5), async {
        let (server_handle, addr, server_task) = build_and_start_server(EchoHandler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        // Issue shutdown.  The run loop must return cleanly within the
        // outer timeout.
        server_handle.shutdown();

        let join = timeout(Duration::from_secs(3), server_task)
            .await
            .expect("server.run did not exit after shutdown");
        assert!(join.is_ok(), "server task panicked: {join:?}");

        client_handle.disconnect();
        let _ = client_task.await;
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
#[ignore = "tracked in #42 — close_session does not terminate the underlying connection"]
async fn test_server_handle_close_session_closes_that_session_only() {
    let outer = timeout(Duration::from_secs(5), async {
        let handler = LifecycleHandler::default();
        let started = Arc::clone(&handler.started);
        let ended = Arc::clone(&handler.ended);

        let (server_handle, addr, server_task) = build_and_start_server(handler, 16).await;

        // Open two clients.
        let (mut c1, t1) = build_and_start_client(addr, Duration::from_secs(2), 0).await;
        let (mut c2, t2) = build_and_start_client(addr, Duration::from_secs(2), 0).await;
        assert!(wait_for_client_connected(&mut c1, Instant::now() + DEFAULT_WAIT).await);
        assert!(wait_for_client_connected(&mut c2, Instant::now() + DEFAULT_WAIT).await);
        assert!(wait_for_counter(&started, 2, Instant::now() + DEFAULT_WAIT).await);

        // Pick the first session id we observe and ask the server to
        // close it.
        let target_session =
            wait_for_session_created(&server_handle, Instant::now() + DEFAULT_WAIT)
                .await
                .expect("no SessionCreated observed");
        server_handle.close_session(target_session);

        // Exactly one on_session_end must fire — the other client must
        // remain connected.
        assert!(
            wait_for_counter(&ended, 1, Instant::now() + DEFAULT_WAIT).await,
            "close_session did not terminate the targeted session"
        );
        // Wait a bit longer to make sure no spurious second close fires.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            ended.load(Ordering::SeqCst),
            1,
            "close_session terminated more than one session"
        );

        c1.disconnect();
        c2.disconnect();
        let _ = t1.await;
        let _ = t2.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
#[ignore = "tracked in #40 — ServerCommand::Broadcast is a no-op"]
async fn test_server_handle_broadcast_reaches_all_sessions() {
    let outer = timeout(Duration::from_secs(5), async {
        let (server_handle, addr, server_task) = build_and_start_server(EchoHandler, 16).await;

        // Connect two clients and confirm the server saw both.
        let (mut c1, t1) = build_and_start_client(addr, Duration::from_secs(2), 0).await;
        let (mut c2, t2) = build_and_start_client(addr, Duration::from_secs(2), 0).await;
        assert!(wait_for_client_connected(&mut c1, Instant::now() + DEFAULT_WAIT).await);
        assert!(wait_for_client_connected(&mut c2, Instant::now() + DEFAULT_WAIT).await);

        // Broadcast a single SBE message — both clients must receive it.
        let frame = build_sbe_message(0xBEEF, b"broadcast-payload");
        server_handle.broadcast(frame.clone());

        let m1 = wait_for_client_message(&mut c1, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("client 1 did not receive broadcast");
        let m2 = wait_for_client_message(&mut c2, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("client 2 did not receive broadcast");
        assert_eq!(m1, frame);
        assert_eq!(m2, frame);

        c1.disconnect();
        c2.disconnect();
        let _ = t1.await;
        let _ = t2.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_client_handle_disconnect_returns_from_run() {
    let outer = timeout(Duration::from_secs(5), async {
        let (_server_handle, addr, server_task) = build_and_start_server(EchoHandler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        client_handle.disconnect();

        // Client::run must return within a couple of seconds.
        let join = timeout(Duration::from_secs(2), client_task)
            .await
            .expect("client.run did not exit after disconnect");
        assert!(join.is_ok(), "client task panicked: {join:?}");

        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
async fn test_client_handle_wait_event_resolves_on_message() {
    let outer = timeout(Duration::from_secs(5), async {
        let (_server_handle, addr, server_task) = build_and_start_server(EchoHandler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );

        // Send a frame and resolve via wait_event() — must yield a
        // Message variant within the deadline.
        let frame = build_sbe_message(7, b"wait-event-payload");
        client_handle
            .send(frame.clone())
            .expect("client send failed");

        let event = timeout(Duration::from_secs(2), client_handle.wait_event())
            .await
            .expect("wait_event did not resolve within 2 s")
            .expect("wait_event returned None (sender dropped)");
        match event {
            ClientEvent::Message(bytes) => assert_eq!(bytes, frame),
            other => panic!("expected Message event, got {other:?}"),
        }

        client_handle.disconnect();
        let _ = client_task.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
#[ignore = "tracked in #41 — Responder::send_to ignores session_id"]
async fn test_responder_send_to_routes_across_sessions() {
    let outer = timeout(Duration::from_secs(5), async {
        let last_session = Arc::new(AtomicU64::new(0));
        let target = Arc::new(AtomicU64::new(0));
        let routed_payloads = Arc::new(Mutex::new(Vec::new()));
        let handler = CrossRouteHandler {
            last_session: Arc::clone(&last_session),
            target: Arc::clone(&target),
            routed_payloads: Arc::clone(&routed_payloads),
        };

        let (server_handle, addr, server_task) = build_and_start_server(handler, 16).await;

        // Two clients: A and B.  We send from A and expect B to receive
        // the bytes via `send_to(B, ...)`.
        let (mut a, ta) = build_and_start_client(addr, Duration::from_secs(2), 0).await;
        let (mut b, tb) = build_and_start_client(addr, Duration::from_secs(2), 0).await;
        assert!(wait_for_client_connected(&mut a, Instant::now() + DEFAULT_WAIT).await);
        assert!(wait_for_client_connected(&mut b, Instant::now() + DEFAULT_WAIT).await);

        // Discover both session ids in order so we know which is B.
        let s1 = wait_for_session_created(&server_handle, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("first SessionCreated missing");
        let s2 = wait_for_session_created(&server_handle, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("second SessionCreated missing");
        // The handler routes to whichever session is *not* the sender.
        // We seed it with s2 so messages from s1 (which the test always
        // sends from first) get routed to s2.
        target.store(s2, Ordering::SeqCst);
        let _ = s1;

        let frame = build_sbe_message(0xCAFE, b"cross-routed");
        a.send(frame.clone()).expect("client A send failed");

        let received_on_b = wait_for_client_message(&mut b, Instant::now() + DEFAULT_WAIT)
            .await
            .expect("client B did not receive cross-routed payload");
        assert_eq!(received_on_b, frame);

        a.disconnect();
        b.disconnect();
        let _ = ta.await;
        let _ = tb.await;
        server_task.abort();
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}

#[tokio::test]
#[ignore = "tracked in #42 — Server::run shutdown does not cancel spawned session tasks"]
async fn test_shutdown_signals_spawned_session_tasks() {
    let outer = timeout(Duration::from_secs(5), async {
        let handler = LifecycleHandler::default();
        let started = Arc::clone(&handler.started);
        let ended = Arc::clone(&handler.ended);

        let (server_handle, addr, server_task) = build_and_start_server(handler, 16).await;
        let (mut client_handle, client_task) =
            build_and_start_client(addr, Duration::from_secs(2), 0).await;

        assert!(
            wait_for_client_connected(&mut client_handle, Instant::now() + DEFAULT_WAIT).await,
            "client did not observe Connected"
        );
        assert!(wait_for_counter(&started, 1, Instant::now() + DEFAULT_WAIT).await);

        // Issue shutdown — the spawned session task must be cancelled,
        // on_session_end must fire, and the client must observe the
        // disconnect.
        server_handle.shutdown();

        let _ = timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server.run did not exit after shutdown");

        assert!(
            wait_for_counter(&ended, 1, Instant::now() + Duration::from_secs(2)).await,
            "on_session_end did not fire after shutdown — spawned session task was not cancelled"
        );

        // Client must observe Disconnected (or Error) within the
        // deadline as a side effect of the server shutting down.
        let mut got_disconnect = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && !got_disconnect {
            if let Some(ev) = client_handle.poll()
                && matches!(ev, ClientEvent::Disconnected | ClientEvent::Error(_))
            {
                got_disconnect = true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            got_disconnect,
            "client did not observe Disconnected after server shutdown"
        );

        client_handle.disconnect();
        let _ = client_task.await;
    })
    .await;
    assert!(outer.is_ok(), "test exceeded outer timeout");
}
