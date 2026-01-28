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

    /// Creates a multicast error.
    pub fn multicast(message: impl Into<String>) -> Self {
        Self::Multicast {
            message: message.into(),
        }
    }

    /// Creates an IPC error.
    pub fn ipc(message: impl Into<String>) -> Self {
        Self::Ipc {
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_error_display() {
        let err = TransportError::ConnectTimeout;
        assert_eq!(err.to_string(), "connection timeout");

        let err = TransportError::ConnectionClosed;
        assert_eq!(err.to_string(), "connection closed");
    }

    #[test]
    fn test_frame_too_large_error() {
        let err = TransportError::frame_too_large(100000, 65536);
        let msg = err.to_string();
        assert!(msg.contains("100000"));
        assert!(msg.contains("65536"));
        assert!(msg.contains("frame too large"));
    }

    #[test]
    fn test_invalid_frame_error() {
        let err = TransportError::invalid_frame("bad header");
        let msg = err.to_string();
        assert!(msg.contains("bad header"));
        assert!(msg.contains("invalid frame"));
    }

    #[test]
    fn test_channel_error() {
        let err = TransportError::channel("channel closed");
        let msg = err.to_string();
        assert!(msg.contains("channel closed"));
        assert!(msg.contains("channel error"));
    }

    #[test]
    fn test_multicast_error() {
        let err = TransportError::multicast("join failed");
        let msg = err.to_string();
        assert!(msg.contains("join failed"));
        assert!(msg.contains("multicast error"));
    }

    #[test]
    fn test_ipc_error() {
        let err = TransportError::ipc("shm not found");
        let msg = err.to_string();
        assert!(msg.contains("shm not found"));
        assert!(msg.contains("IPC error"));
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: TransportError = io_err.into();
        let msg = err.to_string();
        assert!(msg.contains("IO error"));
    }

    #[test]
    fn test_error_debug() {
        let err = TransportError::ConnectTimeout;
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("ConnectTimeout"));
    }
}
