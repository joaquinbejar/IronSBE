//! Error types for IronSBE core operations.

use thiserror::Error;

/// Core error type for IronSBE operations.
#[derive(Debug, Error)]
pub enum Error {
    /// Buffer is too short for the requested operation.
    #[error("buffer too short: required {required} bytes, available {available} bytes")]
    BufferTooShort {
        /// Required buffer size in bytes.
        required: usize,
        /// Available buffer size in bytes.
        available: usize,
    },

    /// Template ID mismatch during decoding.
    #[error("template mismatch: expected {expected}, actual {actual}")]
    TemplateMismatch {
        /// Expected template ID.
        expected: u16,
        /// Actual template ID found.
        actual: u16,
    },

    /// Schema ID mismatch during decoding.
    #[error("schema mismatch: expected {expected}, actual {actual}")]
    SchemaMismatch {
        /// Expected schema ID.
        expected: u16,
        /// Actual schema ID found.
        actual: u16,
    },

    /// Invalid enum value encountered.
    #[error("invalid enum value: tag {tag}, value {value}")]
    InvalidEnumValue {
        /// Field tag/ID.
        tag: u16,
        /// Invalid value encountered.
        value: u64,
    },

    /// Invalid UTF-8 encoding in string field.
    #[error("invalid UTF-8 at offset {offset}")]
    InvalidUtf8 {
        /// Byte offset where invalid UTF-8 was found.
        offset: usize,
    },

    /// Offset out of bounds.
    #[error("offset {offset} out of bounds for buffer of size {size}")]
    OffsetOutOfBounds {
        /// Requested offset.
        offset: usize,
        /// Buffer size in bytes.
        size: usize,
    },

    /// Group iteration error.
    #[error("group iteration error: {message}")]
    GroupError {
        /// Error message.
        message: String,
    },

    /// Version incompatibility.
    #[error(
        "version incompatible: message version {message_version}, min supported {min_supported}"
    )]
    VersionIncompatible {
        /// Message version.
        message_version: u16,
        /// Minimum supported version.
        min_supported: u16,
    },
}

/// Result type alias for IronSBE core operations.
pub type Result<T> = std::result::Result<T, Error>;
