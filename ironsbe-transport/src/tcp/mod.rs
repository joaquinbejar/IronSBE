//! TCP transport module.
//!
//! Provides Tokio-based TCP client and server implementations with SBE message
//! framing.  The [`TokioTcpTransport`] type implements [`crate::Transport`]
//! and is the default backend when the `tcp-tokio` feature is enabled.

use crate::traits;
use socket2::SockRef;
use tokio::net::TcpStream;

pub mod client;
pub mod framing;
pub mod server;

pub use client::{TcpClient, TcpClientConfig};
pub use framing::SbeFrameCodec;
pub use server::{TcpConnection, TcpServer, TcpServerConfig};

/// Applies optional `SO_RCVBUF` / `SO_SNDBUF` to a borrowed TCP stream.
///
/// `recv` / `send` are interpreted as **requested** sizes in bytes.  The
/// kernel may clamp the value, and on Linux the value reported by
/// `getsockopt` will typically be double what was set.  Both options are
/// best-effort, but I/O errors are propagated so callers can detect a
/// completely broken socket.
///
/// # Errors
/// Returns the underlying I/O error if `setsockopt` fails.
pub(crate) fn apply_socket_buffer_sizes(
    stream: &TcpStream,
    recv: Option<usize>,
    send: Option<usize>,
) -> std::io::Result<()> {
    let sock = SockRef::from(stream);
    if let Some(size) = recv {
        sock.set_recv_buffer_size(size)?;
    }
    if let Some(size) = send {
        sock.set_send_buffer_size(size)?;
    }
    Ok(())
}

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
        apply_socket_buffer_sizes(&stream, config.recv_buffer_size, config.send_buffer_size)?;
        let peer_addr = stream.peer_addr()?;
        let framed =
            tokio_util::codec::Framed::new(stream, SbeFrameCodec::new(config.max_frame_size));
        Ok(TcpConnection::from_framed(framed, peer_addr))
    }
}
