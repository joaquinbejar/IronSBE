//! Example IronSBE server running on the Linux io_uring backend.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example uring_server \
//!     --features ironsbe-server/tcp-uring,ironsbe-transport/tcp-uring
//! ```
//!
//! On non-Linux platforms (or without the `tcp-uring` feature) the binary
//! still compiles but `main` exits early with a clear message — that
//! keeps `cargo build --examples` working everywhere.

#[cfg(all(feature = "tcp-uring", target_os = "linux"))]
mod imp {
    use ironsbe_core::header::MessageHeader;
    use ironsbe_server::{LocalServerBuilder, MessageHandler, Responder, ServerError};
    use ironsbe_transport::tcp_uring::UringTcpTransport;
    use std::net::SocketAddr;

    /// Echoes every received SBE message straight back to the sender.
    pub(crate) struct EchoHandler;

    impl MessageHandler for EchoHandler {
        fn on_message(
            &self,
            session_id: u64,
            _header: &MessageHeader,
            buffer: &[u8],
            responder: &dyn Responder,
        ) {
            if let Err(e) = responder.send(buffer) {
                eprintln!("[uring server] session {session_id} echo failed: {e:?}");
            }
        }

        fn on_session_start(&self, session_id: u64) {
            println!("[uring server] session {session_id} connected");
        }

        fn on_session_end(&self, session_id: u64) {
            println!("[uring server] session {session_id} disconnected");
        }
    }

    pub(crate) fn run() -> Result<(), ServerError> {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();

        let addr: SocketAddr = "127.0.0.1:9000"
            .parse()
            .expect("hardcoded example addr is valid");

        let (mut server, _handle) = LocalServerBuilder::<EchoHandler, UringTcpTransport>::new()
            .bind(addr)
            .handler(EchoHandler)
            .max_connections(64)
            .build();

        println!("Starting IronSBE uring server on {addr}");
        println!("Press Ctrl+C to stop");

        // `tokio_uring::start` installs a single-threaded reactor with a
        // `LocalSet` so the server's `spawn_local`-driven session loop
        // can run alongside `!Send` uring connections.
        tokio_uring::start(async move { server.run().await })?;
        Ok(())
    }
}

#[cfg(all(feature = "tcp-uring", target_os = "linux"))]
fn main() -> Result<(), ironsbe_server::ServerError> {
    imp::run()
}

#[cfg(not(all(feature = "tcp-uring", target_os = "linux")))]
fn main() {
    eprintln!(
        "uring_server example requires --features tcp-uring on Linux \
         (kernel >= 5.10).  This build does not have it enabled."
    );
}
