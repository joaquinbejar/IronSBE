//! Decoder traits for SBE messages.
//!
//! This module provides the [`SbeDecoder`] trait for zero-copy message decoding.

use crate::header::MessageHeader;

/// Error type for decoding operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Buffer is too short for the message.
    BufferTooShort {
        /// Required buffer size in bytes.
        required: usize,
        /// Available buffer size in bytes.
        available: usize,
    },
    /// Template ID does not match expected value.
    TemplateMismatch {
        /// Expected template ID.
        expected: u16,
        /// Actual template ID found.
        actual: u16,
    },
    /// Schema ID does not match expected value.
    SchemaMismatch {
        /// Expected schema ID.
        expected: u16,
        /// Actual schema ID found.
        actual: u16,
    },
    /// Invalid enum value encountered.
    InvalidEnumValue {
        /// Field tag/ID.
        tag: u16,
        /// Invalid value.
        value: u64,
    },
    /// Invalid UTF-8 encoding.
    InvalidUtf8 {
        /// Byte offset where invalid UTF-8 was found.
        offset: usize,
    },
    /// Version is not supported.
    UnsupportedVersion {
        /// Message version.
        version: u16,
        /// Minimum supported version.
        min_supported: u16,
    },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BufferTooShort {
                required,
                available,
            } => {
                write!(
                    f,
                    "buffer too short: required {} bytes, available {} bytes",
                    required, available
                )
            }
            Self::TemplateMismatch { expected, actual } => {
                write!(
                    f,
                    "template mismatch: expected {}, actual {}",
                    expected, actual
                )
            }
            Self::SchemaMismatch { expected, actual } => {
                write!(
                    f,
                    "schema mismatch: expected {}, actual {}",
                    expected, actual
                )
            }
            Self::InvalidEnumValue { tag, value } => {
                write!(f, "invalid enum value: tag {}, value {}", tag, value)
            }
            Self::InvalidUtf8 { offset } => {
                write!(f, "invalid UTF-8 at offset {}", offset)
            }
            Self::UnsupportedVersion {
                version,
                min_supported,
            } => {
                write!(
                    f,
                    "unsupported version: {} (min supported: {})",
                    version, min_supported
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Trait for zero-copy SBE message decoders.
///
/// Implementations wrap a byte buffer and provide field accessors that
/// read directly from the buffer without copying data.
///
/// # Type Parameters
/// * `'a` - Lifetime of the underlying buffer
///
/// # Example
/// ```ignore
/// // Generated decoder usage
/// let decoder = NewOrderSingleDecoder::wrap(&buffer, 0, SCHEMA_VERSION);
/// let symbol = decoder.symbol();
/// let quantity = decoder.quantity();
/// ```
pub trait SbeDecoder<'a>: Sized {
    /// Schema template ID for this message type.
    const TEMPLATE_ID: u16;

    /// Schema ID.
    const SCHEMA_ID: u16;

    /// Schema version.
    const SCHEMA_VERSION: u16;

    /// Block length (fixed portion size in bytes).
    const BLOCK_LENGTH: u16;

    /// Wraps a buffer to decode a message (zero-copy).
    ///
    /// # Arguments
    /// * `buffer` - Byte buffer containing the message
    /// * `offset` - Byte offset where the message starts (after header)
    /// * `acting_version` - Version to use for decoding (for compatibility)
    ///
    /// # Returns
    /// A decoder instance wrapping the buffer.
    fn wrap(buffer: &'a [u8], offset: usize, acting_version: u16) -> Self;

    /// Returns the encoded length of the message in bytes.
    ///
    /// This includes the header and all fixed/variable portions.
    fn encoded_length(&self) -> usize;

    /// Validates that the message header matches the expected template.
    ///
    /// # Arguments
    /// * `header` - Message header to validate
    ///
    /// # Errors
    /// Returns an error if the template ID or schema ID doesn't match.
    fn validate_header(header: &MessageHeader) -> Result<(), DecodeError> {
        if header.template_id != Self::TEMPLATE_ID {
            return Err(DecodeError::TemplateMismatch {
                expected: Self::TEMPLATE_ID,
                actual: header.template_id,
            });
        }
        if header.schema_id != Self::SCHEMA_ID {
            return Err(DecodeError::SchemaMismatch {
                expected: Self::SCHEMA_ID,
                actual: header.schema_id,
            });
        }
        Ok(())
    }

    /// Decodes a message from a buffer, including header validation.
    ///
    /// # Arguments
    /// * `buffer` - Byte buffer containing the message (starting with header)
    ///
    /// # Errors
    /// Returns an error if the header is invalid or buffer is too short.
    fn decode(buffer: &'a [u8]) -> Result<Self, DecodeError> {
        if buffer.len() < MessageHeader::ENCODED_LENGTH {
            return Err(DecodeError::BufferTooShort {
                required: MessageHeader::ENCODED_LENGTH,
                available: buffer.len(),
            });
        }

        let header = MessageHeader::wrap(buffer, 0);
        Self::validate_header(&header)?;

        let required_len = MessageHeader::ENCODED_LENGTH + header.block_length as usize;
        if buffer.len() < required_len {
            return Err(DecodeError::BufferTooShort {
                required: required_len,
                available: buffer.len(),
            });
        }

        Ok(Self::wrap(
            buffer,
            MessageHeader::ENCODED_LENGTH,
            header.version,
        ))
    }
}

/// Trait for message dispatching based on template ID.
///
/// This trait allows routing messages to appropriate handlers based on
/// the template ID in the message header.
pub trait MessageDispatch {
    /// Dispatches a message to the appropriate handler.
    ///
    /// # Arguments
    /// * `header` - Message header
    /// * `buffer` - Full message buffer (including header)
    ///
    /// # Returns
    /// Result indicating success or failure of dispatch.
    fn dispatch(&self, header: &MessageHeader, buffer: &[u8]) -> Result<(), DecodeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_error_display() {
        let err = DecodeError::BufferTooShort {
            required: 100,
            available: 50,
        };
        assert!(err.to_string().contains("100"));
        assert!(err.to_string().contains("50"));

        let err = DecodeError::TemplateMismatch {
            expected: 1,
            actual: 2,
        };
        assert!(err.to_string().contains("expected 1"));
        assert!(err.to_string().contains("actual 2"));
    }

    #[test]
    fn test_decode_error_equality() {
        let err1 = DecodeError::TemplateMismatch {
            expected: 1,
            actual: 2,
        };
        let err2 = DecodeError::TemplateMismatch {
            expected: 1,
            actual: 2,
        };
        assert_eq!(err1, err2);
    }
}
