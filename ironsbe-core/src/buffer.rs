//! Buffer traits and implementations for zero-copy SBE operations.
//!
//! This module provides:
//! - [`ReadBuffer`] trait for read-only buffer access
//! - [`WriteBuffer`] trait for read-write buffer access
//! - [`AlignedBuffer`] for cache-line aligned buffers
//! - [`BufferPool`] for reusable buffer allocation

use crossbeam_queue::ArrayQueue;
use std::sync::Arc;

/// Trait for read-only buffer access with optimized primitive reads.
///
/// All read methods use little-endian byte order as per SBE specification.
pub trait ReadBuffer {
    /// Returns the buffer as a byte slice.
    fn as_slice(&self) -> &[u8];

    /// Returns the length of the buffer in bytes.
    fn len(&self) -> usize;

    /// Returns true if the buffer is empty.
    #[must_use]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Reads a u8 at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_u8(&self, offset: usize) -> u8 {
        self.as_slice()[offset]
    }

    /// Reads an i8 at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_i8(&self, offset: usize) -> i8 {
        self.as_slice()[offset] as i8
    }

    /// Reads a u16 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_u16_le(&self, offset: usize) -> u16 {
        let bytes = &self.as_slice()[offset..offset + 2];
        u16::from_le_bytes([bytes[0], bytes[1]])
    }

    /// Reads an i16 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_i16_le(&self, offset: usize) -> i16 {
        let bytes = &self.as_slice()[offset..offset + 2];
        i16::from_le_bytes([bytes[0], bytes[1]])
    }

    /// Reads a u32 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_u32_le(&self, offset: usize) -> u32 {
        let bytes = &self.as_slice()[offset..offset + 4];
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// Reads an i32 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_i32_le(&self, offset: usize) -> i32 {
        let bytes = &self.as_slice()[offset..offset + 4];
        i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// Reads a u64 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_u64_le(&self, offset: usize) -> u64 {
        let bytes = &self.as_slice()[offset..offset + 8];
        u64::from_le_bytes(bytes.try_into().unwrap())
    }

    /// Reads an i64 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_i64_le(&self, offset: usize) -> i64 {
        let bytes = &self.as_slice()[offset..offset + 8];
        i64::from_le_bytes(bytes.try_into().unwrap())
    }

    /// Reads an f32 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_f32_le(&self, offset: usize) -> f32 {
        f32::from_bits(self.get_u32_le(offset))
    }

    /// Reads an f64 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to read from
    #[inline(always)]
    fn get_f64_le(&self, offset: usize) -> f64 {
        f64::from_bits(self.get_u64_le(offset))
    }

    /// Returns a slice of bytes at the given offset and length.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to start from
    /// * `len` - Number of bytes to read
    #[inline(always)]
    fn get_bytes(&self, offset: usize, len: usize) -> &[u8] {
        &self.as_slice()[offset..offset + len]
    }

    /// Reads a fixed-length character array as a string slice.
    /// Trims null bytes from the end.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to start from
    /// * `len` - Maximum length of the string
    #[inline]
    fn get_str(&self, offset: usize, len: usize) -> &str {
        let bytes = self.get_bytes(offset, len);
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
        std::str::from_utf8(&bytes[..end]).unwrap_or("")
    }
}

/// Trait for read-write buffer access with optimized primitive writes.
///
/// All write methods use little-endian byte order as per SBE specification.
pub trait WriteBuffer: ReadBuffer {
    /// Returns the buffer as a mutable byte slice.
    fn as_mut_slice(&mut self) -> &mut [u8];

    /// Writes a u8 at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_u8(&mut self, offset: usize, value: u8) {
        self.as_mut_slice()[offset] = value;
    }

    /// Writes an i8 at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_i8(&mut self, offset: usize, value: i8) {
        self.as_mut_slice()[offset] = value as u8;
    }

    /// Writes a u16 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_u16_le(&mut self, offset: usize, value: u16) {
        let bytes = value.to_le_bytes();
        self.as_mut_slice()[offset..offset + 2].copy_from_slice(&bytes);
    }

    /// Writes an i16 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_i16_le(&mut self, offset: usize, value: i16) {
        let bytes = value.to_le_bytes();
        self.as_mut_slice()[offset..offset + 2].copy_from_slice(&bytes);
    }

    /// Writes a u32 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_u32_le(&mut self, offset: usize, value: u32) {
        let bytes = value.to_le_bytes();
        self.as_mut_slice()[offset..offset + 4].copy_from_slice(&bytes);
    }

    /// Writes an i32 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_i32_le(&mut self, offset: usize, value: i32) {
        let bytes = value.to_le_bytes();
        self.as_mut_slice()[offset..offset + 4].copy_from_slice(&bytes);
    }

    /// Writes a u64 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_u64_le(&mut self, offset: usize, value: u64) {
        let bytes = value.to_le_bytes();
        self.as_mut_slice()[offset..offset + 8].copy_from_slice(&bytes);
    }

    /// Writes an i64 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_i64_le(&mut self, offset: usize, value: i64) {
        let bytes = value.to_le_bytes();
        self.as_mut_slice()[offset..offset + 8].copy_from_slice(&bytes);
    }

    /// Writes an f32 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_f32_le(&mut self, offset: usize, value: f32) {
        self.put_u32_le(offset, value.to_bits());
    }

    /// Writes an f64 in little-endian at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - Value to write
    #[inline(always)]
    fn put_f64_le(&mut self, offset: usize, value: f64) {
        self.put_u64_le(offset, value.to_bits());
    }

    /// Writes a byte slice at the given offset.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `src` - Source bytes to copy
    #[inline(always)]
    fn put_bytes(&mut self, offset: usize, src: &[u8]) {
        self.as_mut_slice()[offset..offset + src.len()].copy_from_slice(src);
    }

    /// Writes a string to a fixed-length field, padding with nulls.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to write to
    /// * `value` - String value to write
    /// * `max_len` - Maximum field length in bytes
    #[inline]
    fn put_str(&mut self, offset: usize, value: &str, max_len: usize) {
        let bytes = value.as_bytes();
        let copy_len = bytes.len().min(max_len);
        self.as_mut_slice()[offset..offset + copy_len].copy_from_slice(&bytes[..copy_len]);
        if copy_len < max_len {
            self.as_mut_slice()[offset + copy_len..offset + max_len].fill(0);
        }
    }

    /// Fills a range with zeros.
    ///
    /// # Arguments
    /// * `offset` - Byte offset to start from
    /// * `len` - Number of bytes to zero
    #[inline]
    fn zero(&mut self, offset: usize, len: usize) {
        self.as_mut_slice()[offset..offset + len].fill(0);
    }
}

/// Implement ReadBuffer for byte slices.
impl ReadBuffer for [u8] {
    #[inline(always)]
    fn as_slice(&self) -> &[u8] {
        self
    }

    #[inline(always)]
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }
}

/// Implement ReadBuffer for `Vec<u8>`.
impl ReadBuffer for Vec<u8> {
    #[inline(always)]
    fn as_slice(&self) -> &[u8] {
        self
    }

    #[inline(always)]
    fn len(&self) -> usize {
        Vec::len(self)
    }
}

/// Implement WriteBuffer for `Vec<u8>`.
impl WriteBuffer for Vec<u8> {
    #[inline(always)]
    fn as_mut_slice(&mut self) -> &mut [u8] {
        self
    }
}

/// Cache-line aligned buffer for optimal CPU cache performance.
///
/// The buffer is aligned to 64 bytes (typical cache line size) to prevent
/// false sharing and ensure optimal memory access patterns.
///
/// # Type Parameters
/// * `N` - Buffer size in bytes
#[repr(C, align(64))]
#[derive(Clone)]
pub struct AlignedBuffer<const N: usize> {
    data: [u8; N],
}

impl<const N: usize> AlignedBuffer<N> {
    /// Creates a new zeroed aligned buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self { data: [0u8; N] }
    }

    /// Creates a new zeroed aligned buffer (alias for `new`).
    #[must_use]
    pub const fn zeroed() -> Self {
        Self { data: [0u8; N] }
    }

    /// Returns the capacity of the buffer in bytes.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        N
    }
}

impl<const N: usize> Default for AlignedBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> ReadBuffer for AlignedBuffer<N> {
    #[inline(always)]
    fn as_slice(&self) -> &[u8] {
        &self.data
    }

    #[inline(always)]
    fn len(&self) -> usize {
        N
    }
}

impl<const N: usize> WriteBuffer for AlignedBuffer<N> {
    #[inline(always)]
    fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

impl<const N: usize> AsRef<[u8]> for AlignedBuffer<N> {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl<const N: usize> AsMut<[u8]> for AlignedBuffer<N> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

impl<const N: usize> std::fmt::Debug for AlignedBuffer<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlignedBuffer")
            .field("capacity", &N)
            .finish()
    }
}

/// Default buffer size for the pool (64KB).
pub const DEFAULT_BUFFER_SIZE: usize = 65536;

/// Pool of reusable aligned buffers to avoid allocation overhead.
///
/// The pool uses a lock-free queue for thread-safe buffer acquisition
/// and release with minimal contention.
pub struct BufferPool {
    buffers: Arc<ArrayQueue<Box<AlignedBuffer<DEFAULT_BUFFER_SIZE>>>>,
    capacity: usize,
}

impl BufferPool {
    /// Creates a new buffer pool with the specified capacity.
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of buffers in the pool
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let buffers = ArrayQueue::new(capacity);
        for _ in 0..capacity {
            let _ = buffers.push(Box::new(AlignedBuffer::zeroed()));
        }
        Self {
            buffers: Arc::new(buffers),
            capacity,
        }
    }

    /// Acquires a buffer from the pool.
    ///
    /// Returns `None` if the pool is empty.
    #[inline]
    #[must_use]
    pub fn acquire(&self) -> Option<Box<AlignedBuffer<DEFAULT_BUFFER_SIZE>>> {
        self.buffers.pop()
    }

    /// Releases a buffer back to the pool.
    ///
    /// The buffer is zeroed before being returned to the pool for security.
    ///
    /// # Arguments
    /// * `buffer` - Buffer to release
    #[inline]
    pub fn release(&self, mut buffer: Box<AlignedBuffer<DEFAULT_BUFFER_SIZE>>) {
        buffer.as_mut_slice().fill(0);
        let _ = self.buffers.push(buffer);
    }

    /// Returns the capacity of the pool.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the number of available buffers in the pool.
    #[must_use]
    pub fn available(&self) -> usize {
        self.buffers.len()
    }
}

impl Clone for BufferPool {
    fn clone(&self) -> Self {
        Self {
            buffers: Arc::clone(&self.buffers),
            capacity: self.capacity,
        }
    }
}

impl std::fmt::Debug for BufferPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferPool")
            .field("capacity", &self.capacity)
            .field("available", &self.buffers.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aligned_buffer_creation() {
        let buf: AlignedBuffer<1024> = AlignedBuffer::new();
        assert_eq!(buf.len(), 1024);
        assert!(buf.as_slice().iter().all(|&b| b == 0));
    }

    #[test]
    fn test_aligned_buffer_alignment() {
        let buf: AlignedBuffer<64> = AlignedBuffer::new();
        let ptr = buf.as_slice().as_ptr() as usize;
        assert_eq!(ptr % 64, 0, "Buffer should be 64-byte aligned");
    }

    #[test]
    fn test_read_write_primitives() {
        let mut buf: AlignedBuffer<64> = AlignedBuffer::new();

        buf.put_u8(0, 0xFF);
        assert_eq!(buf.get_u8(0), 0xFF);

        buf.put_i8(1, -42);
        assert_eq!(buf.get_i8(1), -42);

        buf.put_u16_le(2, 0x1234);
        assert_eq!(buf.get_u16_le(2), 0x1234);

        buf.put_i16_le(4, -1000);
        assert_eq!(buf.get_i16_le(4), -1000);

        buf.put_u32_le(8, 0x12345678);
        assert_eq!(buf.get_u32_le(8), 0x12345678);

        buf.put_i32_le(12, -100000);
        assert_eq!(buf.get_i32_le(12), -100000);

        buf.put_u64_le(16, 0x123456789ABCDEF0);
        assert_eq!(buf.get_u64_le(16), 0x123456789ABCDEF0);

        buf.put_i64_le(24, -1_000_000_000_000);
        assert_eq!(buf.get_i64_le(24), -1_000_000_000_000);

        buf.put_f32_le(32, std::f32::consts::PI);
        assert!((buf.get_f32_le(32) - std::f32::consts::PI).abs() < 0.00001);

        buf.put_f64_le(40, std::f64::consts::PI);
        assert!((buf.get_f64_le(40) - std::f64::consts::PI).abs() < 0.0000001);
    }

    #[test]
    fn test_read_write_bytes() {
        let mut buf: AlignedBuffer<64> = AlignedBuffer::new();
        let data = b"Hello, SBE!";

        buf.put_bytes(0, data);
        assert_eq!(buf.get_bytes(0, data.len()), data);
    }

    #[test]
    fn test_read_write_str() {
        let mut buf: AlignedBuffer<64> = AlignedBuffer::new();

        buf.put_str(0, "AAPL", 8);
        assert_eq!(buf.get_str(0, 8), "AAPL");

        buf.put_str(8, "VERY_LONG_SYMBOL", 8);
        assert_eq!(buf.get_str(8, 8), "VERY_LON");
    }

    #[test]
    fn test_buffer_pool() {
        let pool = BufferPool::new(4);
        assert_eq!(pool.capacity(), 4);
        assert_eq!(pool.available(), 4);

        let buf1 = pool.acquire().expect("Should acquire buffer");
        assert_eq!(pool.available(), 3);

        let buf2 = pool.acquire().expect("Should acquire buffer");
        assert_eq!(pool.available(), 2);

        pool.release(buf1);
        assert_eq!(pool.available(), 3);

        pool.release(buf2);
        assert_eq!(pool.available(), 4);
    }

    #[test]
    fn test_buffer_pool_empty() {
        let pool = BufferPool::new(1);
        let _buf = pool.acquire().expect("Should acquire buffer");
        assert!(pool.acquire().is_none(), "Pool should be empty");
    }

    #[test]
    fn test_slice_read_buffer() {
        let data: &[u8] = &[0x12, 0x34, 0x56, 0x78];
        assert_eq!(data.get_u8(0), 0x12);
        assert_eq!(data.get_u16_le(0), 0x3412);
        assert_eq!(data.get_u32_le(0), 0x78563412);
    }

    #[test]
    fn test_vec_write_buffer() {
        let mut data = vec![0u8; 16];
        data.put_u32_le(0, 0xDEADBEEF);
        assert_eq!(data.get_u32_le(0), 0xDEADBEEF);
    }

    #[test]
    fn test_aligned_buffer_zeroed() {
        let buf: AlignedBuffer<128> = AlignedBuffer::zeroed();
        assert_eq!(buf.len(), 128);
        assert!(buf.as_slice().iter().all(|&b| b == 0));
    }

    #[test]
    fn test_aligned_buffer_as_mut_slice() {
        let mut buf: AlignedBuffer<64> = AlignedBuffer::new();
        let slice = buf.as_mut_slice();
        slice[0] = 0xAB;
        slice[1] = 0xCD;
        assert_eq!(buf.get_u8(0), 0xAB);
        assert_eq!(buf.get_u8(1), 0xCD);
    }

    #[test]
    fn test_aligned_buffer_debug() {
        let buf: AlignedBuffer<256> = AlignedBuffer::new();
        let debug_str = format!("{:?}", buf);
        assert!(debug_str.contains("AlignedBuffer"));
        assert!(debug_str.contains("256"));
    }

    #[test]
    fn test_buffer_pool_clone() {
        let pool1 = BufferPool::new(2);
        let pool2 = pool1.clone();

        // Both pools share the same underlying buffers
        let buf = pool1.acquire().expect("Should acquire");
        assert_eq!(pool1.available(), 1);
        assert_eq!(pool2.available(), 1);

        pool2.release(buf);
        assert_eq!(pool1.available(), 2);
        assert_eq!(pool2.available(), 2);
    }

    #[test]
    fn test_buffer_pool_debug() {
        let pool = BufferPool::new(4);
        let debug_str = format!("{:?}", pool);
        assert!(debug_str.contains("BufferPool"));
        assert!(debug_str.contains("capacity"));
        assert!(debug_str.contains("4"));
    }

    #[test]
    fn test_slice_read_buffer_all_types() {
        let data: &[u8] = &[
            0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
            0x77, 0x88,
        ];

        assert_eq!(data.get_i8(0), 0x12);
        assert_eq!(data.get_i16_le(0), 0x3412);
        assert_eq!(data.get_i32_le(0), 0x78563412_u32 as i32);
        assert_eq!(data.get_i64_le(0), 0xF0DEBC9A78563412_u64 as i64);
        assert_eq!(data.get_u64_le(0), 0xF0DEBC9A78563412);
    }

    #[test]
    fn test_vec_write_buffer_all_types() {
        let mut data = vec![0u8; 64];

        data.put_u8(0, 0xFF);
        assert_eq!(data.get_u8(0), 0xFF);

        data.put_i8(1, -1);
        assert_eq!(data.get_i8(1), -1);

        data.put_u16_le(2, 0xABCD);
        assert_eq!(data.get_u16_le(2), 0xABCD);

        data.put_i16_le(4, -1234);
        assert_eq!(data.get_i16_le(4), -1234);

        data.put_i32_le(8, -123456);
        assert_eq!(data.get_i32_le(8), -123456);

        data.put_u64_le(16, 0x123456789ABCDEF0);
        assert_eq!(data.get_u64_le(16), 0x123456789ABCDEF0);

        data.put_i64_le(24, -9876543210);
        assert_eq!(data.get_i64_le(24), -9876543210);

        data.put_f32_le(32, 1.23456);
        assert!((data.get_f32_le(32) - 1.23456).abs() < 0.0001);

        data.put_f64_le(40, 9.87654321);
        assert!((data.get_f64_le(40) - 9.87654321).abs() < 0.0000001);

        data.put_bytes(48, b"test");
        assert_eq!(data.get_bytes(48, 4), b"test");

        data.put_str(52, "hello", 8);
        assert_eq!(data.get_str(52, 8), "hello");
    }

    #[test]
    fn test_aligned_buffer_default() {
        let buf: AlignedBuffer<32> = AlignedBuffer::default();
        assert_eq!(buf.len(), 32);
    }
}
