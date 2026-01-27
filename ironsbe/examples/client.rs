//! Example SBE client demonstrating basic message sending.
//!
//! Run with: `cargo run --example client`
//!
//! Make sure the server is running first: `cargo run --example server`

use ironsbe_client::builder::ClientBuilder;
use ironsbe_client::builder::ClientEvent;
use ironsbe_core::buffer::{AlignedBuffer, ReadBuffer, WriteBuffer};
use ironsbe_core::header::MessageHeader;
use std::net::SocketAddr;
use std::time::Duration;

/// Creates a simple SBE message with a header and payload.
fn create_message(template_id: u16, payload: &[u8]) -> Vec<u8> {
    let mut buffer = AlignedBuffer::<256>::new();

    // Create and encode the message header
    let header = MessageHeader::new(
        payload.len() as u16, // block_length
        template_id,          // template_id
        1,                    // schema_id
        1,                    // version
    );
    header.encode(&mut buffer, 0);

    // Copy payload after header
    let header_size = MessageHeader::ENCODED_LENGTH;
    buffer.as_mut_slice()[header_size..header_size + payload.len()].copy_from_slice(payload);

    // Return the complete message
    buffer.as_slice()[..header_size + payload.len()].to_vec()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let addr: SocketAddr = "127.0.0.1:9000".parse()?;

    println!("Connecting to IronSBE server at {}", addr);

    let (mut client, mut handle) = ClientBuilder::new(addr)
        .connect_timeout(Duration::from_secs(5))
        .max_reconnect_attempts(3)
        .build();

    // Spawn the client connection task
    let client_task = tokio::spawn(async move {
        if let Err(e) = client.run().await {
            eprintln!("[Client] Error: {:?}", e);
        }
    });

    // Wait for connection
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send some test messages
    println!("\nSending test messages...\n");

    for i in 1..=5 {
        let payload = format!("Hello from IronSBE client! Message #{}", i);
        let message = create_message(100 + i as u16, payload.as_bytes());

        match handle.send(message) {
            Ok(()) => println!("[Client] Sent message #{}", i),
            Err(e) => eprintln!("[Client] Failed to send message #{}: {:?}", i, e),
        }

        // Small delay between messages
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Check for responses
        while let Some(event) = handle.poll() {
            match event {
                ClientEvent::Connected => {
                    println!("[Client] Connected to server");
                }
                ClientEvent::Disconnected => {
                    println!("[Client] Disconnected from server");
                }
                ClientEvent::Message(data) => {
                    println!(
                        "[Client] Received response: {} bytes",
                        data.len()
                    );
                    // Try to decode the payload
                    if data.len() > MessageHeader::ENCODED_LENGTH {
                        let payload = &data[MessageHeader::ENCODED_LENGTH..];
                        if let Ok(text) = std::str::from_utf8(payload) {
                            println!("[Client] Response payload: {}", text);
                        }
                    }
                }
                ClientEvent::Error(e) => {
                    eprintln!("[Client] Error: {}", e);
                }
            }
        }
    }

    // Wait a bit for final responses
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Drain remaining events
    while let Some(event) = handle.poll() {
        if let ClientEvent::Message(data) = event {
            println!("[Client] Received response: {} bytes", data.len());
            if data.len() > MessageHeader::ENCODED_LENGTH {
                let payload = &data[MessageHeader::ENCODED_LENGTH..];
                if let Ok(text) = std::str::from_utf8(payload) {
                    println!("[Client] Response payload: {}", text);
                }
            }
        }
    }

    // Disconnect
    println!("\nDisconnecting...");
    handle.disconnect();

    // Wait for client task to finish
    tokio::time::timeout(Duration::from_secs(2), client_task).await.ok();

    println!("Client stopped");
    Ok(())
}
