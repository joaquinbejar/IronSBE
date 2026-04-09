//! Trait abstraction over the L3/L4 layer that sits above the AF_XDP
//! datapath.
//!
//! Two implementations are shipped:
//!
//! - [`udp::UdpStack`] — minimal length-prefix framing on top of UDP/IPv4.
//!   Lowest latency, lowest complexity, but loses wire-compatibility with
//!   the TCP backends.
//! - [`tcp::SmoltcpStack`] — userspace TCP via `smoltcp`.  Wire-compatible
//!   with `tcp-tokio` / `tcp-uring`, at the cost of higher complexity and
//!   per-connection state.
//!
//! Backends pick one of the two via [`crate::xdp::config::XdpServerConfig`].

use crate::traits::LocalConnection;
use std::net::IpAddr;

pub mod tcp;
pub mod udp;

pub use tcp::SmoltcpStack;
pub use udp::UdpStack;

/// A small queue of outbound frames the stack hands back to the datapath.
///
/// The datapath drives one rx → stack → tx round-trip per polled rx slot.
/// `FrameTxQueue` is a thin wrapper over `&mut Vec<Vec<u8>>` so the trait
/// shape doesn't leak `Vec` ownership semantics into stack implementations.
pub struct FrameTxQueue<'a> {
    inner: &'a mut Vec<Vec<u8>>,
}

impl<'a> FrameTxQueue<'a> {
    /// Wraps a borrowed transmit queue.
    #[must_use]
    pub fn new(inner: &'a mut Vec<Vec<u8>>) -> Self {
        Self { inner }
    }

    /// Queues a fully-built Ethernet frame for transmission.
    #[inline]
    pub fn push(&mut self, frame: Vec<u8>) {
        self.inner.push(frame);
    }

    /// Returns the number of frames currently queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Userspace L3/L4 stack that sits above the AF_XDP datapath.
///
/// The stack is **single-threaded** by construction: the AF_XDP socket is
/// pinned to a specific NIC queue and polled from one thread, and the stack
/// owns all per-connection state behind that single thread.  Implementations
/// therefore do not need any locking.
pub trait XdpStack: 'static {
    /// Connection type yielded when a peer establishes (or sends its first
    /// packet, depending on the stack).
    type Connection: LocalConnection;
    /// Stack-level error type.
    type Error: std::error::Error + 'static;

    /// Called by the datapath whenever a frame arrives on the rx ring.
    ///
    /// The implementation parses the frame, updates per-connection state,
    /// and may push zero or more outbound frames into `out`.  If a new
    /// connection became ready as a result, it is returned in
    /// `Ok(Some(conn))` so the listener can hand it to the application.
    ///
    /// # Errors
    /// Returns the stack-specific error if the frame is malformed beyond
    /// recovery or violates a protocol invariant.
    fn on_rx(
        &mut self,
        frame: &[u8],
        out: &mut FrameTxQueue<'_>,
    ) -> Result<Option<Self::Connection>, Self::Error>;

    /// Called periodically by the datapath even when no frames have
    /// arrived, to let the stack flush timers, retransmissions, ARP
    /// refreshes, etc.
    ///
    /// `UdpStack` typically does nothing here.  `SmoltcpStack` advances
    /// the smoltcp `Interface::poll` clock.
    ///
    /// # Errors
    /// Returns the stack-specific error if a timer-driven action fails.
    fn poll_timers(&mut self, out: &mut FrameTxQueue<'_>) -> Result<(), Self::Error>;

    /// Returns the local IP address the stack is bound to.
    ///
    /// Used by the listener for `local_addr()` reporting.
    fn local_ip(&self) -> IpAddr;
}
