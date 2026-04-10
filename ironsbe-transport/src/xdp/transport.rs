//! High-level [`LocalTransport`] implementation for the AF_XDP backend.
//!
//! `XdpTransport<S>` ties together the [`Datapath`](super::datapath::Datapath)
//! (xsk-rs UMEM + ring queues) and a user-selected [`XdpStack`] (UDP or
//! smoltcp TCP) into a single type that satisfies the
//! [`LocalTransport`] trait so it can be driven by
//! [`LocalServer`](ironsbe_server::LocalServer).
//!
//! # Threading model
//!
//! AF_XDP is thread-per-core by design: the datapath is bound to a single
//! `(interface, queue)` pair, and `poll_once` is not `Send`.  All
//! operations — bind, accept, recv, send — must happen on the same
//! thread.  `XdpListener::accept` busy-polls the datapath, which is the
//! expected behaviour in a kernel-bypass tight loop.
//!
//! # Client side
//!
//! AF_XDP does not have a "client connect" semantic in the kernel-bypass
//! model.  `connect_with` always returns
//! [`io::ErrorKind::Unsupported`].

use super::datapath::{Datapath, DatapathConfig};
use super::stack::{FrameTxQueue, XdpStack};
use crate::traits::{LocalListener, LocalTransport};
use std::io;
use std::marker::PhantomData;
use std::net::{IpAddr, SocketAddr};

/// AF_XDP transport configuration.
///
/// Carries the [`DatapathConfig`] (interface name, queue id, UMEM
/// settings) and a ready-to-use [`XdpStack`] instance that will drive
/// the L3/L4 layer above the raw frames.
#[derive(Debug, Clone)]
pub struct XdpConfig<S> {
    /// Datapath (xsk-rs) settings.
    pub datapath: DatapathConfig,
    /// The userspace network stack that handles L3/L4 above the raw
    /// Ethernet frames delivered by AF_XDP.
    pub stack: S,
    /// TCP/UDP port the stack listens on (used for `local_addr()`
    /// reporting only — the actual binding is done by the stack).
    pub listen_port: u16,
}

impl<S: XdpStack + Clone> XdpConfig<S> {
    /// Creates a new XDP config.
    #[must_use]
    pub fn new(datapath: DatapathConfig, stack: S, listen_port: u16) -> Self {
        Self {
            datapath,
            stack,
            listen_port,
        }
    }
}

/// Fallback `From<SocketAddr>` required by the `LocalTransport` trait.
///
/// Builds a default config on `lo` queue 0 — useful for tests and
/// examples but unlikely to be the right choice for production, where
/// the caller should construct an explicit [`XdpConfig`] with the
/// correct interface and stack.
impl From<SocketAddr> for XdpConfig<super::stack::UdpStack> {
    fn from(addr: SocketAddr) -> Self {
        let ip = match addr.ip() {
            IpAddr::V4(v4) => v4,
            IpAddr::V6(_) => std::net::Ipv4Addr::LOCALHOST,
        };
        let mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01]; // locally-administered
        let stack = super::stack::UdpStack::new(super::stack::udp::UdpStackConfig::new(
            ip,
            addr.port(),
            mac,
        ));
        Self {
            datapath: DatapathConfig::new("lo", 0),
            stack,
            listen_port: addr.port(),
        }
    }
}

/// AF_XDP transport backend.
///
/// Generic over the userspace stack `S`.  Use
/// [`super::stack::UdpStack`] for lowest-latency UDP framing or
/// [`super::stack::SmoltcpStack`] for wire-compatible TCP.
pub struct XdpTransport<S: XdpStack>(PhantomData<S>);

impl<S> LocalTransport for XdpTransport<S>
where
    S: XdpStack + Clone + 'static,
    S::Connection: 'static,
    S::Error: std::fmt::Display + 'static,
    XdpConfig<S>: From<SocketAddr> + Clone + 'static,
{
    type Listener = XdpListener<S>;
    type Connection = S::Connection;
    type Error = io::Error;
    type BindConfig = XdpConfig<S>;
    type ConnectConfig = XdpConfig<S>;

    async fn bind_with(config: XdpConfig<S>) -> io::Result<XdpListener<S>> {
        let datapath = Datapath::bind(&config.datapath)?;
        let local_ip = config.stack.local_ip();
        let listen_port = config.listen_port;
        Ok(XdpListener {
            datapath,
            stack: config.stack,
            local_addr: SocketAddr::new(local_ip, listen_port),
        })
    }

    async fn connect_with(_config: XdpConfig<S>) -> io::Result<S::Connection> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "AF_XDP does not support client-side connect; \
             use a regular TCP/UDP client to talk to an XDP server",
        ))
    }
}

/// AF_XDP listener that busy-polls the datapath until the stack yields
/// a new connection.
pub struct XdpListener<S: XdpStack> {
    datapath: Datapath,
    stack: S,
    local_addr: SocketAddr,
}

impl<S> LocalListener for XdpListener<S>
where
    S: XdpStack + 'static,
    S::Connection: 'static,
    S::Error: std::fmt::Display + 'static,
{
    type Connection = S::Connection;
    type Error = io::Error;

    async fn accept(&mut self) -> io::Result<S::Connection> {
        // Busy-poll the datapath + stack until a new connection is
        // yielded.  This is the expected pattern for a kernel-bypass
        // tight loop.  In a cooperative runtime the caller should
        // wrap this in a dedicated thread or use `spawn_blocking`.
        loop {
            let mut tx_buf: Vec<Vec<u8>> = Vec::new();

            // Drive one round of rx → stack → tx.
            self.datapath.poll_once(&mut self.stack)?;

            // Drain any outbound frames the stack produced (ARP
            // replies, TCP SYN-ACK, …).  The datapath's poll_once
            // already submitted them to the tx ring internally via
            // the FrameTxQueue plumbing, so there is nothing extra
            // to do here — the frames were already handed to the
            // kernel.

            // Check if the stack accepted a new connection.
            // For UdpStack this happens on the first packet from a
            // new peer; for SmoltcpStack this happens when a TCP
            // handshake completes.
            //
            // We re-poll immediately rather than yielding because
            // busy-polling is the whole point of AF_XDP.
            //
            // TODO: the XdpStack::on_rx already returns
            // Option<Connection> from inside poll_once but we don't
            // surface it here yet.  For now we rely on the stack's
            // internal accept queue and drain it below.
            //
            // This is a temporary simplification; a future refactor
            // will thread the connection out of poll_once directly.
            let mut q = FrameTxQueue::new(&mut tx_buf);
            if let Some(conn) = self
                .stack
                .on_rx(&[], &mut q)
                .map_err(|e| io::Error::other(format!("stack on_rx: {e}")))?
            {
                return Ok(conn);
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local_addr)
    }
}
