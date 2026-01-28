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
    use crate::buffer::{AlignedBuffer, ReadBuffer};

    #[test]
    fn test_decode_error_display_buffer_too_short() {
        let err = DecodeError::BufferTooShort {
            required: 100,
            available: 50,
        };
        let msg = err.to_string();
        assert!(msg.contains("100"));
        assert!(msg.contains("50"));
        assert!(msg.contains("buffer too short"));
    }

    #[test]
    fn test_decode_error_display_template_mismatch() {
        let err = DecodeError::TemplateMismatch {
            expected: 1,
            actual: 2,
        };
        let msg = err.to_string();
        assert!(msg.contains("expected 1"));
        assert!(msg.contains("actual 2"));
        assert!(msg.contains("template mismatch"));
    }

    #[test]
    fn test_decode_error_display_schema_mismatch() {
        let err = DecodeError::SchemaMismatch {
            expected: 100,
            actual: 200,
        };
        let msg = err.to_string();
        assert!(msg.contains("100"));
        assert!(msg.contains("200"));
        assert!(msg.contains("schema mismatch"));
    }

    #[test]
    fn test_decode_error_display_invalid_enum() {
        let err = DecodeError::InvalidEnumValue { tag: 55, value: 99 };
        let msg = err.to_string();
        assert!(msg.contains("55"));
        assert!(msg.contains("99"));
        assert!(msg.contains("invalid enum"));
    }

    #[test]
    fn test_decode_error_display_invalid_utf8() {
        let err = DecodeError::InvalidUtf8 { offset: 42 };
        let msg = err.to_string();
        assert!(msg.contains("42"));
        assert!(msg.contains("UTF-8"));
    }

    #[test]
    fn test_decode_error_display_unsupported_version() {
        let err = DecodeError::UnsupportedVersion {
            version: 5,
            min_supported: 1,
        };
        let msg = err.to_string();
        assert!(msg.contains("5"));
        assert!(msg.contains("1"));
        assert!(msg.contains("unsupported version"));
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

        let err3 = DecodeError::TemplateMismatch {
            expected: 1,
            actual: 3,
        };
        assert_ne!(err1, err3);
    }

    #[test]
    fn test_decode_error_clone() {
        let err = DecodeError::BufferTooShort {
            required: 100,
            available: 50,
        };
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn test_decode_error_debug() {
        let err = DecodeError::InvalidEnumValue { tag: 1, value: 2 };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("InvalidEnumValue"));
    }

    /// Test decoder implementation for testing purposes.
    struct TestDecoder<'a> {
        buffer: &'a [u8],
        offset: usize,
        #[allow(dead_code)]
        acting_version: u16,
    }

    impl<'a> SbeDecoder<'a> for TestDecoder<'a> {
        const TEMPLATE_ID: u16 = 1;
        const SCHEMA_ID: u16 = 100;
        const SCHEMA_VERSION: u16 = 1;
        const BLOCK_LENGTH: u16 = 16;

        fn wrap(buffer: &'a [u8], offset: usize, acting_version: u16) -> Self {
            Self {
                buffer,
                offset,
                acting_version,
            }
        }

        fn encoded_length(&self) -> usize {
            MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize
        }
    }

    #[test]
    fn test_validate_header_success() {
        let header = MessageHeader::new(16, 1, 100, 1);
        assert!(TestDecoder::validate_header(&header).is_ok());
    }

    #[test]
    fn test_validate_header_template_mismatch() {
        let header = MessageHeader::new(16, 99, 100, 1);
        let result = TestDecoder::validate_header(&header);
        assert!(matches!(result, Err(DecodeError::TemplateMismatch { .. })));
    }

    #[test]
    fn test_validate_header_schema_mismatch() {
        let header = MessageHeader::new(16, 1, 999, 1);
        let result = TestDecoder::validate_header(&header);
        assert!(matches!(result, Err(DecodeError::SchemaMismatch { .. })));
    }

    #[test]
    fn test_decode_buffer_too_short_for_header() {
        let buffer = [0u8; 4]; // Less than header size
        let result = TestDecoder::decode(&buffer);
        assert!(matches!(result, Err(DecodeError::BufferTooShort { .. })));
    }

    #[test]
    fn test_decode_buffer_too_short_for_message() {
        let mut buffer = AlignedBuffer::<16>::new();
        let header = MessageHeader::new(100, 1, 100, 1); // block_length = 100
        header.encode(&mut buffer, 0);

        let result = TestDecoder::decode(buffer.as_slice());
        assert!(matches!(result, Err(DecodeError::BufferTooShort { .. })));
    }

    #[test]
    fn test_decode_success() {
        let mut buffer = AlignedBuffer::<32>::new();
        let header = MessageHeader::new(16, 1, 100, 1);
        header.encode(&mut buffer, 0);

        let result = TestDecoder::decode(buffer.as_slice());
        assert!(result.is_ok());
        let decoder = result.unwrap();
        assert_eq!(decoder.encoded_length(), 24); // 8 header + 16 block
    }

    #[test]
    fn test_decoder_wrap() {
        let buffer = [0u8; 32];
        let decoder = TestDecoder::wrap(&buffer, 8, 1);
        assert_eq!(decoder.offset, 8);
        assert_eq!(decoder.buffer.len(), 32);
    }
}
