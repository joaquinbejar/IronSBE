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

use ironsbe_core::buffer::{AlignedBuffer, ReadBuffer, WriteBuffer};
use ironsbe_core::header::MessageHeader;
use std::net::UdpSocket;
use std::time::Duration;

/// Builds a valid SBE-framed message: `MessageHeader` + payload.
fn create_sbe_message(template_id: u16, payload: &[u8]) -> Vec<u8> {
    let mut buffer = AlignedBuffer::<256>::new();
    let header = MessageHeader::new(payload.len() as u16, template_id, 1, 1);
    header.encode(&mut buffer, 0);
    let header_size = MessageHeader::ENCODED_LENGTH;
    buffer.as_mut_slice()[header_size..header_size + payload.len()].copy_from_slice(payload);
    buffer.as_slice()[..header_size + payload.len()].to_vec()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server_addr = "10.0.0.1:9000";
    println!("[xdp client] Sending to {server_addr} via regular UDP");

    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(Duration::from_secs(2)))?;

    for i in 0..5u32 {
        // Build a length-prefixed SBE message (matches UdpStack's wire
        // format: 4-byte LE length + SBE message).
        let sbe_msg = create_sbe_message(1, format!("hello-{i}").as_bytes());
        let frame_len = sbe_msg.len() as u32;
        let mut datagram = Vec::with_capacity(4 + sbe_msg.len());
        datagram.extend_from_slice(&frame_len.to_le_bytes());
        datagram.extend_from_slice(&sbe_msg);

        sock.send_to(&datagram, server_addr)?;
        println!("[xdp client] sent message #{i} ({} bytes)", sbe_msg.len());

        let mut buf = [0u8; 2048];
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                println!("[xdp client] echo from {from}: {n} bytes");
            }
            Err(e) => {
                eprintln!("[xdp client] recv timeout or error: {e}");
            }
        }
    }

    Ok(())
}
