//! Error types for transport operations.

use thiserror::Error;

/// Error type for transport operations.
#[derive(Debug, Error)]
pub enum TransportError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Connection timeout.
    #[error("connection timeout")]
    ConnectTimeout,

    /// Connection closed.
    #[error("connection closed")]
    ConnectionClosed,

    /// Frame too large.
    #[error("frame too large: {size} bytes exceeds maximum {max} bytes")]
    FrameTooLarge {
        /// Actual frame size.
        size: usize,
        /// Maximum allowed size.
        max: usize,
    },

    /// Invalid frame.
    #[error("invalid frame: {message}")]
    InvalidFrame {
        /// Error message.
        message: String,
    },

    /// Address parse error.
    #[error("address parse error: {0}")]
    AddrParse(#[from] std::net::AddrParseError),

    /// Channel error.
    #[error("channel error: {message}")]
    Channel {
        /// Error message.
        message: String,
    },

    /// Multicast error.
    #[error("multicast error: {message}")]
    Multicast {
        /// Error message.
        message: String,
    },

    /// IPC error.
    #[error("IPC error: {message}")]
    Ipc {
        /// Error message.
        message: String,
    },
}

impl TransportError {
    /// Creates a frame too large error.
    pub fn frame_too_large(size: usize, max: usize) -> Self {
        Self::FrameTooLarge { size, max }
    }

    /// Creates an invalid frame error.
    pub fn invalid_frame(message: impl Into<String>) -> Self {
        Self::InvalidFrame {
            message: message.into(),
        }
    }

    /// Creates a channel error.
    pub fn channel(message: impl Into<String>) -> Self {
        Self::Channel {
            message: message.into(),
        }
    }
}
