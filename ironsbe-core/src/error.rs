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

/// Error returned by the generated `try_set_<field>` var data setters.
///
/// Every check runs before anything is written, so on error the buffer
/// and the encoder's write cursor are left unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VarDataError {
    /// The value is longer than the field's length header can encode.
    #[error(
        "var data field '{field}': length {length} bytes exceeds the length header maximum {max}"
    )]
    LengthOverflow {
        /// Schema name of the var data field.
        field: &'static str,
        /// Length of the rejected value in bytes.
        length: usize,
        /// Largest length the header can encode, in bytes.
        max: u64,
    },

    /// The field, or a part after it, was already written.
    #[error(
        "var data field '{field}' written out of schema order (parts already written: {written})"
    )]
    OutOfOrder {
        /// Schema name of the var data field.
        field: &'static str,
        /// Number of variable parts already written on the owner.
        written: u16,
    },

    /// The buffer cannot hold the empty headers of skipped parts plus
    /// this field's header and value.
    #[error(
        "var data field '{field}': buffer too short, required {required} bytes, available {available} bytes"
    )]
    BufferTooShort {
        /// Schema name of the var data field.
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
    fn test_var_data_error_length_overflow_display_includes_values() {
        let err = VarDataError::LengthOverflow {
            field: "note",
            length: 256,
            max: 255,
        };
        assert_eq!(
            err.to_string(),
            "var data field 'note': length 256 bytes exceeds the length header maximum 255"
        );
    }

    #[test]
    fn test_var_data_error_out_of_order_display_includes_values() {
        let err = VarDataError::OutOfOrder {
            field: "note",
            written: 3,
        };
        assert_eq!(
            err.to_string(),
            "var data field 'note' written out of schema order (parts already written: 3)"
        );
    }

    #[test]
    fn test_var_data_error_buffer_too_short_display_includes_values() {
        let err = VarDataError::BufferTooShort {
            field: "note",
            required: 40,
            available: 32,
        };
        assert_eq!(
            err.to_string(),
            "var data field 'note': buffer too short, required 40 bytes, available 32 bytes"
        );
    }
}
