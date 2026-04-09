//! Linux io_uring TCP backend (feature `tcp-uring`).
//!
//! Implements [`crate::traits::LocalTransport`],
//! [`crate::traits::LocalListener`], and [`crate::traits::LocalConnection`]
//! on top of [`tokio_uring`].  All operations submit
//! Submission Queue Entries (SQEs) to the kernel via `io_uring(7)`, returning
//! ownership of buffers in [`tokio_uring::BufResult`] so that no in-flight
//! buffer is ever borrowed.
//!
//! # Requirements
//!
//! - Linux kernel **≥ 5.10** (older kernels lack the syscalls used by
//!   `tokio-uring 0.5`).
//! - The application must run inside a [`tokio_uring::start`] block; the
//!   crate is not interoperable with the standard Tokio reactor.
//!
//! # Wire format
//!
//! Identical to the Tokio TCP backend (4-byte little-endian length prefix
//! followed by the payload), so the two backends are wire-compatible — a
//! `tcp-tokio` server can serve a `tcp-uring` client and vice versa.
//!
//! # Scope of this module
//!
//! This is the minimal backend that satisfies the trait surface.  Buffer
//! pooling, registered buffers, registered fds, and `IORING_OP_SEND_ZC` are
//! out of scope here and tracked in the follow-up issue.

use crate::traits;
use bytes::{Bytes, BytesMut};
use std::io;
use std::net::SocketAddr;

/// Length-prefix size in bytes (matches `SbeFrameCodec`).
const LENGTH_PREFIX_BYTES: usize = 4;

/// Default read buffer size used when the caller does not override
/// `max_frame_size`.
const DEFAULT_MAX_FRAME_SIZE: usize = 64 * 1024;

/// Configuration for [`UringTcpTransport::bind_with`].
#[derive(Debug, Clone)]
pub struct UringServerConfig {
    /// Address to bind to.
    pub bind_addr: SocketAddr,
    /// Maximum SBE frame size, in bytes.
    pub max_frame_size: usize,
}

impl Default for UringServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:9000"
                .parse()
                .expect("hardcoded default bind addr is valid"),
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
        }
    }
}

impl From<SocketAddr> for UringServerConfig {
    fn from(addr: SocketAddr) -> Self {
        Self::new(addr)
    }
}

impl UringServerConfig {
    /// Creates a new server config bound to `bind_addr` with default tunables.
    #[must_use]
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            ..Default::default()
        }
    }

    /// Sets the maximum SBE frame size.
    #[must_use]
    pub fn max_frame_size(mut self, size: usize) -> Self {
        self.max_frame_size = size;
        self
    }
}

/// Configuration for [`UringTcpTransport::connect_with`].
#[derive(Debug, Clone)]
pub struct UringClientConfig {
    /// Address of the remote server.
    pub server_addr: SocketAddr,
    /// Maximum SBE frame size, in bytes.
    pub max_frame_size: usize,
}

impl Default for UringClientConfig {
    fn default() -> Self {
        Self {
            server_addr: "127.0.0.1:9000"
                .parse()
                .expect("hardcoded default server addr is valid"),
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
        }
    }
}

impl From<SocketAddr> for UringClientConfig {
    fn from(addr: SocketAddr) -> Self {
        Self::new(addr)
    }
}

impl UringClientConfig {
    /// Creates a new client config targeting `server_addr` with default
    /// tunables.
    #[must_use]
    pub fn new(server_addr: SocketAddr) -> Self {
        Self {
            server_addr,
            ..Default::default()
        }
    }

    /// Sets the maximum SBE frame size.
    #[must_use]
    pub fn max_frame_size(mut self, size: usize) -> Self {
        self.max_frame_size = size;
        self
    }
}

/// Linux io_uring TCP transport backend.
///
/// Implements [`traits::LocalTransport`].  All operations must be driven
/// from inside a [`tokio_uring::start`] block.
pub struct UringTcpTransport;

impl traits::LocalTransport for UringTcpTransport {
    type Listener = UringListener;
    type Connection = UringConnection;
    type Error = io::Error;
    type BindConfig = UringServerConfig;
    type ConnectConfig = UringClientConfig;

    async fn bind_with(config: UringServerConfig) -> io::Result<UringListener> {
        let listener = tokio_uring::net::TcpListener::bind(config.bind_addr)?;
        Ok(UringListener {
            listener,
            max_frame_size: config.max_frame_size,
        })
    }

    async fn connect_with(config: UringClientConfig) -> io::Result<UringConnection> {
        let stream = tokio_uring::net::TcpStream::connect(config.server_addr).await?;
        let peer_addr = config.server_addr;
        Ok(UringConnection::new(
            stream,
            peer_addr,
            config.max_frame_size,
        ))
    }
}

/// Server-side listener for [`UringTcpTransport`].
pub struct UringListener {
    listener: tokio_uring::net::TcpListener,
    max_frame_size: usize,
}

impl traits::LocalListener for UringListener {
    type Connection = UringConnection;
    type Error = io::Error;

    async fn accept(&mut self) -> io::Result<UringConnection> {
        let (stream, peer_addr) = self.listener.accept().await?;
        Ok(UringConnection::new(stream, peer_addr, self.max_frame_size))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// A single io_uring TCP connection.
///
/// Owns the socket and a small staging buffer used to assemble incoming
/// frames.  Buffers are passed by value through `tokio-uring`'s `BufResult`
/// API so that the kernel always has exclusive ownership of any in-flight
/// buffer (a hard requirement of io_uring).
pub struct UringConnection {
    stream: tokio_uring::net::TcpStream,
    peer_addr: SocketAddr,
    max_frame_size: usize,
    /// Already-received bytes that have not yet been consumed by `recv`.
    pending: BytesMut,
}

impl UringConnection {
    /// Creates a new connection wrapping an existing `tokio-uring` stream.
    fn new(
        stream: tokio_uring::net::TcpStream,
        peer_addr: SocketAddr,
        max_frame_size: usize,
    ) -> Self {
        Self {
            stream,
            peer_addr,
            max_frame_size,
            pending: BytesMut::with_capacity(max_frame_size + LENGTH_PREFIX_BYTES),
        }
    }

    /// Attempts to extract one complete frame from the pending buffer.
    ///
    /// Returns `Ok(Some(frame))` if a full frame is available, `Ok(None)` if
    /// more bytes are needed, or an error if the length prefix is malformed
    /// or exceeds `max_frame_size`.
    fn try_take_frame(&mut self) -> io::Result<Option<BytesMut>> {
        if self.pending.len() < LENGTH_PREFIX_BYTES {
            return Ok(None);
        }
        let len_bytes = [
            self.pending[0],
            self.pending[1],
            self.pending[2],
            self.pending[3],
        ];
        let frame_len = u32::from_le_bytes(len_bytes) as usize;
        if frame_len > self.max_frame_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "frame length {frame_len} exceeds max_frame_size {}",
                    self.max_frame_size
                ),
            ));
        }
        let total = LENGTH_PREFIX_BYTES
            .checked_add(frame_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "frame length overflow"))?;
        if self.pending.len() < total {
            return Ok(None);
        }
        // Advance past the prefix and split off the payload.
        let _prefix = self.pending.split_to(LENGTH_PREFIX_BYTES);
        let payload = self.pending.split_to(frame_len);
        Ok(Some(payload))
    }
}

impl traits::LocalConnection for UringConnection {
    type Error = io::Error;

    async fn recv(&mut self) -> io::Result<Option<BytesMut>> {
        loop {
            if let Some(frame) = self.try_take_frame()? {
                return Ok(Some(frame));
            }
            // Submit a fresh read into a fixed-size scratch buffer.  The
            // buffer is owned by the kernel for the duration of the op and
            // returned via BufResult.
            let scratch = vec![0u8; self.max_frame_size + LENGTH_PREFIX_BYTES];
            let (res, buf) = self.stream.read(scratch).await;
            let n = res?;
            if n == 0 {
                // Peer closed cleanly.
                return Ok(None);
            }
            self.pending.extend_from_slice(&buf[..n]);
        }
    }

    async fn send(&mut self, msg: &[u8]) -> io::Result<()> {
        // Borrow path: copy into an owned buffer (io_uring requires owned).
        // Callers wanting true zero-copy should use `send_owned` instead.
        let owned = Bytes::copy_from_slice(msg);
        self.send_owned(owned).await
    }

    async fn send_owned(&mut self, msg: Bytes) -> io::Result<()> {
        let frame_len = msg.len();
        if frame_len > self.max_frame_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "frame length {frame_len} exceeds max_frame_size {}",
                    self.max_frame_size
                ),
            ));
        }
        // The on-wire length prefix is a 4-byte little-endian u32, so any
        // frame longer than u32::MAX cannot be encoded without truncation.
        // Reject explicitly rather than letting `as u32` silently wrap.
        let frame_len_u32: u32 = u32::try_from(frame_len).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame length {frame_len} exceeds u32::MAX"),
            )
        })?;
        let total = LENGTH_PREFIX_BYTES
            .checked_add(frame_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "frame length overflow"))?;
        // Build a single owned frame: 4-byte LE length prefix followed by
        // the payload.  Allocating a fresh Vec here is the simplest correct
        // path; a future change can use a pooled BytesMut to remove the
        // allocation.
        let mut framed = Vec::with_capacity(total);
        framed.extend_from_slice(&frame_len_u32.to_le_bytes());
        framed.extend_from_slice(&msg);
        // tokio-uring's write_all returns the buffer back regardless of
        // success so we can drop it cleanly.
        let (res, _buf) = self.stream.write_all(framed).await;
        res
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uring_server_config_default() {
        let cfg = UringServerConfig::default();
        assert_eq!(cfg.bind_addr.port(), 9000);
        assert_eq!(cfg.max_frame_size, DEFAULT_MAX_FRAME_SIZE);
    }

    #[test]
    fn test_uring_server_config_new_and_builder() {
        let addr: SocketAddr = "127.0.0.1:8080"
            .parse()
            .expect("test addr literal is valid");
        let cfg = UringServerConfig::new(addr).max_frame_size(128 * 1024);
        assert_eq!(cfg.bind_addr, addr);
        assert_eq!(cfg.max_frame_size, 128 * 1024);
    }

    #[test]
    fn test_uring_server_config_from_socket_addr() {
        let addr: SocketAddr = "127.0.0.1:8081"
            .parse()
            .expect("test addr literal is valid");
        let cfg: UringServerConfig = addr.into();
        assert_eq!(cfg.bind_addr, addr);
        assert_eq!(cfg.max_frame_size, DEFAULT_MAX_FRAME_SIZE);
    }

    #[test]
    fn test_uring_client_config_default() {
        let cfg = UringClientConfig::default();
        assert_eq!(cfg.server_addr.port(), 9000);
        assert_eq!(cfg.max_frame_size, DEFAULT_MAX_FRAME_SIZE);
    }

    #[test]
    fn test_uring_client_config_new_and_builder() {
        let addr: SocketAddr = "127.0.0.1:8082"
            .parse()
            .expect("test addr literal is valid");
        let cfg = UringClientConfig::new(addr).max_frame_size(256 * 1024);
        assert_eq!(cfg.server_addr, addr);
        assert_eq!(cfg.max_frame_size, 256 * 1024);
    }

    #[test]
    fn test_uring_client_config_from_socket_addr() {
        let addr: SocketAddr = "127.0.0.1:8083"
            .parse()
            .expect("test addr literal is valid");
        let cfg: UringClientConfig = addr.into();
        assert_eq!(cfg.server_addr, addr);
        assert_eq!(cfg.max_frame_size, DEFAULT_MAX_FRAME_SIZE);
    }
}
