//! Linux-only end-to-end test for the [`LocalServer`] / [`LocalClient`]
//! types running on top of the io_uring backend.
//!
//! Drives the whole stack inside a single `tokio_uring::start` block:
//!
//! 1. Build a [`LocalServer`] with an echo handler.
//! 2. Spawn it on the local set with `spawn_local`.
//! 3. Build a [`LocalClient`] targeting the bound port and spawn its
//!    `run()` loop.
//! 4. Send an SBE message via the [`ClientHandle`], wait for the echo,
//!    assert payload equality.
//! 5. Tear everything down cleanly.

#![cfg(all(feature = "tcp-uring", target_os = "linux"))]

use ironsbe_client::{ClientEvent, LocalClientBuilder};
use ironsbe_core::buffer::{AlignedBuffer, ReadBuffer, WriteBuffer};
use ironsbe_core::header::MessageHeader;
use ironsbe_server::{LocalServerBuilder, MessageHandler, Responder, ServerEvent};
use ironsbe_transport::tcp_uring::{UringServerConfig, UringTcpTransport};
use std::net::SocketAddr;
use std::time::Duration;

const PAYLOAD: &[u8] = b"hello-uring-server";
const TEMPLATE_ID: u16 = 42;

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

/// Builds a real SBE message: encoded `MessageHeader` + payload.  We
/// validate the full encode/decode path rather than handing the server
/// an all-zero header.
fn build_sbe_message() -> Vec<u8> {
    let mut buffer = AlignedBuffer::<128>::new();
    let header = MessageHeader::new(PAYLOAD.len() as u16, TEMPLATE_ID, 1, 1);
    header.encode(&mut buffer, 0);
    let header_size = MessageHeader::ENCODED_LENGTH;
    buffer.as_mut_slice()[header_size..header_size + PAYLOAD.len()].copy_from_slice(PAYLOAD);
    buffer.as_slice()[..header_size + PAYLOAD.len()].to_vec()
}

#[test]
fn test_local_server_local_client_round_trip_uring() {
    tokio_uring::start(async {
        // Bind to port 0 and let the server tell us the effective addr
        // through the ServerEvent::Listening event — no probe-port race.
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
        let server_cfg = UringServerConfig::new(bind_addr);
        let (mut server, server_handle) =
            LocalServerBuilder::<EchoHandler, UringTcpTransport>::new()
                .bind_config(server_cfg)
                .handler(EchoHandler)
                .build();

        // Run the server on the local set.
        let _server_task = tokio::task::spawn_local(async move {
            let _ = server.run().await;
        });

        // Wait for ServerEvent::Listening so we know the effective addr.
        let listen_deadline = std::time::Instant::now() + Duration::from_secs(2);
        let server_addr = loop {
            assert!(
                std::time::Instant::now() < listen_deadline,
                "server did not emit Listening within 2 s"
            );
            if let Some(ServerEvent::Listening(addr)) = server_handle.poll_events().next() {
                break addr;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        // Build the client and run its driver.
        let (client, mut client_handle) = LocalClientBuilder::<UringTcpTransport>::new(server_addr)
            .connect_timeout(Duration::from_secs(2))
            .max_reconnect_attempts(2)
            .build();
        let mut client = client;
        let _client_task = tokio::task::spawn_local(async move {
            let _ = client.run().await;
        });

        // Wait for connection establishment via the Connected event.
        let connect_deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            assert!(
                std::time::Instant::now() < connect_deadline,
                "client never connected"
            );
            if let Some(ClientEvent::Connected) = client_handle.poll() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Build a real SBE message and send it.
        let framed = build_sbe_message();
        client_handle.send(framed.clone()).expect("send");

        // Drain events until we see the echo (or time out).
        let echo_deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut got_echo = false;
        while std::time::Instant::now() < echo_deadline {
            while let Some(event) = client_handle.poll() {
                if let ClientEvent::Message(bytes) = event
                    && bytes == framed
                {
                    got_echo = true;
                    break;
                }
            }
            if got_echo {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(got_echo, "did not receive echo within deadline");

        client_handle.disconnect();
    });
}
