//! Backend-agnostic transport traits.
//!
//! This module defines the core abstractions that every transport backend must
//! implement.  Connections are **framed**: [`Connection::recv`] returns one
//! complete SBE message (length prefix already stripped) and
//! [`Connection::send`] writes one message (length prefix added by the
//! backend).  Keeping the framing codec inside the backend lets future
//! zero-copy transports (io_uring, DPDK, ef_vi) avoid extra buffer copies.
//!
//! # Adding a new backend
//!
//! 1. Create a new module under `ironsbe-transport/src/` (e.g. `uring/`).
//! 2. Implement [`Transport`], [`Listener`], and [`Connection`] for your types.
//! 3. Gate the module behind a cargo feature (e.g. `tcp-uring`).
//! 4. Add a conditional `DefaultTransport` alias in `lib.rs` if appropriate.

use bytes::BytesMut;
use std::future::Future;
use std::net::SocketAddr;

/// Backend-agnostic transport factory.
///
/// A `Transport` knows how to create server-side [`Listener`]s and
/// client-side [`Connection`]s for a given socket address.
///
/// # Examples
///
/// ```ignore
/// use ironsbe_transport::{Transport, DefaultTransport};
///
/// let listener = DefaultTransport::bind("0.0.0.0:9000".parse().unwrap()).await?;
/// let conn     = DefaultTransport::connect("127.0.0.1:9000".parse().unwrap()).await?;
/// ```
pub trait Transport: Send + Sync + 'static {
    /// Server-side listener produced by [`bind`](Self::bind).
    type Listener: Listener<Connection = Self::Connection>;
    /// A single framed connection (client **or** accepted server connection).
    type Connection: Connection;
    /// Error type returned by transport operations.
    type Error: std::error::Error + Send + Sync + 'static;
    /// Backend-specific configuration consumed by [`bind_with`](Self::bind_with).
    ///
    /// Must be constructible from a bare [`SocketAddr`] so generic callers
    /// that only know the bind address can still spin up a listener with
    /// default tunables.
    type BindConfig: From<SocketAddr> + Clone + Send + Sync + 'static;
    /// Backend-specific configuration consumed by [`connect_with`](Self::connect_with).
    ///
    /// Must be constructible from a bare [`SocketAddr`] for the same reason
    /// as [`BindConfig`](Self::BindConfig).
    type ConnectConfig: From<SocketAddr> + Clone + Send + Sync + 'static;

    /// Binds a listener using a backend-specific configuration.
    ///
    /// Backends must implement this method.  [`bind`](Self::bind) is provided
    /// as a default that constructs `Self::BindConfig` from the address only.
    ///
    /// # Errors
    /// Returns an error if the address is already in use or binding fails.
    fn bind_with(
        config: Self::BindConfig,
    ) -> impl Future<Output = Result<Self::Listener, Self::Error>> + Send;

    /// Opens a client connection using a backend-specific configuration.
    ///
    /// Backends must implement this method.  [`connect`](Self::connect) is
    /// provided as a default that constructs `Self::ConnectConfig` from the
    /// address only.
    ///
    /// # Errors
    /// Returns an error if the connection cannot be established.
    fn connect_with(
        config: Self::ConnectConfig,
    ) -> impl Future<Output = Result<Self::Connection, Self::Error>> + Send;

    /// Binds a listener to `addr` using default tunables.
    ///
    /// Convenience wrapper around [`bind_with`](Self::bind_with) for callers
    /// that do not need to override backend-specific options.
    ///
    /// # Errors
    /// Returns an error if the address is already in use or binding fails.
    fn bind(addr: SocketAddr) -> impl Future<Output = Result<Self::Listener, Self::Error>> + Send {
        Self::bind_with(Self::BindConfig::from(addr))
    }

    /// Opens a client connection to `addr` using default tunables.
    ///
    /// Convenience wrapper around [`connect_with`](Self::connect_with) for
    /// callers that do not need to override backend-specific options.
    ///
    /// # Errors
    /// Returns an error if the connection cannot be established.
    fn connect(
        addr: SocketAddr,
    ) -> impl Future<Output = Result<Self::Connection, Self::Error>> + Send {
        Self::connect_with(Self::ConnectConfig::from(addr))
    }
}

/// Server-side listener that accepts incoming connections.
pub trait Listener: Send + 'static {
    /// Connection type yielded by [`accept`](Self::accept).
    type Connection: Connection;
    /// Error type returned by listener operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Accepts the next inbound connection.
    ///
    /// # Errors
    /// Returns an error if the accept syscall fails.
    fn accept(&mut self)
    -> impl Future<Output = Result<Self::Connection, Self::Error>> + Send + '_;

    /// Returns the local address this listener is bound to.
    ///
    /// # Errors
    /// Returns an IO error if the address cannot be determined.
    fn local_addr(&self) -> std::io::Result<SocketAddr>;
}

/// A framed, message-oriented connection.
///
/// Every call to [`recv`](Self::recv) returns exactly one SBE message (the
/// length prefix has already been consumed by the backend).  Every call to
/// [`send`](Self::send) transmits one message (the backend prepends the
/// length prefix).
pub trait Connection: Send + 'static {
    /// Error type returned by connection operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Receives one framed SBE message.
    ///
    /// Returns `Ok(Some(bytes))` when a message is available, or `Ok(None)`
    /// when the peer has closed the connection.
    ///
    /// # Errors
    /// Returns an error on I/O failure or protocol violation.
    fn recv(&mut self) -> impl Future<Output = Result<Option<BytesMut>, Self::Error>> + Send + '_;

    /// Sends one framed SBE message.
    ///
    /// # Errors
    /// Returns an error on I/O failure.
    fn send<'a>(
        &'a mut self,
        msg: &'a [u8],
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'a;

    /// Returns the remote peer address.
    ///
    /// # Errors
    /// Returns an IO error if the address cannot be determined.
    fn peer_addr(&self) -> std::io::Result<SocketAddr>;
}
