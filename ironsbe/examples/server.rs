//! Example SBE server demonstrating basic message handling.
//!
//! Run with: `cargo run --example server`

use ironsbe_core::header::MessageHeader;
use ironsbe_server::builder::ServerBuilder;
use ironsbe_server::handler::{MessageHandler, Responder};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Simple echo handler that logs received messages and echoes them back.
struct EchoHandler {
    message_count: AtomicU64,
}

impl EchoHandler {
    fn new() -> Self {
        Self {
            message_count: AtomicU64::new(0),
        }
    }
}

impl MessageHandler for EchoHandler {
    fn on_message(
        &self,
        session_id: u64,
        header: &MessageHeader,
        buffer: &[u8],
        responder: &dyn Responder,
    ) {
        let count = self.message_count.fetch_add(1, Ordering::Relaxed) + 1;
        let template_id = { header.template_id };

        println!(
            "[Server] Message #{} from session {}: template_id={}, size={} bytes",
            count,
            session_id,
            template_id,
            buffer.len()
        );

        // Echo the message back to the client
        if let Err(e) = responder.send(buffer) {
            eprintln!("[Server] Failed to send response: {:?}", e);
        }
    }

    fn on_session_start(&self, session_id: u64) {
        println!("[Server] Session {} connected", session_id);
    }

    fn on_session_end(&self, session_id: u64) {
        println!("[Server] Session {} disconnected", session_id);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let addr: SocketAddr = "127.0.0.1:9000".parse()?;
    let handler = EchoHandler::new();

    println!("Starting IronSBE server on {}", addr);
    println!("Press Ctrl+C to stop");

    let (mut server, handle) = ServerBuilder::new()
        .bind(addr)
        .handler(handler)
        .max_connections(100)
        .build();

    // Wrap handle in Arc for sharing across tasks
    let handle = Arc::new(handle);
    let shutdown_handle = Arc::clone(&handle);

    // Spawn a task to handle shutdown
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        println!("\nShutting down server...");
        shutdown_handle.shutdown();
    });

    // Run the server
    server.run().await?;

    println!("Server stopped");
    Ok(())
}
