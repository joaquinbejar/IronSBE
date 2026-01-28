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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcp_client_config_default() {
        let config = TcpClientConfig::default();
        assert_eq!(config.server_addr.port(), 9000);
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
        assert_eq!(config.max_frame_size, 64 * 1024);
        assert!(config.tcp_nodelay);
        assert_eq!(config.recv_buffer_size, Some(256 * 1024));
        assert_eq!(config.send_buffer_size, Some(256 * 1024));
    }

    #[test]
    fn test_tcp_client_config_new() {
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        let config = TcpClientConfig::new(addr);
        assert_eq!(config.server_addr, addr);
        assert_eq!(config.connect_timeout, Duration::from_secs(5));
    }

    #[test]
    fn test_tcp_client_config_builder() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let config = TcpClientConfig::new(addr)
            .connect_timeout(Duration::from_secs(10))
            .max_frame_size(128 * 1024)
            .tcp_nodelay(false);

        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert_eq!(config.max_frame_size, 128 * 1024);
        assert!(!config.tcp_nodelay);
    }

    #[test]
    fn test_tcp_client_config_clone() {
        let config = TcpClientConfig::default();
        let cloned = config.clone();
        assert_eq!(config.server_addr, cloned.server_addr);
        assert_eq!(config.max_frame_size, cloned.max_frame_size);
    }

    #[test]
    fn test_tcp_client_config_debug() {
        let config = TcpClientConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("TcpClientConfig"));
        assert!(debug_str.contains("9000"));
    }
}
