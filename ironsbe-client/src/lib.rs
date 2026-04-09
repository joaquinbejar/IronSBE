//! # IronSBE Client
//!
//! Client-side engine for SBE messaging.
//!
//! This crate provides:
//! - Client builder with configuration options
//! - Automatic reconnection logic
//! - Async/sync bridging for message handling

pub mod builder;
pub mod error;
pub mod local_builder;
pub mod reconnect;
pub mod session;

pub use builder::{Client, ClientBuilder, ClientCommand, ClientEvent, ClientHandle};
pub use error::ClientError;
pub use local_builder::{LocalClient, LocalClientBuilder};
