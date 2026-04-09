//! TCP transport module.
//!
//! Provides Tokio-based TCP client and server implementations with SBE message
//! framing.  The [`TokioTcpTransport`] type implements [`crate::Transport`]
//! and is the default backend when the `tcp-tokio` feature is enabled.

use crate::traits;

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
    type BindConfig = TcpServerConfig;
    type ConnectConfig = TcpClientConfig;

    async fn bind_with(config: TcpServerConfig) -> Result<TcpServer, std::io::Error> {
        TcpServer::bind(config).await
    }

    async fn connect_with(config: TcpClientConfig) -> Result<TcpConnection, std::io::Error> {
        let stream = tokio::time::timeout(
            config.connect_timeout,
            tokio::net::TcpStream::connect(config.server_addr),
        )
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout"))??;
        stream.set_nodelay(config.tcp_nodelay)?;
        let peer_addr = stream.peer_addr()?;
        let framed =
            tokio_util::codec::Framed::new(stream, SbeFrameCodec::new(config.max_frame_size));
        Ok(TcpConnection::from_framed(framed, peer_addr))
    }
}
