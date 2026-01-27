//! TCP client implementation.

use super::framing::SbeFrameCodec;
use crate::error::TransportError;
use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

/// Configuration for TCP client.
#[derive(Debug, Clone)]
pub struct TcpClientConfig {
    /// Server address to connect to.
    pub server_addr: SocketAddr,
    /// Connection timeout.
    pub connect_timeout: Duration,
    /// Maximum frame size in bytes.
    pub max_frame_size: usize,
    /// Enable TCP_NODELAY.
    pub tcp_nodelay: bool,
    /// Receive buffer size.
    pub recv_buffer_size: Option<usize>,
    /// Send buffer size.
    pub send_buffer_size: Option<usize>,
}

impl Default for TcpClientConfig {
    fn default() -> Self {
        Self {
            server_addr: "127.0.0.1:9000".parse().unwrap(),
            connect_timeout: Duration::from_secs(5),
            max_frame_size: 64 * 1024,
            tcp_nodelay: true,
            recv_buffer_size: Some(256 * 1024),
            send_buffer_size: Some(256 * 1024),
        }
    }
}

impl TcpClientConfig {
    /// Creates a new client config with the specified server address.
    #[must_use]
    pub fn new(server_addr: SocketAddr) -> Self {
        Self {
            server_addr,
            ..Default::default()
        }
    }

    /// Sets the connection timeout.
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Sets the maximum frame size.
    #[must_use]
    pub fn max_frame_size(mut self, size: usize) -> Self {
        self.max_frame_size = size;
        self
    }

    /// Sets TCP_NODELAY option.
    #[must_use]
    pub fn tcp_nodelay(mut self, enabled: bool) -> Self {
        self.tcp_nodelay = enabled;
        self
    }
}

/// TCP client for SBE messaging.
pub struct TcpClient {
    framed: Framed<TcpStream, SbeFrameCodec>,
    peer_addr: SocketAddr,
}

impl TcpClient {
    /// Connects to a server with the given configuration.
    ///
    /// # Arguments
    /// * `config` - Client configuration
    ///
    /// # Errors
    /// Returns `TransportError` if connection fails.
    pub async fn connect(config: TcpClientConfig) -> Result<Self, TransportError> {
        let stream = tokio::time::timeout(
            config.connect_timeout,
            TcpStream::connect(config.server_addr),
        )
        .await
        .map_err(|_| TransportError::ConnectTimeout)?
        .map_err(TransportError::Io)?;

        // Configure socket
        stream.set_nodelay(config.tcp_nodelay)?;

        let peer_addr = stream.peer_addr()?;
        let framed = Framed::new(stream, SbeFrameCodec::new(config.max_frame_size));

        Ok(Self { framed, peer_addr })
    }

    /// Sends a message to the server.
    ///
    /// # Arguments
    /// * `message` - Message bytes to send
    ///
    /// # Errors
    /// Returns `TransportError` if send fails.
    pub async fn send(&mut self, message: &[u8]) -> Result<(), TransportError> {
        self.framed.send(message).await.map_err(TransportError::Io)
    }

    /// Receives a message from the server.
    ///
    /// # Returns
    /// `Ok(Some(bytes))` if a message was received, `Ok(None)` if connection closed.
    ///
    /// # Errors
    /// Returns `TransportError` if receive fails.
    pub async fn recv(&mut self) -> Result<Option<BytesMut>, TransportError> {
        self.framed
            .next()
            .await
            .transpose()
            .map_err(TransportError::Io)
    }

    /// Returns the peer address.
    #[must_use]
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    /// Closes the connection.
    pub async fn close(mut self) -> Result<(), TransportError> {
        SinkExt::<&[u8]>::close(&mut self.framed)
            .await
            .map_err(TransportError::Io)
    }
}
