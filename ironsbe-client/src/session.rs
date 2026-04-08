//! Client session management.
//!
//! Wraps a transport [`Connection`] to provide send/recv for the client.

use bytes::BytesMut;
use ironsbe_transport::traits::Connection;

/// Client session wrapping a transport [`Connection`].
///
/// `C` is the concrete connection type supplied by the active
/// [`Transport`](ironsbe_transport::Transport) backend.
pub struct ClientSession<C: Connection> {
    conn: C,
}

impl<C: Connection> ClientSession<C> {
    /// Creates a new client session from a transport connection.
    #[must_use]
    pub fn new(conn: C) -> Self {
        Self { conn }
    }

    /// Sends a message to the server.
    ///
    /// # Errors
    /// Returns an error if send fails.
    pub async fn send(&mut self, message: &[u8]) -> std::io::Result<()> {
        self.conn
            .send(message)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Receives a message from the server.
    ///
    /// # Returns
    /// `Ok(Some(bytes))` if received, `Ok(None)` if connection closed.
    ///
    /// # Errors
    /// Returns an error if receive fails.
    pub async fn recv(&mut self) -> std::io::Result<Option<BytesMut>> {
        self.conn
            .recv()
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
}
