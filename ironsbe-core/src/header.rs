//! SBE message header types.
//!
//! This module provides the standard SBE header structures:
//! - [`MessageHeader`] - 8-byte message header
//! - [`GroupHeader`] - 4-byte repeating group header
//! - [`VarDataHeader`] - Variable-length data header

use crate::buffer::{ReadBuffer, WriteBuffer};

/// Standard SBE message header (8 bytes).
///
/// The message header precedes every SBE message and contains:
/// - Block length: Size of the root block in bytes
/// - Template ID: Message type identifier
/// - Schema ID: Schema identifier
/// - Version: Schema version number
///
/// # Wire Format
/// ```text
/// +0: blockLength  (u16, 2 bytes)
/// +2: templateId   (u16, 2 bytes)
/// +4: schemaId     (u16, 2 bytes)
/// +6: version      (u16, 2 bytes)
/// ```
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MessageHeader {
    /// Length of the root block in bytes.
    pub block_length: u16,
    /// Message template identifier.
    pub template_id: u16,
    /// Schema identifier.
    pub schema_id: u16,
    /// Schema version number.
    pub version: u16,
}

impl MessageHeader {
    /// Encoded length of the message header in bytes.
    pub const ENCODED_LENGTH: usize = 8;

    /// Creates a new message header with the specified values.
    ///
    /// # Arguments
    /// * `block_length` - Length of the root block in bytes
    /// * `template_id` - Message template identifier
    /// * `schema_id` - Schema identifier
    /// * `version` - Schema version number
    #[must_use]
    pub const fn new(block_length: u16, template_id: u16, schema_id: u16, version: u16) -> Self {
        Self {
            block_length,
            template_id,
            schema_id,
            version,
        }
    }

    /// Wraps a buffer and decodes the message header at the given offset.
    ///
    /// # Arguments
    /// * `buffer` - Buffer to read from
    /// * `offset` - Byte offset to start reading
    ///
    /// # Panics
    /// Panics if the buffer is too short.
    #[inline(always)]
    #[must_use]
    pub fn wrap<B: ReadBuffer + ?Sized>(buffer: &B, offset: usize) -> Self {
        Self {
            block_length: buffer.get_u16_le(offset),
            template_id: buffer.get_u16_le(offset + 2),
            schema_id: buffer.get_u16_le(offset + 4),
            version: buffer.get_u16_le(offset + 6),
        }
    }

    /// Encodes the message header to the buffer at the given offset.
    ///
    /// # Arguments
    /// * `buffer` - Buffer to write to
    /// * `offset` - Byte offset to start writing
    #[inline(always)]
    pub fn encode<B: WriteBuffer + ?Sized>(&self, buffer: &mut B, offset: usize) {
        buffer.put_u16_le(offset, self.block_length);
        buffer.put_u16_le(offset + 2, self.template_id);
        buffer.put_u16_le(offset + 4, self.schema_id);
        buffer.put_u16_le(offset + 6, self.version);
    }

    /// Returns the total message size (header + block).
    #[must_use]
    pub const fn message_size(&self) -> usize {
        Self::ENCODED_LENGTH + self.block_length as usize
    }
}

/// Repeating group header (4 bytes).
///
/// The group header precedes each repeating group and contains:
/// - Block length: Size of each group entry in bytes
/// - Num in group: Number of entries in the group
///
/// # Wire Format
/// ```text
/// +0: blockLength  (u16, 2 bytes)
/// +2: numInGroup   (u16, 2 bytes)
/// ```
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GroupHeader {
    /// Length of each group entry in bytes.
    pub block_length: u16,
    /// Number of entries in the group.
    pub num_in_group: u16,
}

impl GroupHeader {
    /// Encoded length of the group header in bytes.
    pub const ENCODED_LENGTH: usize = 4;

    /// Creates a new group header with the specified values.
    ///
    /// # Arguments
    /// * `block_length` - Length of each group entry in bytes
    /// * `num_in_group` - Number of entries in the group
    #[must_use]
    pub const fn new(block_length: u16, num_in_group: u16) -> Self {
        Self {
            block_length,
            num_in_group,
        }
    }

    /// Wraps a buffer and decodes the group header at the given offset.
    ///
    /// # Arguments
    /// * `buffer` - Buffer to read from
    /// * `offset` - Byte offset to start reading
    ///
    /// # Panics
    /// Panics if the buffer is too short.
    #[inline(always)]
    #[must_use]
    pub fn wrap<B: ReadBuffer + ?Sized>(buffer: &B, offset: usize) -> Self {
        Self {
            block_length: buffer.get_u16_le(offset),
            num_in_group: buffer.get_u16_le(offset + 2),
        }
    }

    /// Encodes the group header to the buffer at the given offset.
    ///
    /// # Arguments
    /// * `buffer` - Buffer to write to
    /// * `offset` - Byte offset to start writing
    #[inline(always)]
    pub fn encode<B: WriteBuffer + ?Sized>(&self, buffer: &mut B, offset: usize) {
        buffer.put_u16_le(offset, self.block_length);
        buffer.put_u16_le(offset + 2, self.num_in_group);
    }

    /// Returns the total size of the group (header + all entries).
    #[must_use]
    pub const fn group_size(&self) -> usize {
        Self::ENCODED_LENGTH + (self.block_length as usize * self.num_in_group as usize)
    }

    /// Returns true if the group is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.num_in_group == 0
    }
}

/// Variable-length data header.
///
/// The var data header precedes variable-length data fields and contains
/// the length of the data that follows.
///
/// # Wire Format
/// ```text
/// +0: length (u16, 2 bytes) - for standard varDataEncoding
/// ```
///
/// Note: Some schemas may use u8 or u32 for the length field.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VarDataHeader {
    /// Length of the variable data in bytes.
    pub length: u16,
}

impl VarDataHeader {
    /// Encoded length of the var data header in bytes (u16 length).
    pub const ENCODED_LENGTH: usize = 2;

    /// Creates a new var data header with the specified length.
    ///
    /// # Arguments
    /// * `length` - Length of the variable data in bytes
    #[must_use]
    pub const fn new(length: u16) -> Self {
        Self { length }
    }

    /// Wraps a buffer and decodes the var data header at the given offset.
    ///
    /// # Arguments
    /// * `buffer` - Buffer to read from
    /// * `offset` - Byte offset to start reading
    ///
    /// # Panics
    /// Panics if the buffer is too short.
    #[inline(always)]
    #[must_use]
    pub fn wrap<B: ReadBuffer + ?Sized>(buffer: &B, offset: usize) -> Self {
        Self {
            length: buffer.get_u16_le(offset),
        }
    }

    /// Encodes the var data header to the buffer at the given offset.
    ///
    /// # Arguments
    /// * `buffer` - Buffer to write to
    /// * `offset` - Byte offset to start writing
    #[inline(always)]
    pub fn encode<B: WriteBuffer + ?Sized>(&self, buffer: &mut B, offset: usize) {
        buffer.put_u16_le(offset, self.length);
    }

    /// Returns the total size (header + data).
    #[must_use]
    pub const fn total_size(&self) -> usize {
        Self::ENCODED_LENGTH + self.length as usize
    }

    /// Returns true if the data is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }
}

/// Variable-length data header with u8 length (1 byte).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarDataHeader8 {
    /// Length of the variable data in bytes.
    pub length: u8,
}

impl VarDataHeader8 {
    /// Encoded length of the var data header in bytes.
    pub const ENCODED_LENGTH: usize = 1;

    /// Creates a new var data header with the specified length.
    #[must_use]
    pub const fn new(length: u8) -> Self {
        Self { length }
    }

    /// Wraps a buffer and decodes the var data header at the given offset.
    #[inline(always)]
    #[must_use]
    pub fn wrap<B: ReadBuffer + ?Sized>(buffer: &B, offset: usize) -> Self {
        Self {
            length: buffer.get_u8(offset),
        }
    }

    /// Encodes the var data header to the buffer at the given offset.
    #[inline(always)]
    pub fn encode<B: WriteBuffer + ?Sized>(&self, buffer: &mut B, offset: usize) {
        buffer.put_u8(offset, self.length);
    }
}

/// Variable-length data header with u32 length (4 bytes).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarDataHeader32 {
    /// Length of the variable data in bytes.
    pub length: u32,
}

impl VarDataHeader32 {
    /// Encoded length of the var data header in bytes.
    pub const ENCODED_LENGTH: usize = 4;

    /// Creates a new var data header with the specified length.
    #[must_use]
    pub const fn new(length: u32) -> Self {
        Self { length }
    }

    /// Wraps a buffer and decodes the var data header at the given offset.
    #[inline(always)]
    #[must_use]
    pub fn wrap<B: ReadBuffer + ?Sized>(buffer: &B, offset: usize) -> Self {
        Self {
            length: buffer.get_u32_le(offset),
        }
    }

    /// Encodes the var data header to the buffer at the given offset.
    #[inline(always)]
    pub fn encode<B: WriteBuffer + ?Sized>(&self, buffer: &mut B, offset: usize) {
        buffer.put_u32_le(offset, self.length);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::AlignedBuffer;

    #[test]
    fn test_message_header_encode_decode() {
        let mut buf: AlignedBuffer<16> = AlignedBuffer::new();
        let header = MessageHeader::new(64, 1, 100, 1);

        header.encode(&mut buf, 0);
        let decoded = MessageHeader::wrap(&buf, 0);

        assert_eq!(header, decoded);
        assert_eq!({ decoded.block_length }, 64);
        assert_eq!({ decoded.template_id }, 1);
        assert_eq!({ decoded.schema_id }, 100);
        assert_eq!({ decoded.version }, 1);
    }

    #[test]
    fn test_message_header_size() {
        assert_eq!(MessageHeader::ENCODED_LENGTH, 8);
        let header = MessageHeader::new(64, 1, 100, 1);
        assert_eq!(header.message_size(), 72);
    }

    #[test]
    fn test_group_header_encode_decode() {
        let mut buf: AlignedBuffer<16> = AlignedBuffer::new();
        let header = GroupHeader::new(32, 5);

        header.encode(&mut buf, 0);
        let decoded = GroupHeader::wrap(&buf, 0);

        assert_eq!(header, decoded);
        assert_eq!({ decoded.block_length }, 32);
        assert_eq!({ decoded.num_in_group }, 5);
    }

    #[test]
    fn test_group_header_size() {
        assert_eq!(GroupHeader::ENCODED_LENGTH, 4);
        let header = GroupHeader::new(32, 5);
        assert_eq!(header.group_size(), 4 + 32 * 5);
        assert!(!header.is_empty());

        let empty = GroupHeader::new(32, 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn test_var_data_header_encode_decode() {
        let mut buf: AlignedBuffer<16> = AlignedBuffer::new();
        let header = VarDataHeader::new(256);

        header.encode(&mut buf, 0);
        let decoded = VarDataHeader::wrap(&buf, 0);

        assert_eq!(header, decoded);
        assert_eq!({ decoded.length }, 256);
        assert_eq!(decoded.total_size(), 258);
    }

    #[test]
    fn test_var_data_header8() {
        let mut buf: AlignedBuffer<16> = AlignedBuffer::new();
        let header = VarDataHeader8::new(100);

        header.encode(&mut buf, 0);
        let decoded = VarDataHeader8::wrap(&buf, 0);

        assert_eq!(header, decoded);
        assert_eq!(VarDataHeader8::ENCODED_LENGTH, 1);
    }

    #[test]
    fn test_var_data_header32() {
        let mut buf: AlignedBuffer<16> = AlignedBuffer::new();
        let header = VarDataHeader32::new(1_000_000);

        header.encode(&mut buf, 0);
        let decoded = VarDataHeader32::wrap(&buf, 0);

        assert_eq!(header, decoded);
        assert_eq!(VarDataHeader32::ENCODED_LENGTH, 4);
    }

    #[test]
    fn test_header_wire_format() {
        let mut buf: AlignedBuffer<16> = AlignedBuffer::new();
        let header = MessageHeader::new(0x0102, 0x0304, 0x0506, 0x0708);
        header.encode(&mut buf, 0);

        // Verify little-endian encoding
        assert_eq!(buf.get_u8(0), 0x02); // block_length low byte
        assert_eq!(buf.get_u8(1), 0x01); // block_length high byte
        assert_eq!(buf.get_u8(2), 0x04); // template_id low byte
        assert_eq!(buf.get_u8(3), 0x03); // template_id high byte
        assert_eq!(buf.get_u8(4), 0x06); // schema_id low byte
        assert_eq!(buf.get_u8(5), 0x05); // schema_id high byte
        assert_eq!(buf.get_u8(6), 0x08); // version low byte
        assert_eq!(buf.get_u8(7), 0x07); // version high byte
    }
}
