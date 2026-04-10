//! Example IronSBE server running on the AF_XDP backend with UdpStack.
//!
//! Run with (requires root / `CAP_NET_ADMIN` + `CAP_BPF`):
//!
//! ```sh
//! sudo cargo run -p ironsbe --example xdp_server --features xdp
//! ```
//!
//! On non-Linux platforms (or without the `xdp` feature) the binary
//! still compiles but `main` exits early with a clear message.

#[cfg(all(feature = "xdp", target_os = "linux"))]
mod imp {
    use ironsbe_core::header::MessageHeader;
    use ironsbe_server::{LocalServerBuilder, MessageHandler, Responder, ServerError};
    use ironsbe_transport::xdp::{
        DatapathConfig, UdpStack, XdpConfig, XdpTransport,
    };
    use ironsbe_transport::xdp::stack::udp::UdpStackConfig;
    use std::net::Ipv4Addr;

    struct EchoHandler;

    impl MessageHandler for EchoHandler {
        fn on_message(
            &self,
            session_id: u64,
            _header: &MessageHeader,
            buffer: &[u8],
            responder: &dyn Responder,
        ) {
            if let Err(e) = responder.send(buffer) {
                eprintln!("[xdp server] session {session_id} echo failed: {e:?}");
            }
        }

        fn on_session_start(&self, session_id: u64) {
            println!("[xdp server] session {session_id} connected");
        }

        fn on_session_end(&self, session_id: u64) {
            println!("[xdp server] session {session_id} disconnected");
        }
    }

    pub(crate) fn run() -> Result<(), ServerError> {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();

        let local_mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
        let local_ip = Ipv4Addr::new(10, 0, 0, 1);
        let local_port = 9000u16;

        let stack = UdpStack::new(UdpStackConfig::new(local_ip, local_port, local_mac));
        let datapath_cfg = DatapathConfig::new("eth0", 0);
        let xdp_cfg = XdpConfig::new(datapath_cfg, stack, local_port);

        let (mut server, _handle) =
            LocalServerBuilder::<EchoHandler, XdpTransport<UdpStack>>::new()
                .bind_config(xdp_cfg)
                .handler(EchoHandler)
                .max_connections(64)
                .build();

        println!("[xdp server] Starting on eth0 queue 0, UDP port {local_port}");
        println!("[xdp server] Press Ctrl+C to stop");
        println!("[xdp server] NOTE: requires sudo / CAP_NET_ADMIN + CAP_BPF");

        // AF_XDP is single-threaded; we drive the server in a blocking
        // loop on the current thread.  tokio_uring or a plain LocalSet
        // provides the reactor.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move { server.run().await })?;
        Ok(())
    }
}

#[cfg(all(feature = "xdp", target_os = "linux"))]
fn main() -> Result<(), ironsbe_server::ServerError> {
    imp::run()
}

#[cfg(not(all(feature = "xdp", target_os = "linux")))]
fn main() {
    eprintln!(
        "xdp_server example requires --features xdp on Linux \
         (kernel >= 5.11).  This build does not have it enabled."
    );
}
