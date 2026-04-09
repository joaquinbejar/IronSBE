//! TCP transport module.
//!
//! Provides Tokio-based TCP client and server implementations with SBE message
//! framing.  The [`TokioTcpTransport`] type implements [`crate::Transport`]
//! and is the default backend when the `tcp-tokio` feature is enabled.

use crate::traits;
use std::net::SocketAddr;

pub mod client;
pub mod framing;
pub mod server;

pub use client::{TcpClient, TcpClientConfig};
pub use framing::SbeFrameCodec;
pub use server::{TcpConnection, TcpServer, TcpServerConfig};

/// Tokio-based TCP transport backend.
///
/// This is the default [`Transport`](crate::Transport) implementation.
/// Connections are framed with a 4-byte little-endian length prefix using
/// [`SbeFrameCodec`].
///
/// # Server usage
///
/// ```ignore
/// let mut listener = TokioTcpTransport::bind("0.0.0.0:9000".parse()?).await?;
/// let conn = listener.accept().await?;
/// ```
///
/// # Client usage
///
/// ```ignore
/// let conn = TokioTcpTransport::connect("127.0.0.1:9000".parse()?).await?;
/// ```
pub struct TokioTcpTransport;

impl traits::Transport for TokioTcpTransport {
    type Listener = TcpServer;
    type Connection = TcpConnection;
    type Error = std::io::Error;

    async fn bind(addr: SocketAddr) -> Result<TcpServer, std::io::Error> {
        TcpServer::bind(TcpServerConfig::new(addr)).await
    }

    async fn connect(addr: SocketAddr) -> Result<TcpConnection, std::io::Error> {
        let stream = tokio::net::TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;
        let peer_addr = stream.peer_addr()?;
        let framed = tokio_util::codec::Framed::new(stream, SbeFrameCodec::new(64 * 1024));
        Ok(TcpConnection::from_framed(framed, peer_addr))
    }
}
