//! TCP server implementation.

use super::framing::SbeFrameCodec;
use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

/// Configuration for TCP server.
#[derive(Debug, Clone)]
pub struct TcpServerConfig {
    /// Address to bind to.
    pub bind_addr: SocketAddr,
    /// Maximum number of connections.
    pub max_connections: usize,
    /// Maximum frame size in bytes.
    pub max_frame_size: usize,
    /// Enable TCP_NODELAY.
    pub tcp_nodelay: bool,
}

impl Default for TcpServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:9000".parse().unwrap(),
            max_connections: 1000,
            max_frame_size: 64 * 1024,
            tcp_nodelay: true,
        }
    }
}

impl TcpServerConfig {
    /// Creates a new server config with the specified bind address.
    #[must_use]
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            ..Default::default()
        }
    }

    /// Sets the maximum number of connections.
    #[must_use]
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Sets the maximum frame size.
    #[must_use]
    pub fn max_frame_size(mut self, size: usize) -> Self {
        self.max_frame_size = size;
        self
    }
}

/// TCP server for SBE messaging.
pub struct TcpServer {
    listener: TcpListener,
    config: Arc<TcpServerConfig>,
}

impl TcpServer {
    /// Binds to the specified address and creates a new server.
    ///
    /// # Arguments
    /// * `config` - Server configuration
    ///
    /// # Errors
    /// Returns IO error if binding fails.
    pub async fn bind(config: TcpServerConfig) -> std::io::Result<Self> {
        let listener = TcpListener::bind(config.bind_addr).await?;
        Ok(Self {
            listener,
            config: Arc::new(config),
        })
    }

    /// Accepts a new connection.
    ///
    /// # Returns
    /// A new `TcpConnection` for the accepted client.
    ///
    /// # Errors
    /// Returns IO error if accept fails.
    pub async fn accept(&self) -> std::io::Result<TcpConnection> {
        let (stream, addr) = self.listener.accept().await?;
        stream.set_nodelay(self.config.tcp_nodelay)?;

        Ok(TcpConnection {
            framed: Framed::new(stream, SbeFrameCodec::new(self.config.max_frame_size)),
            peer_addr: addr,
        })
    }

    /// Returns the local address the server is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// A TCP connection to a client.
pub struct TcpConnection {
    framed: Framed<TcpStream, SbeFrameCodec>,
    peer_addr: SocketAddr,
}

impl TcpConnection {
    /// Returns the peer address.
    #[must_use]
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    /// Sends a message to the client.
    ///
    /// # Arguments
    /// * `message` - Message bytes to send
    ///
    /// # Errors
    /// Returns IO error if send fails.
    pub async fn send(&mut self, message: &[u8]) -> std::io::Result<()> {
        self.framed.send(message).await
    }

    /// Receives a message from the client.
    ///
    /// # Returns
    /// `Some(Ok(bytes))` if a message was received, `None` if connection closed.
    pub async fn recv(&mut self) -> Option<std::io::Result<BytesMut>> {
        self.framed.next().await
    }

    /// Closes the connection.
    pub async fn close(mut self) -> std::io::Result<()> {
        SinkExt::<&[u8]>::close(&mut self.framed).await
    }
}
