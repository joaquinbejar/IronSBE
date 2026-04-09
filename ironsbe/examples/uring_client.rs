//! Example IronSBE client running on the Linux io_uring backend.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p ironsbe --example uring_client --features tcp-uring
//! ```
//!
//! Pair with `cargo run -p ironsbe --example uring_server --features tcp-uring`.
//!
//! On non-Linux platforms (or without the `tcp-uring` feature) the binary
//! still compiles but `main` exits early with a clear message.

#[cfg(all(feature = "tcp-uring", target_os = "linux"))]
mod imp {
    use ironsbe_client::{ClientEvent, LocalClientBuilder};
    use ironsbe_core::buffer::{AlignedBuffer, ReadBuffer, WriteBuffer};
    use ironsbe_core::header::MessageHeader;
    use ironsbe_transport::tcp_uring::UringTcpTransport;
    use std::net::SocketAddr;
    use std::time::Duration;

    /// Builds a tiny SBE message with the given template ID and payload.
    fn create_message(template_id: u16, payload: &[u8]) -> Vec<u8> {
        let mut buffer = AlignedBuffer::<256>::new();
        let header = MessageHeader::new(payload.len() as u16, template_id, 1, 1);
        header.encode(&mut buffer, 0);
        let header_size = MessageHeader::ENCODED_LENGTH;
        buffer.as_mut_slice()[header_size..header_size + payload.len()].copy_from_slice(payload);
        buffer.as_slice()[..header_size + payload.len()].to_vec()
    }

    pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();

        let addr: SocketAddr = "127.0.0.1:9000".parse()?;
        println!("Connecting IronSBE uring client to {addr}");

        let (client, mut handle) = LocalClientBuilder::<UringTcpTransport>::new(addr)
            .connect_timeout(Duration::from_secs(5))
            .max_reconnect_attempts(3)
            .build();
        let mut client = client;

        // The whole client lives inside the single-threaded uring reactor.
        tokio_uring::start(async move {
            // Spawn the client driver on the local task set.
            let driver = tokio::task::spawn_local(async move {
                if let Err(e) = client.run().await {
                    eprintln!("[uring client] driver error: {e:?}");
                }
            });

            // Give the driver a moment to connect.
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Send a few messages and read the echoes back.
            for i in 0..5u32 {
                let payload = format!("hello-{i}");
                let msg = create_message(1, payload.as_bytes());
                if let Err(e) = handle.send(msg) {
                    eprintln!("[uring client] send #{i} failed: {e:?}");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                while let Some(event) = handle.poll() {
                    match event {
                        ClientEvent::Message(bytes) => {
                            println!("[uring client] echo received ({} bytes)", bytes.len());
                        }
                        ClientEvent::Connected => println!("[uring client] connected"),
                        ClientEvent::Disconnected => {
                            println!("[uring client] disconnected");
                        }
                        ClientEvent::Error(msg) => {
                            eprintln!("[uring client] error event: {msg}");
                        }
                    }
                }
            }

            handle.disconnect();
            let _ = driver.await;
        });

        Ok(())
    }
}

#[cfg(all(feature = "tcp-uring", target_os = "linux"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    imp::run()
}

#[cfg(not(all(feature = "tcp-uring", target_os = "linux")))]
fn main() {
    eprintln!(
        "uring_client example requires --features tcp-uring on Linux \
         (kernel >= 5.10).  This build does not have it enabled."
    );
}
