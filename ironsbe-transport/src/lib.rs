//! # IronSBE Transport
//!
//! Network transport layer for SBE messaging.
//!
//! This crate provides:
//! - [`tcp`] - TCP client/server with message framing
//! - [`udp`] - UDP unicast and multicast with A/B arbitration
//! - [`ipc`] - Shared memory IPC transport

pub mod error;
pub mod ipc;
pub mod tcp;
pub mod udp;

pub use error::TransportError;
