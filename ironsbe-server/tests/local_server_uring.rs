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
use ironsbe_core::header::MessageHeader;
use ironsbe_server::{LocalServerBuilder, MessageHandler, Responder};
use ironsbe_transport::tcp_uring::{UringServerConfig, UringTcpTransport};
use std::net::SocketAddr;
use std::time::Duration;

const PAYLOAD: &[u8] = b"hello-uring-server";

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

#[test]
fn test_local_server_local_client_round_trip_uring() {
    tokio_uring::start(async {
        // The LocalServer takes ownership of the bind step and does not
        // currently expose the bound socket back to the caller, so we
        // pre-probe an ephemeral port via a stdlib listener and re-use
        // it for the server.  There's a small race window but for a
        // single-threaded test on loopback it is acceptable.
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
        let port = probe.local_addr().expect("probe addr").port();
        drop(probe);

        let server_addr: SocketAddr = format!("127.0.0.1:{port}").parse().expect("addr");
        let server_cfg = UringServerConfig::new(server_addr);
        let (mut server, _server_handle) =
            LocalServerBuilder::<EchoHandler, UringTcpTransport>::new()
                .bind_config(server_cfg)
                .handler(EchoHandler)
                .build();

        // 2. Run the server on the local set.
        let _server_task = tokio::task::spawn_local(async move {
            let _ = server.run().await;
        });

        // Give the server a moment to bind before connecting.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 3. Build the client and run its driver.
        let (mut client, mut client_handle) =
            LocalClientBuilder::<UringTcpTransport>::new(server_addr)
                .connect_timeout(Duration::from_secs(2))
                .max_reconnect_attempts(2)
                .build();
        let _client_task = tokio::task::spawn_local(async move {
            let _ = client.run().await;
        });

        // Wait for connection establishment.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 4. Build a tiny SBE message and send it.
        let mut framed = Vec::with_capacity(MessageHeader::ENCODED_LENGTH + PAYLOAD.len());
        framed.extend_from_slice(&[0u8; MessageHeader::ENCODED_LENGTH]);
        framed.extend_from_slice(PAYLOAD);
        client_handle.send(framed.clone()).expect("send");

        // 5. Drain events until we see the echo (or time out).
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut got_echo = false;
        while std::time::Instant::now() < deadline {
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
