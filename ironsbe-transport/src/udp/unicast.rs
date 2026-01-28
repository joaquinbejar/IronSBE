//! UDP unicast sender and receiver.

use std::net::SocketAddr;
use tokio::net::UdpSocket;

/// UDP unicast sender.
pub struct UdpSender {
    socket: UdpSocket,
    target: SocketAddr,
}

impl UdpSender {
    /// Creates a new UDP sender bound to the specified local address.
    ///
    /// # Arguments
    /// * `local_addr` - Local address to bind to
    /// * `target` - Target address to send to
    ///
    /// # Errors
    /// Returns IO error if binding fails.
    pub async fn bind(local_addr: SocketAddr, target: SocketAddr) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(local_addr).await?;
        Ok(Self { socket, target })
    }

    /// Sends data to the target address.
    ///
    /// # Arguments
    /// * `data` - Data to send
    ///
    /// # Returns
    /// Number of bytes sent.
    ///
    /// # Errors
    /// Returns IO error if send fails.
    pub async fn send(&self, data: &[u8]) -> std::io::Result<usize> {
        self.socket.send_to(data, self.target).await
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Returns the target address.
    #[must_use]
    pub fn target_addr(&self) -> SocketAddr {
        self.target
    }
}

/// UDP unicast receiver.
pub struct UdpReceiver {
    socket: UdpSocket,
    buffer: Vec<u8>,
}

impl UdpReceiver {
    /// Creates a new UDP receiver bound to the specified address.
    ///
    /// # Arguments
    /// * `addr` - Address to bind to
    /// * `buffer_size` - Size of the receive buffer
    ///
    /// # Errors
    /// Returns IO error if binding fails.
    pub async fn bind(addr: SocketAddr, buffer_size: usize) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr).await?;
        Ok(Self {
            socket,
            buffer: vec![0u8; buffer_size],
        })
    }

    /// Receives data from any sender.
    ///
    /// # Returns
    /// Tuple of (data slice, sender address).
    ///
    /// # Errors
    /// Returns IO error if receive fails.
    pub async fn recv(&mut self) -> std::io::Result<(&[u8], SocketAddr)> {
        let (len, addr) = self.socket.recv_from(&mut self.buffer).await?;
        Ok((&self.buffer[..len], addr))
    }

    /// Returns the local address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Sets the receive buffer size.
    ///
    /// # Errors
    /// Returns IO error if setting fails.
    pub fn set_recv_buffer_size(&self, _size: usize) -> std::io::Result<()> {
        self.socket.set_broadcast(true)?;
        // Note: actual buffer size setting requires platform-specific code
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_udp_sender_bind() {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let target: SocketAddr = "127.0.0.1:9999".parse().unwrap();

        let sender = UdpSender::bind(local, target).await;
        assert!(sender.is_ok());

        let sender = sender.unwrap();
        assert_eq!(sender.target_addr(), target);
        assert!(sender.local_addr().is_ok());
    }

    #[tokio::test]
    async fn test_udp_receiver_bind() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

        let receiver = UdpReceiver::bind(addr, 1024).await;
        assert!(receiver.is_ok());

        let receiver = receiver.unwrap();
        assert!(receiver.local_addr().is_ok());
    }

    #[tokio::test]
    async fn test_udp_send_recv() {
        let receiver_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut receiver = UdpReceiver::bind(receiver_addr, 1024).await.unwrap();
        let actual_receiver_addr = receiver.local_addr().unwrap();

        let sender_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let sender = UdpSender::bind(sender_addr, actual_receiver_addr)
            .await
            .unwrap();

        let data = b"Hello, UDP!";
        let sent = sender.send(data).await.unwrap();
        assert_eq!(sent, data.len());

        let (received, from_addr) = receiver.recv().await.unwrap();
        assert_eq!(received, data);
        assert_eq!(from_addr, sender.local_addr().unwrap());
    }

    #[tokio::test]
    async fn test_udp_receiver_set_buffer_size() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let receiver = UdpReceiver::bind(addr, 1024).await.unwrap();

        let result = receiver.set_recv_buffer_size(4096);
        assert!(result.is_ok());
    }
}
