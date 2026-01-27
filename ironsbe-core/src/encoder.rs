//! Encoder traits for SBE messages.
//!
//! This module provides the [`SbeEncoder`] trait for message encoding.

use crate::header::MessageHeader;

/// Trait for SBE message encoders.
///
/// Implementations wrap a mutable byte buffer and provide field setters
/// that write directly to the buffer.
///
/// # Example
/// ```ignore
/// // Generated encoder usage
/// let mut buffer = [0u8; 256];
/// let mut encoder = NewOrderSingleEncoder::wrap(&mut buffer, 0);
/// encoder
///     .set_symbol(b"AAPL    ")
///     .set_quantity(100)
///     .set_price_mantissa(15050)
///     .set_price_exponent(-2);
/// let len = encoder.encoded_length();
/// ```
pub trait SbeEncoder: Sized {
    /// Schema template ID for this message type.
    const TEMPLATE_ID: u16;

    /// Schema ID.
    const SCHEMA_ID: u16;

    /// Schema version.
    const SCHEMA_VERSION: u16;

    /// Block length (fixed portion size in bytes).
    const BLOCK_LENGTH: u16;

    /// Wraps a mutable buffer for encoding.
    ///
    /// This automatically writes the message header.
    ///
    /// # Arguments
    /// * `buffer` - Mutable byte buffer to write to
    /// * `offset` - Byte offset where the message starts
    ///
    /// # Returns
    /// An encoder instance wrapping the buffer.
    fn wrap(buffer: &mut [u8], offset: usize) -> Self;

    /// Returns the final encoded length after all writes.
    ///
    /// This includes the header and all written portions.
    fn encoded_length(&self) -> usize;

    /// Creates the message header for this encoder.
    #[must_use]
    fn create_header() -> MessageHeader {
        MessageHeader {
            block_length: Self::BLOCK_LENGTH,
            template_id: Self::TEMPLATE_ID,
            schema_id: Self::SCHEMA_ID,
            version: Self::SCHEMA_VERSION,
        }
    }
}

/// Helper struct for building encoded messages.
///
/// Provides a convenient way to track the current write position
/// and manage buffer space.
#[derive(Debug)]
pub struct EncoderBuffer<'a> {
    buffer: &'a mut [u8],
    offset: usize,
    position: usize,
}

impl<'a> EncoderBuffer<'a> {
    /// Creates a new encoder buffer.
    ///
    /// # Arguments
    /// * `buffer` - Mutable byte buffer to write to
    /// * `offset` - Starting offset in the buffer
    #[must_use]
    pub fn new(buffer: &'a mut [u8], offset: usize) -> Self {
        Self {
            buffer,
            offset,
            position: offset,
        }
    }

    /// Returns the underlying buffer.
    #[must_use]
    pub fn buffer(&self) -> &[u8] {
        self.buffer
    }

    /// Returns the mutable underlying buffer.
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        self.buffer
    }

    /// Returns the starting offset.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the current write position.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Returns the number of bytes written.
    #[must_use]
    pub const fn bytes_written(&self) -> usize {
        self.position - self.offset
    }

    /// Returns the remaining capacity.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buffer.len() - self.position
    }

    /// Advances the position by the given number of bytes.
    ///
    /// # Arguments
    /// * `count` - Number of bytes to advance
    pub fn advance(&mut self, count: usize) {
        self.position += count;
    }

    /// Sets the position to a specific offset.
    ///
    /// # Arguments
    /// * `pos` - New position
    pub fn set_position(&mut self, pos: usize) {
        self.position = pos;
    }

    /// Writes a u8 at the current position and advances.
    pub fn write_u8(&mut self, value: u8) {
        self.buffer[self.position] = value;
        self.position += 1;
    }

    /// Writes a u16 in little-endian at the current position and advances.
    pub fn write_u16_le(&mut self, value: u16) {
        let bytes = value.to_le_bytes();
        self.buffer[self.position..self.position + 2].copy_from_slice(&bytes);
        self.position += 2;
    }

    /// Writes a u32 in little-endian at the current position and advances.
    pub fn write_u32_le(&mut self, value: u32) {
        let bytes = value.to_le_bytes();
        self.buffer[self.position..self.position + 4].copy_from_slice(&bytes);
        self.position += 4;
    }

    /// Writes a u64 in little-endian at the current position and advances.
    pub fn write_u64_le(&mut self, value: u64) {
        let bytes = value.to_le_bytes();
        self.buffer[self.position..self.position + 8].copy_from_slice(&bytes);
        self.position += 8;
    }

    /// Writes bytes at the current position and advances.
    pub fn write_bytes(&mut self, data: &[u8]) {
        self.buffer[self.position..self.position + data.len()].copy_from_slice(data);
        self.position += data.len();
    }

    /// Writes zeros at the current position and advances.
    pub fn write_zeros(&mut self, count: usize) {
        self.buffer[self.position..self.position + count].fill(0);
        self.position += count;
    }
}

/// Trait for group encoders.
///
/// Group encoders handle the encoding of repeating groups within messages.
pub trait GroupEncoder {
    /// The entry encoder type for this group.
    type Entry<'a>
    where
        Self: 'a;

    /// Returns the number of entries that will be written.
    fn count(&self) -> u16;

    /// Begins encoding a new entry.
    ///
    /// # Returns
    /// An entry encoder for the new entry.
    fn next_entry(&mut self) -> Option<Self::Entry<'_>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_buffer_basic() {
        let mut buf = [0u8; 64];
        let mut encoder = EncoderBuffer::new(&mut buf, 0);

        assert_eq!(encoder.offset(), 0);
        assert_eq!(encoder.position(), 0);
        assert_eq!(encoder.bytes_written(), 0);
        assert_eq!(encoder.remaining(), 64);

        encoder.write_u8(0xFF);
        assert_eq!(encoder.position(), 1);
        assert_eq!(encoder.bytes_written(), 1);

        encoder.write_u16_le(0x1234);
        assert_eq!(encoder.position(), 3);

        encoder.write_u32_le(0xDEADBEEF);
        assert_eq!(encoder.position(), 7);

        encoder.write_u64_le(0x123456789ABCDEF0);
        assert_eq!(encoder.position(), 15);
    }

    #[test]
    fn test_encoder_buffer_with_offset() {
        let mut buf = [0u8; 64];
        let mut encoder = EncoderBuffer::new(&mut buf, 8);

        assert_eq!(encoder.offset(), 8);
        assert_eq!(encoder.position(), 8);
        assert_eq!(encoder.bytes_written(), 0);

        encoder.write_u32_le(0x12345678);
        assert_eq!(encoder.position(), 12);
        assert_eq!(encoder.bytes_written(), 4);

        // Verify data was written at correct offset
        assert_eq!(buf[8], 0x78);
        assert_eq!(buf[9], 0x56);
        assert_eq!(buf[10], 0x34);
        assert_eq!(buf[11], 0x12);
    }

    #[test]
    fn test_encoder_buffer_write_bytes() {
        let mut buf = [0u8; 64];
        let mut encoder = EncoderBuffer::new(&mut buf, 0);

        encoder.write_bytes(b"Hello");
        assert_eq!(encoder.position(), 5);
        assert_eq!(&buf[0..5], b"Hello");
    }

    #[test]
    fn test_encoder_buffer_write_zeros() {
        let mut buf = [0xFFu8; 64];
        let mut encoder = EncoderBuffer::new(&mut buf, 0);

        encoder.write_zeros(8);
        assert_eq!(encoder.position(), 8);
        assert!(buf[0..8].iter().all(|&b| b == 0));
        assert_eq!(buf[8], 0xFF);
    }
}
