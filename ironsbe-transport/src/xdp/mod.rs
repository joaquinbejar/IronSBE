//! AF_XDP partial kernel-bypass backend.
//!
//! See [`crate::xdp`] in the docs and `docs/transport-backends.md` for
//! background.  The module is split into:
//!
//! - [`frames`]      — pure-Rust Ethernet/IPv4/UDP/ARP parsers and builders.
//! - [`stack`]       — [`XdpStack`] trait abstraction over the L3/L4 layer.
//! - [`stack::udp`]  — minimal UDP-based stack with length-prefix framing.
//! - [`stack::tcp`]  — `smoltcp`-based TCP stack, wire-compatible with
//!   `tcp-tokio`.
//!
//! The pure-Rust pieces compile everywhere under the `xdp-stacks` feature
//! and are fully unit-tested without root or hardware.  The AF_XDP
//! datapath itself (`xsk-rs` wrapper, UMEM, rx/tx ring poll loop) is
//! tracked in the follow-up issue together with the server integration
//! and benchmark suite — those pieces require both a Linux kernel and a
//! real NIC to validate, so they are landed separately.

pub mod frames;
pub mod stack;

#[cfg(all(feature = "xdp", target_os = "linux"))]
pub mod datapath;

#[cfg(all(feature = "xdp", target_os = "linux"))]
pub mod transport;

pub use frames::{
    FrameError, MacAddr, ParsedArp, ParsedFrame, ParsedUdp, build_arp_reply, build_udp_ipv4,
    parse_arp, parse_ethernet, parse_ipv4_udp,
};
pub use stack::{FrameTxQueue, SmoltcpStack, UdpStack, XdpStack};

#[cfg(all(feature = "xdp", target_os = "linux"))]
pub use datapath::{Datapath, DatapathConfig};
#[cfg(all(feature = "xdp", target_os = "linux"))]
pub use transport::{XdpConfig, XdpListener, XdpTransport};
