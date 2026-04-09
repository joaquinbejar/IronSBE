//! Integration tests for backend-specific transport configuration.
//!
//! Validates that `Transport::bind_with` / `connect_with` honour caller-supplied
//! tunables end-to-end.  Round-trips a payload larger than the default 64 KiB
//! frame after raising `max_frame_size` on both sides.

#![cfg(feature = "tcp-tokio")]

use ironsbe_transport::tcp::{TcpClientConfig, TcpServerConfig, TokioTcpTransport};
use ironsbe_transport::traits::Transport;
use std::net::SocketAddr;

const LARGE_FRAME: usize = 256 * 1024;
const PAYLOAD_LEN: usize = 100 * 1024;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_round_trip_with_custom_frame_size() {
    let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("valid addr");

    let server_cfg = TcpServerConfig::new(bind_addr).max_frame_size(LARGE_FRAME);
    let mut listener = TokioTcpTransport::bind_with(server_cfg)
        .await
        .expect("bind");
    let listen_addr = listener.local_addr().expect("local_addr");

    let payload: Vec<u8> = (0..PAYLOAD_LEN).map(|i| (i % 251) as u8).collect();
    let payload_clone = payload.clone();

    let server_task = tokio::spawn(async move {
        let mut conn = listener.accept().await.expect("accept");
        let received = conn
            .recv()
            .await
            .expect("server recv")
            .expect("framed message");
        assert_eq!(received.len(), PAYLOAD_LEN);
        assert_eq!(&received[..], &payload_clone[..]);
        conn.send(&payload_clone).await.expect("server send");
    });

    let client_cfg = TcpClientConfig::new(listen_addr).max_frame_size(LARGE_FRAME);
    let mut client = TokioTcpTransport::connect_with(client_cfg)
        .await
        .expect("connect");
    client.send(&payload).await.expect("client send");
    let echoed = client
        .recv()
        .await
        .expect("client recv")
        .expect("framed message");
    assert_eq!(echoed.len(), PAYLOAD_LEN);
    assert_eq!(&echoed[..], &payload[..]);

    server_task.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_default_bind_and_connect_still_work() {
    // Default tunables: ensure the From<SocketAddr> path is exercised.
    let bind_addr: SocketAddr = "127.0.0.1:0".parse().expect("valid addr");
    let mut listener = TokioTcpTransport::bind(bind_addr).await.expect("bind");
    let listen_addr = listener.local_addr().expect("local_addr");

    let server_task = tokio::spawn(async move {
        let mut conn = listener.accept().await.expect("accept");
        let msg = conn.recv().await.expect("recv").expect("frame");
        assert_eq!(&msg[..], b"hello");
        conn.send(b"world").await.expect("send");
    });

    let mut client = TokioTcpTransport::connect(listen_addr)
        .await
        .expect("connect");
    client.send(b"hello").await.expect("client send");
    let reply = client.recv().await.expect("client recv").expect("frame");
    assert_eq!(&reply[..], b"world");

    server_task.await.expect("server task");
}

#[test]
fn test_tcp_server_config_from_socket_addr() {
    let addr: SocketAddr = "127.0.0.1:9001".parse().expect("valid addr");
    let cfg: TcpServerConfig = addr.into();
    assert_eq!(cfg.bind_addr, addr);
    assert_eq!(cfg.max_frame_size, 64 * 1024);
    assert!(cfg.tcp_nodelay);
}

#[test]
fn test_tcp_client_config_from_socket_addr() {
    let addr: SocketAddr = "127.0.0.1:9002".parse().expect("valid addr");
    let cfg: TcpClientConfig = addr.into();
    assert_eq!(cfg.server_addr, addr);
    assert_eq!(cfg.max_frame_size, 64 * 1024);
    assert!(cfg.tcp_nodelay);
}
