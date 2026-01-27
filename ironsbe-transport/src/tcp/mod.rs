//! TCP transport module.
//!
//! Provides TCP client and server implementations with SBE message framing.

pub mod client;
pub mod framing;
pub mod server;

pub use client::{TcpClient, TcpClientConfig};
pub use framing::SbeFrameCodec;
pub use server::{TcpConnection, TcpServer, TcpServerConfig};
