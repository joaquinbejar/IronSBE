//! # IronSBE Transport
//!
//! Network transport layer for SBE messaging.
//!
//! This crate provides:
//! - [`traits`] - Backend-agnostic [`Transport`], [`Listener`], [`Connection`]
//!   traits (always available)
//! - [`tcp`] - Tokio-based TCP backend (feature `tcp-tokio`, enabled by
//!   default)
//! - [`udp`] - UDP unicast and multicast with A/B arbitration
//! - [`ipc`] - Shared memory IPC transport
//!
//! # Selecting a backend
//!
//! The default feature set enables `tcp-tokio`.  To use a different backend
//! (once available), disable default features and enable the desired one:
//!
//! ```toml
//! ironsbe-transport = { version = "...", default-features = false, features = ["tcp-uring"] }
//! ```
//!
//! [`DefaultTransport`] is a type alias that resolves to the backend selected
//! by the active feature.  Code that is generic over `T: Transport` can use
//! `DefaultTransport` as the default type parameter.

pub mod error;
pub mod ipc;
pub mod traits;
pub mod udp;

#[cfg(feature = "tcp-tokio")]
pub mod tcp;

pub use error::TransportError;
pub use traits::{Connection, Listener, Transport};

/// The transport backend selected by the active cargo feature.
///
/// Currently resolves to [`tcp::TokioTcpTransport`] when the `tcp-tokio`
/// feature is enabled (the default).
#[cfg(feature = "tcp-tokio")]
pub type DefaultTransport = tcp::TokioTcpTransport;
