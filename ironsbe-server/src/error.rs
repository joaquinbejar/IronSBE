//! Error types for server operations.

use thiserror::Error;

/// Error type for server operations.
#[derive(Debug, Error)]
pub enum ServerError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Transport error.
    #[error("transport error: {0}")]
    Transport(#[from] ironsbe_transport::TransportError),

    /// Session error.
    #[error("session error: {message}")]
    Session {
        /// Error message.
        message: String,
    },

    /// Handler error.
    #[error("handler error: {message}")]
    Handler {
        /// Error message.
        message: String,
    },

    /// Channel error.
    #[error("channel error: {message}")]
    Channel {
        /// Error message.
        message: String,
    },

    /// Server shutdown.
    #[error("server shutdown")]
    Shutdown,
}
