//! Shared helpers for the end-to-end integration tests.
//!
//! These tests spin up a real `Server` + `Client` over the
//! `tcp-tokio` backend, bind to an ephemeral port (`127.0.0.1:0`),
//! and drive the full request/response lifecycle through the
//! high-level APIs.  Every wait is event-driven with an explicit
//! deadline — no fixed sleeps — so the tests are flake-free.

#![allow(dead_code)]

use ironsbe_client::{Client, ClientBuilder, ClientEvent, ClientHandle};
use ironsbe_core::header::MessageHeader;
use ironsbe_server::{MessageHandler, Server, ServerBuilder, ServerEvent, ServerHandle};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// Default deadline for waiting on a single event.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(5);

/// Builds and starts a `Server<H>` on `127.0.0.1:0`, waits for the
/// `Listening` event, and returns the effective bound address plus
/// the spawned server task.
///
/// The returned `ServerHandle` is wrapped in `Arc` so multiple tests
/// can poll events concurrently.
pub async fn build_and_start_server<H>(
    handler: H,
    max_connections: usize,
) -> (Arc<ServerHandle>, SocketAddr, JoinHandle<()>)
where
    H: MessageHandler + 'static,
{
    let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("hardcoded addr");
    let (mut server, handle): (Server<H>, ServerHandle) = ServerBuilder::<H>::new()
        .bind(bind_addr)
        .handler(handler)
        .max_connections(max_connections)
        .build();

    let handle = Arc::new(handle);
    let server_task = tokio::spawn(async move {
        let _ = server.run().await;
    });

    let deadline = Instant::now() + DEFAULT_WAIT;
    let server_addr = wait_for_listening(&handle, deadline)
        .await
        .expect("server did not emit Listening within 5 s");

    (handle, server_addr, server_task)
}

/// Builds and starts a `Client` targeting `addr` with the given
/// connect timeout and max reconnect attempts.
///
/// The client driver runs in a background task; the returned
/// `ClientHandle` is the application-facing control surface.
pub async fn build_and_start_client(
    addr: SocketAddr,
    connect_timeout: Duration,
    max_reconnect_attempts: usize,
) -> (ClientHandle, JoinHandle<()>) {
    let (client, handle): (Client, ClientHandle) = ClientBuilder::new(addr)
        .connect_timeout(connect_timeout)
        .max_reconnect_attempts(max_reconnect_attempts)
        .build();
    let mut client = client;
    let client_task = tokio::spawn(async move {
        let _ = client.run().await;
    });
    (handle, client_task)
}

/// Builds a real SBE-framed message: `MessageHeader` + raw payload.
///
/// The header's `block_length` field is `u16` so payloads larger than
/// `u16::MAX` get a saturated length — that is fine for transport-level
/// framing tests where the SBE root-block size is not what's under test.
pub fn build_sbe_message(template_id: u16, payload: &[u8]) -> Vec<u8> {
    let header_size = MessageHeader::ENCODED_LENGTH;
    let mut frame = vec![0u8; header_size + payload.len()];
    let block_length = u16::try_from(payload.len()).unwrap_or(u16::MAX);
    let header = MessageHeader::new(block_length, template_id, 1, 1);
    header.encode(frame.as_mut_slice(), 0);
    frame[header_size..].copy_from_slice(payload);
    frame
}

/// Polls `handle.poll_events()` until `ServerEvent::Listening(addr)`
/// is observed or the deadline expires.
pub async fn wait_for_listening(
    handle: &Arc<ServerHandle>,
    deadline: Instant,
) -> Option<SocketAddr> {
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

/// Polls `handle.poll_events()` until `SessionCreated` is observed
/// or the deadline expires, returning the new session id.
pub async fn wait_for_session_created(
    handle: &Arc<ServerHandle>,
    deadline: Instant,
) -> Option<u64> {
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

/// Polls `handle.poll_events()` until `SessionClosed(expected)` is
/// observed or the deadline expires.
pub async fn wait_for_session_closed(
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

/// Waits for a `ClientEvent` matching `pred` or returns `None` on
/// deadline.
pub async fn wait_for_client_event<F>(
    handle: &mut ClientHandle,
    pred: F,
    deadline: Instant,
) -> Option<ClientEvent>
where
    F: Fn(&ClientEvent) -> bool,
{
    while Instant::now() < deadline {
        if let Some(event) = handle.poll()
            && pred(&event)
        {
            return Some(event);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    None
}

/// Waits specifically for `ClientEvent::Connected`.
pub async fn wait_for_client_connected(handle: &mut ClientHandle, deadline: Instant) -> bool {
    wait_for_client_event(handle, |e| matches!(e, ClientEvent::Connected), deadline)
        .await
        .is_some()
}

/// Waits for the next `ClientEvent::Message` and returns its payload.
pub async fn wait_for_client_message(
    handle: &mut ClientHandle,
    deadline: Instant,
) -> Option<Vec<u8>> {
    while Instant::now() < deadline {
        if let Some(ClientEvent::Message(bytes)) = handle.poll() {
            return Some(bytes);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    None
}
