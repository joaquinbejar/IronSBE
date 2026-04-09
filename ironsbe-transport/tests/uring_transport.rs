//! Linux-only integration tests for the io_uring TCP backend.
//!
//! These tests must run inside a [`tokio_uring::start`] block since the
//! backend's handle types are `!Send` and tied to a single-threaded runtime.

#![cfg(all(feature = "tcp-uring", target_os = "linux"))]

use bytes::Bytes;
use ironsbe_transport::tcp_uring::{UringClientConfig, UringServerConfig, UringTcpTransport};
use ironsbe_transport::traits::{LocalConnection, LocalListener, LocalTransport};
use std::net::SocketAddr;
use std::rc::Rc;

const PAYLOAD: &[u8] = b"hello io_uring world";

#[test]
fn test_uring_round_trip_send_borrowed() {
    tokio_uring::start(async {
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("valid addr");
        let server_cfg = UringServerConfig::new(bind_addr);
        let mut listener = UringTcpTransport::bind_with(server_cfg)
            .await
            .expect("bind");
        let listen_addr = listener.local_addr().expect("local_addr");

        // Spawn server-side accept loop on the same single-threaded runtime.
        let server_done = Rc::new(std::cell::Cell::new(false));
        let server_done_clone = Rc::clone(&server_done);
        let server_task = tokio_uring::spawn(async move {
            let mut conn = listener.accept().await.expect("accept");
            let received = conn
                .recv()
                .await
                .expect("server recv")
                .expect("frame available");
            assert_eq!(&received[..], PAYLOAD);
            conn.send(PAYLOAD).await.expect("server send");
            server_done_clone.set(true);
        });

        let client_cfg = UringClientConfig::new(listen_addr);
        let mut client = UringTcpTransport::connect_with(client_cfg)
            .await
            .expect("connect");
        client.send(PAYLOAD).await.expect("client send");
        let echoed = client
            .recv()
            .await
            .expect("client recv")
            .expect("frame available");
        assert_eq!(&echoed[..], PAYLOAD);

        server_task.await.expect("server task");
        assert!(server_done.get(), "server task should have completed");
    });
}

#[test]
fn test_uring_round_trip_send_owned() {
    tokio_uring::start(async {
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("valid addr");
        let mut listener = UringTcpTransport::bind_with(UringServerConfig::new(bind_addr))
            .await
            .expect("bind");
        let listen_addr = listener.local_addr().expect("local_addr");

        let server_task = tokio_uring::spawn(async move {
            let mut conn = listener.accept().await.expect("accept");
            let received = conn
                .recv()
                .await
                .expect("server recv")
                .expect("frame available");
            assert_eq!(&received[..], PAYLOAD);
            conn.send_owned(Bytes::from_static(PAYLOAD))
                .await
                .expect("server send_owned");
        });

        let mut client = UringTcpTransport::connect_with(UringClientConfig::new(listen_addr))
            .await
            .expect("connect");
        client
            .send_owned(Bytes::from_static(PAYLOAD))
            .await
            .expect("client send_owned");
        let echoed = client
            .recv()
            .await
            .expect("client recv")
            .expect("frame available");
        assert_eq!(&echoed[..], PAYLOAD);

        server_task.await.expect("server task");
    });
}

#[test]
fn test_uring_recv_partial_frames_assemble_correctly() {
    // A larger payload exercises the read loop path that pulls multiple
    // chunks before a complete frame is available.
    const LARGE_LEN: usize = 50 * 1024;

    tokio_uring::start(async {
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("valid addr");
        let server_cfg = UringServerConfig::new(bind_addr).max_frame_size(64 * 1024);
        let mut listener = UringTcpTransport::bind_with(server_cfg)
            .await
            .expect("bind");
        let listen_addr = listener.local_addr().expect("local_addr");

        let payload: Bytes = (0..LARGE_LEN).map(|i| (i % 251) as u8).collect();
        let payload_clone = payload.clone();

        let server_task = tokio_uring::spawn(async move {
            let mut conn = listener.accept().await.expect("accept");
            let received = conn
                .recv()
                .await
                .expect("server recv")
                .expect("frame available");
            assert_eq!(received.len(), LARGE_LEN);
            assert_eq!(&received[..], &payload_clone[..]);
        });

        let mut client = UringTcpTransport::connect_with(
            UringClientConfig::new(listen_addr).max_frame_size(64 * 1024),
        )
        .await
        .expect("connect");
        client.send_owned(payload).await.expect("client send_owned");

        server_task.await.expect("server task");
    });
}

#[test]
fn test_uring_send_oversized_frame_returns_err() {
    tokio_uring::start(async {
        let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("valid addr");
        let cfg = UringServerConfig::new(bind_addr).max_frame_size(1024);
        let mut listener = UringTcpTransport::bind_with(cfg).await.expect("bind");
        let listen_addr = listener.local_addr().expect("local_addr");

        let server_task = tokio_uring::spawn(async move {
            let _conn = listener.accept().await.expect("accept");
            // Hold the connection open while the client tries to send.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        });

        let mut client = UringTcpTransport::connect_with(
            UringClientConfig::new(listen_addr).max_frame_size(1024),
        )
        .await
        .expect("connect");
        let oversized = Bytes::from(vec![0u8; 4096]);
        let result = client.send_owned(oversized).await;
        assert!(result.is_err(), "oversized frame should be rejected");

        server_task.await.expect("server task");
    });
}
