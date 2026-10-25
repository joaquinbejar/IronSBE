//! Decoder traits for SBE messages.
//!
//! This module provides the [`SbeDecoder`] trait for zero-copy message decoding.

use crate::header::MessageHeader;

/// Error type for decoding operations.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// The header declares a block shorter than the fields of its own version.
    ///
    /// The declared block length is untrusted input; accepting it would let a
    /// field accessor read past the block. Older versions legitimately declare
    /// shorter blocks, which is why the requirement depends on the version.
    BlockLengthTooSmall {
        /// Block length the message version requires.
        required: u16,
        /// Block length the header declared.
        declared: u16,
        /// Version the header declared (clamped to the decoder's schema version).
        version: u16,
    },
    /// A repeating group, group entry or var data field extends past the
    /// end of the buffer. Its lengths come from the wire and are untrusted.
    VarPartOutOfBounds {
        /// The part, e.g. `var data field 'label'` or `repeating group 'legs'`.
        part: &'static str,
        /// Buffer size the part needs, in bytes (`usize::MAX` when the
        /// size itself overflows).
        required: usize,
        /// Buffer size in bytes.
        available: usize,
    },
    /// A sequential reader was asked for a part it has already read past.
    OutOfOrder {
        /// The part that was requested.
        part: &'static str,
        /// Number of variable parts already consumed by the reader.
        read: u16,
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
            Self::BlockLengthTooSmall {
                required,
                declared,
                version,
            } => {
                write!(
                    f,
                    "block length too small: version {} requires {} bytes, header declares {}",
                    version, required, declared
                )
            }
            Self::VarPartOutOfBounds {
                part,
                required,
                available,
            } => {
                write!(
                    f,
                    "{} extends past the end of the buffer: required {} bytes, available {} bytes",
                    part, required, available
                )
            }
            Self::OutOfOrder { part, read } => {
                write!(
                    f,
                    "{} read out of schema order (parts already read: {})",
                    part, read
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

    /// Block length (fixed portion size in bytes) of the current schema version.
    const BLOCK_LENGTH: u16;

    /// Smallest block length a message of `acting_version` can carry: the end
    /// of the last fixed field whose `sinceVersion` is at most that version.
    ///
    /// Generated decoders override this from the schema so that a message
    /// encoded by an older version (shorter block) still decodes, while a
    /// header claiming the current version with a truncated block is rejected
    /// before any accessor can read past it. The default is the conservative
    /// answer for hand-written decoders: the whole current block.
    #[must_use]
    fn required_block_length(acting_version: u16) -> u16 {
        let _ = acting_version;
        Self::BLOCK_LENGTH
    }

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

    /// Wraps a buffer whose header declared `acting_block_length` for the
    /// fixed block.
    ///
    /// Generated decoders store the value and start repeating groups and var
    /// data after it, so a message encoded by an older version (shorter
    /// block) is walked where the wire says, not where the current schema
    /// would put it. The default ignores the value; hand-written decoders
    /// that only read fixed fields need nothing more.
    #[must_use]
    fn wrap_acting(
        buffer: &'a [u8],
        offset: usize,
        acting_version: u16,
        acting_block_length: u16,
    ) -> Self {
        let _ = acting_block_length;
        Self::wrap(buffer, offset, acting_version)
    }

    /// Returns the encoded length of the message in bytes.
    ///
    /// This includes the header and all fixed/variable portions.
    ///
    /// # Errors
    /// Returns [`DecodeError::VarPartOutOfBounds`] when a repeating group or
    /// var data field claims more bytes than the buffer holds.
    fn encoded_length(&self) -> Result<usize, DecodeError>;

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
    /// # Guarantee
    /// A successful `decode` guarantees the header and the **root block** are
    /// present and consistent with the message's version, so every fixed
    /// field accessor is in bounds. Repeating groups and var data are read
    /// lazily and their lengths come from the wire; generated accessors for
    /// them check every length against the buffer and return
    /// [`DecodeError::VarPartOutOfBounds`] instead of reading past it.
    ///
    /// # Errors
    /// Returns an error if the header is invalid, the declared block is shorter
    /// than the fields its version requires, or the buffer is too short.
    fn decode(buffer: &'a [u8]) -> Result<Self, DecodeError> {
        if buffer.len() < MessageHeader::ENCODED_LENGTH {
            return Err(DecodeError::BufferTooShort {
                required: MessageHeader::ENCODED_LENGTH,
                available: buffer.len(),
            });
        }

        let header = MessageHeader::wrap(buffer, 0);
        Self::validate_header(&header)?;

        // A newer schema version only appends fields, so its block covers ours;
        // validate against the newest version this decoder knows.
        let acting_version = header.version.min(Self::SCHEMA_VERSION);
        let required_block = Self::required_block_length(acting_version);
        if header.block_length < required_block {
            return Err(DecodeError::BlockLengthTooSmall {
                required: required_block,
                declared: header.block_length,
                version: acting_version,
            });
        }

        let required_len = MessageHeader::ENCODED_LENGTH + header.block_length as usize;
        if buffer.len() < required_len {
            return Err(DecodeError::BufferTooShort {
                required: required_len,
                available: buffer.len(),
            });
        }

        Ok(Self::wrap_acting(
            buffer,
            MessageHeader::ENCODED_LENGTH,
            header.version,
            header.block_length,
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

        fn encoded_length(&self) -> Result<usize, DecodeError> {
            Ok(MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize)
        }
    }

    /// Decoder with a versioned layout: v1 has one 8-byte field, v2 appends another.
    struct VersionedDecoder<'a> {
        #[allow(dead_code)]
        buffer: &'a [u8],
    }

    impl<'a> SbeDecoder<'a> for VersionedDecoder<'a> {
        const TEMPLATE_ID: u16 = 1;
        const SCHEMA_ID: u16 = 100;
        const SCHEMA_VERSION: u16 = 2;
        const BLOCK_LENGTH: u16 = 16;

        fn required_block_length(acting_version: u16) -> u16 {
            if acting_version >= 2 { 16 } else { 8 }
        }

        fn wrap(buffer: &'a [u8], _offset: usize, _acting_version: u16) -> Self {
            Self { buffer }
        }

        fn encoded_length(&self) -> Result<usize, DecodeError> {
            Ok(MessageHeader::ENCODED_LENGTH + Self::BLOCK_LENGTH as usize)
        }
    }

    fn header_bytes(block_length: u16, version: u16) -> Vec<u8> {
        let mut h = vec![0u8; MessageHeader::ENCODED_LENGTH];
        h[0..2].copy_from_slice(&block_length.to_le_bytes());
        h[2..4].copy_from_slice(&1u16.to_le_bytes());
        h[4..6].copy_from_slice(&100u16.to_le_bytes());
        h[6..8].copy_from_slice(&version.to_le_bytes());
        h
    }

    #[test]
    fn test_decode_error_var_part_out_of_bounds_display_includes_values() {
        let err = DecodeError::VarPartOutOfBounds {
            part: "var data field 'label'",
            required: 40,
            available: 32,
        };
        assert_eq!(
            err.to_string(),
            "var data field 'label' extends past the end of the buffer: required 40 bytes, available 32 bytes"
        );
    }

    #[test]
    fn test_decode_error_out_of_order_display_includes_values() {
        let err = DecodeError::OutOfOrder {
            part: "repeating group 'legs'",
            read: 2,
        };
        assert_eq!(
            err.to_string(),
            "repeating group 'legs' read out of schema order (parts already read: 2)"
        );
    }

    #[test]
    fn test_decode_truncated_block_for_current_version_is_rejected() {
        // Header only: block_length = 0, version = current. Previously accepted,
        // and the first accessor then read out of bounds.
        let bytes = header_bytes(0, 1);
        assert_eq!(
            TestDecoder::decode(&bytes).err(),
            Some(DecodeError::BlockLengthTooSmall {
                required: 16,
                declared: 0,
                version: 1,
            })
        );
    }

    #[test]
    fn test_decode_older_version_with_its_own_block_is_accepted() {
        let mut bytes = header_bytes(8, 1);
        bytes.extend_from_slice(&[0u8; 8]);
        assert!(VersionedDecoder::decode(&bytes).is_ok());
    }

    #[test]
    fn test_decode_current_version_with_older_block_is_rejected() {
        let mut bytes = header_bytes(8, 2);
        bytes.extend_from_slice(&[0u8; 8]);
        assert_eq!(
            VersionedDecoder::decode(&bytes).err(),
            Some(DecodeError::BlockLengthTooSmall {
                required: 16,
                declared: 8,
                version: 2,
            })
        );
    }

    #[test]
    fn test_decode_newer_version_is_validated_against_known_schema() {
        // Version 3 is unknown to the decoder; its block must at least cover v2.
        let mut bytes = header_bytes(24, 3);
        bytes.extend_from_slice(&[0u8; 24]);
        assert!(VersionedDecoder::decode(&bytes).is_ok());
        let short = header_bytes(8, 3);
        assert!(matches!(
            VersionedDecoder::decode(&short),
            Err(DecodeError::BlockLengthTooSmall { version: 2, .. })
        ));
    }

    #[test]
    fn test_decode_declared_block_beyond_buffer_is_too_short() {
        let bytes = header_bytes(16, 1);
        assert_eq!(
            TestDecoder::decode(&bytes).err(),
            Some(DecodeError::BufferTooShort {
                required: 24,
                available: 8,
            })
        );
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
        assert_eq!(decoder.encoded_length(), Ok(24)); // 8 header + 16 block
    }

    #[test]
    fn test_decoder_wrap() {
        let buffer = [0u8; 32];
        let decoder = TestDecoder::wrap(&buffer, 8, 1);
        assert_eq!(decoder.offset, 8);
        assert_eq!(decoder.buffer.len(), 32);
    }
}
