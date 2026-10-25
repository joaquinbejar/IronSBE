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

/// Former name of [`EncodeError`], kept for code written against 0.7.2.
#[deprecated(since = "0.8.0", note = "renamed to `EncodeError`")]
pub type VarDataError = EncodeError;

/// Error returned by the fallible methods of generated encoders: `wrap`,
/// `<group>_count`, `next_entry`, `try_set_<field>` and `finish`.
///
/// Every check runs before anything is written, so on error the buffer
/// and the encoder's write cursor are left unchanged. `field` names the
/// part concerned: a var data field (`note`), a repeating group
/// (`repeating group 'legs'`), an entry or the message block.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum EncodeError {
    /// The value is longer than the field's length header can encode.
    #[error("{field}: length {length} bytes exceeds the length header maximum {max}")]
    LengthOverflow {
        /// The var data field, e.g. `var data field 'note'`.
        field: &'static str,
        /// Length of the rejected value in bytes.
        length: usize,
        /// Largest length the header can encode, in bytes.
        max: u64,
    },

    /// The part, or a part after it, was already written.
    #[error("{field} written out of schema order (parts already written: {written})")]
    OutOfOrder {
        /// The part being written.
        field: &'static str,
        /// Number of variable parts already written on the owner.
        written: u16,
    },

    /// The buffer cannot hold what the call would write: the part itself
    /// plus the empty headers of any skipped parts.
    #[error("{field}: buffer too short, required {required} bytes, available {available} bytes")]
    BufferTooShort {
        /// The part being written.
        field: &'static str,
        /// Buffer size the write needs, in bytes (`usize::MAX` when the
        /// size itself overflows).
        required: usize,
        /// Buffer size in bytes.
        available: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_error_length_overflow_display_includes_values() {
        let err = EncodeError::LengthOverflow {
            field: "var data field 'note'",
            length: 256,
            max: 255,
        };
        assert_eq!(
            err.to_string(),
            "var data field 'note': length 256 bytes exceeds the length header maximum 255"
        );
    }

    #[test]
    fn test_encode_error_out_of_order_display_includes_values() {
        let err = EncodeError::OutOfOrder {
            field: "var data field 'note'",
            written: 3,
        };
        assert_eq!(
            err.to_string(),
            "var data field 'note' written out of schema order (parts already written: 3)"
        );
    }

    #[test]
    fn test_encode_error_buffer_too_short_display_includes_values() {
        let err = EncodeError::BufferTooShort {
            field: "var data field 'note'",
            required: 40,
            available: 32,
        };
        assert_eq!(
            err.to_string(),
            "var data field 'note': buffer too short, required 40 bytes, available 32 bytes"
        );
    }
}
