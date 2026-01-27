//! Error types for client operations.

use thiserror::Error;

/// Error type for client operations.
#[derive(Debug, Error)]
pub enum ClientError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Transport error.
    #[error("transport error: {0}")]
    Transport(#[from] ironsbe_transport::TransportError),

    /// Connection timeout.
    #[error("connection timeout")]
    ConnectTimeout,

    /// Connection closed by server.
    #[error("connection closed")]
    ConnectionClosed,

    /// Maximum reconnect attempts reached.
    #[error("maximum reconnect attempts reached")]
    MaxReconnectAttempts,

    /// Channel error.
    #[error("channel error")]
    Channel,
}
