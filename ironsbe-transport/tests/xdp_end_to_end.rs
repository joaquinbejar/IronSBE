//! End-to-end AF_XDP integration test.
//!
//! This test is `#[ignore]` by default because it requires:
//!
//! - Linux kernel ≥ 5.11
//! - `CAP_NET_ADMIN` + `CAP_BPF` (or root)
//! - A NIC / veth with XDP support
//! - Build deps for `libbpf-sys` (`libelf-dev`, `clang`, etc.)
//!
//! To run manually:
//!
//! ```sh
//! sudo -E cargo test -p ironsbe-transport --test xdp_end_to_end \
//!     --features xdp -- --ignored --nocapture
//! ```
//!
//! The test binds an AF_XDP socket on `lo` queue 0 in copy mode and
//! validates that the `UdpStack` can receive a length-prefixed frame
//! sent from a regular UDP socket.

#![cfg(all(feature = "xdp", target_os = "linux"))]

use ironsbe_transport::traits::{LocalListener, LocalTransport};
use ironsbe_transport::xdp::stack::udp::UdpStackConfig;
use ironsbe_transport::xdp::{DatapathConfig, UdpStack, XdpConfig, XdpTransport};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

const LOCAL_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
const LOCAL_IP: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
const LOCAL_PORT: u16 = 19000;
const PAYLOAD: &[u8] = b"hello-xdp";

#[test]
#[ignore = "requires CAP_NET_ADMIN + AF_XDP-capable interface (run with sudo)"]
fn test_xdp_udp_stack_receives_frame_from_regular_socket() {
    let stack = UdpStack::new(UdpStackConfig::new(LOCAL_IP, LOCAL_PORT, LOCAL_MAC));
    let datapath_cfg = DatapathConfig::new("lo", 0);
    let xdp_cfg = XdpConfig::new(datapath_cfg, stack, LOCAL_PORT);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    let local = tokio::task::LocalSet::new();

    local.block_on(&rt, async {
        let mut listener = XdpTransport::<UdpStack>::bind_with(xdp_cfg)
            .await
            .expect("bind AF_XDP on lo");

        // Send a length-prefixed datagram from a regular UDP socket.
        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
        let frame_len = PAYLOAD.len() as u32;
        let mut datagram = Vec::with_capacity(4 + PAYLOAD.len());
        datagram.extend_from_slice(&frame_len.to_le_bytes());
        datagram.extend_from_slice(PAYLOAD);
        sender
            .send_to(&datagram, format!("127.0.0.1:{LOCAL_PORT}"))
            .expect("send");

        // Accept the connection produced by UdpStack::on_rx.
        let conn = tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
            .await
            .expect("accept timeout")
            .expect("accept error");

        // The first recv should yield the payload we sent.
        use ironsbe_transport::traits::LocalConnection;
        let mut conn = conn;
        let msg = conn
            .recv()
            .await
            .expect("recv")
            .expect("frame should be available");
        assert_eq!(&msg[..], PAYLOAD);
    });
}
