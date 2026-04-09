//! TCP server implementation.

use super::framing::SbeFrameCodec;
use crate::traits;
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
    /// `SO_RCVBUF` to apply to accepted sockets, in bytes.
    ///
    /// `None` leaves the kernel default in place.  The kernel may clamp or
    /// double the requested value (Linux exposes `2 * size` via `getsockopt`).
    pub recv_buffer_size: Option<usize>,
    /// `SO_SNDBUF` to apply to accepted sockets, in bytes.
    ///
    /// Same caveats as [`recv_buffer_size`](Self::recv_buffer_size).
    pub send_buffer_size: Option<usize>,
}

impl Default for TcpServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:9000".parse().unwrap(),
            max_connections: 1000,
            max_frame_size: 64 * 1024,
            tcp_nodelay: true,
            recv_buffer_size: Some(256 * 1024),
            send_buffer_size: Some(256 * 1024),
        }
    }
}

impl From<SocketAddr> for TcpServerConfig {
    /// Creates a config with default tunables bound to `addr`.
    fn from(addr: SocketAddr) -> Self {
        Self::new(addr)
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

    /// Sets TCP_NODELAY.
    #[must_use]
    pub fn tcp_nodelay(mut self, enabled: bool) -> Self {
        self.tcp_nodelay = enabled;
        self
    }

    /// Sets `SO_RCVBUF` for accepted sockets.
    #[must_use]
    pub fn recv_buffer_size(mut self, size: usize) -> Self {
        self.recv_buffer_size = Some(size);
        self
    }

    /// Sets `SO_SNDBUF` for accepted sockets.
    #[must_use]
    pub fn send_buffer_size(mut self, size: usize) -> Self {
        self.send_buffer_size = Some(size);
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
    pub async fn accept(&mut self) -> std::io::Result<TcpConnection> {
        let (stream, addr) = self.listener.accept().await?;
        stream.set_nodelay(self.config.tcp_nodelay)?;
        super::apply_socket_buffer_sizes(
            &stream,
            self.config.recv_buffer_size,
            self.config.send_buffer_size,
        )?;

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
    /// Creates a `TcpConnection` from an already-framed stream.
    #[must_use]
    pub fn from_framed(framed: Framed<TcpStream, SbeFrameCodec>, peer_addr: SocketAddr) -> Self {
        Self { framed, peer_addr }
    }

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
    /// `Ok(Some(bytes))` if a message was received, `Ok(None)` if connection
    /// closed.
    ///
    /// # Errors
    /// Returns IO error if receive fails.
    pub async fn recv(&mut self) -> std::io::Result<Option<BytesMut>> {
        match self.framed.next().await {
            Some(result) => result.map(Some),
            None => Ok(None),
        }
    }

    /// Closes the connection.
    pub async fn close(mut self) -> std::io::Result<()> {
        SinkExt::<&[u8]>::close(&mut self.framed).await
    }
}

impl traits::Listener for TcpServer {
    type Connection = TcpConnection;
    type Error = std::io::Error;

    async fn accept(&mut self) -> Result<TcpConnection, std::io::Error> {
        TcpServer::accept(self).await
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        TcpServer::local_addr(self)
    }
}

impl traits::Connection for TcpConnection {
    type Error = std::io::Error;

    async fn recv(&mut self) -> Result<Option<BytesMut>, std::io::Error> {
        TcpConnection::recv(self).await
    }

    async fn send<'a>(&'a mut self, msg: &'a [u8]) -> Result<(), std::io::Error> {
        TcpConnection::send(self, msg).await
    }

    fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        Ok(self.peer_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcp_server_config_default() {
        let config = TcpServerConfig::default();
        assert_eq!(config.bind_addr.port(), 9000);
        assert_eq!(config.max_connections, 1000);
        assert_eq!(config.max_frame_size, 64 * 1024);
        assert!(config.tcp_nodelay);
    }

    #[test]
    fn test_tcp_server_config_new() {
        let addr: SocketAddr = "192.168.1.1:8080".parse().unwrap();
        let config = TcpServerConfig::new(addr);
        assert_eq!(config.bind_addr, addr);
        assert_eq!(config.max_connections, 1000);
    }

    #[test]
    fn test_tcp_server_config_builder() {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let config = TcpServerConfig::new(addr)
            .max_connections(500)
            .max_frame_size(128 * 1024);

        assert_eq!(config.max_connections, 500);
        assert_eq!(config.max_frame_size, 128 * 1024);
    }

    #[test]
    fn test_tcp_server_config_clone() {
        let config = TcpServerConfig::default();
        let cloned = config.clone();
        assert_eq!(config.bind_addr, cloned.bind_addr);
        assert_eq!(config.max_connections, cloned.max_connections);
    }

    #[test]
    fn test_tcp_server_config_debug() {
        let config = TcpServerConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("TcpServerConfig"));
        assert!(debug_str.contains("9000"));
    }
}
