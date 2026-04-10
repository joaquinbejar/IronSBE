//! Example client that talks to an XDP server via a regular UDP socket.
//!
//! AF_XDP does not have a "client connect" concept — the kernel-bypass
//! path is for the **server** side only.  Clients just use a normal
//! UDP (or TCP, if the server runs SmoltcpStack) socket.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p ironsbe --example xdp_client
//! ```
//!
//! Make sure the `xdp_server` example is running first.

use std::net::UdpSocket;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server_addr = "10.0.0.1:9000";
    println!("[xdp client] Sending to {server_addr} via regular UDP");

    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(Duration::from_secs(2)))?;

    for i in 0..5u32 {
        // Build a length-prefixed SBE message (matches UdpStack's wire
        // format: 4-byte LE length + payload).
        let payload = format!("hello-{i}");
        let frame_len = payload.len() as u32;
        let mut datagram = Vec::with_capacity(4 + payload.len());
        datagram.extend_from_slice(&frame_len.to_le_bytes());
        datagram.extend_from_slice(payload.as_bytes());

        sock.send_to(&datagram, server_addr)?;
        println!("[xdp client] sent: {payload}");

        let mut buf = [0u8; 2048];
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                println!("[xdp client] echo from {from}: {} bytes", n);
            }
            Err(e) => {
                eprintln!("[xdp client] recv timeout or error: {e}");
            }
        }
    }

    Ok(())
}
