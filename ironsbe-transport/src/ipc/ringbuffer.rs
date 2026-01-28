//! Lock-free SPSC ring buffer over shared memory.

use memmap2::{MmapMut, MmapOptions};
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Lock-free SPSC ring buffer over shared memory.
///
/// The ring buffer uses a cache-line aligned control block to prevent
/// false sharing between producer and consumer.
#[repr(C)]
pub struct SharedRingBuffer {
    /// Write position (producer).
    head: AtomicU64,
    /// Padding to separate cache lines.
    _pad1: [u8; 56],
    /// Read position (consumer).
    tail: AtomicU64,
    /// Padding to separate cache lines.
    _pad2: [u8; 56],
    /// Ring buffer capacity.
    capacity: u64,
    /// Capacity mask for fast modulo (capacity - 1).
    mask: u64,
}

impl SharedRingBuffer {
    /// Size of the control block header in bytes.
    pub const HEADER_SIZE: usize = 128;

    /// Creates a new shared ring buffer backed by a file.
    ///
    /// # Arguments
    /// * `path` - Path to the backing file
    /// * `capacity` - Capacity of the ring buffer (must be power of 2)
    ///
    /// # Errors
    /// Returns IO error if file operations fail.
    ///
    /// # Panics
    /// Panics if capacity is not a power of 2.
    pub fn create(path: &Path, capacity: usize) -> std::io::Result<MmapMut> {
        assert!(capacity.is_power_of_two(), "capacity must be power of 2");

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;

        let total_size = Self::HEADER_SIZE + capacity;
        file.set_len(total_size as u64)?;

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // Initialize header
        let header = unsafe { &mut *(mmap.as_mut_ptr() as *mut SharedRingBuffer) };
        header.head = AtomicU64::new(0);
        header.tail = AtomicU64::new(0);
        header.capacity = capacity as u64;
        header.mask = (capacity - 1) as u64;

        Ok(mmap)
    }

    /// Opens an existing shared ring buffer.
    ///
    /// # Arguments
    /// * `path` - Path to the backing file
    ///
    /// # Errors
    /// Returns IO error if file operations fail.
    pub fn open(path: &Path) -> std::io::Result<MmapMut> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;

        unsafe { MmapOptions::new().map_mut(&file) }
    }
}

/// Producer side of shared ring buffer.
pub struct SharedProducer {
    mmap: MmapMut,
}

impl SharedProducer {
    /// Creates a new producer from a memory map.
    #[must_use]
    pub fn new(mmap: MmapMut) -> Self {
        Self { mmap }
    }

    /// Writes a message to the ring buffer.
    ///
    /// # Arguments
    /// * `data` - Data to write
    ///
    /// # Returns
    /// `true` if written successfully, `false` if buffer is full.
    #[inline]
    pub fn write(&mut self, data: &[u8]) -> bool {
        let (head, tail, capacity, mask) = {
            let header = self.header();
            (
                header.head.load(Ordering::Relaxed),
                header.tail.load(Ordering::Acquire),
                header.capacity,
                header.mask,
            )
        };

        let available = capacity - (head - tail);
        let needed = 4 + data.len() as u64; // length prefix + data

        if available < needed {
            return false;
        }

        // Write length prefix
        let offset = SharedRingBuffer::HEADER_SIZE + ((head & mask) as usize);
        let len_bytes = (data.len() as u32).to_le_bytes();

        // Handle wrap-around for length prefix
        self.write_with_wrap(offset, &len_bytes, capacity as usize);

        // Write data
        let data_offset = offset + 4;
        self.write_with_wrap(data_offset, data, capacity as usize);

        // Update head with release semantics
        self.header().head.store(head + needed, Ordering::Release);

        true
    }

    /// Writes data handling wrap-around.
    fn write_with_wrap(&mut self, offset: usize, data: &[u8], capacity: usize) {
        let data_region =
            &mut self.mmap[SharedRingBuffer::HEADER_SIZE..SharedRingBuffer::HEADER_SIZE + capacity];
        let relative_offset = (offset - SharedRingBuffer::HEADER_SIZE) % capacity;

        if relative_offset + data.len() <= capacity {
            // No wrap
            data_region[relative_offset..relative_offset + data.len()].copy_from_slice(data);
        } else {
            // Wrap around
            let first_part = capacity - relative_offset;
            data_region[relative_offset..].copy_from_slice(&data[..first_part]);
            data_region[..data.len() - first_part].copy_from_slice(&data[first_part..]);
        }
    }

    /// Returns a reference to the header.
    fn header(&self) -> &SharedRingBuffer {
        unsafe { &*(self.mmap.as_ptr() as *const SharedRingBuffer) }
    }

    /// Returns the number of bytes available for writing.
    #[must_use]
    pub fn available(&self) -> usize {
        let header = self.header();
        let head = header.head.load(Ordering::Relaxed);
        let tail = header.tail.load(Ordering::Acquire);
        (header.capacity - (head - tail)) as usize
    }
}

/// Consumer side of shared ring buffer.
pub struct SharedConsumer {
    mmap: MmapMut,
}

impl SharedConsumer {
    /// Creates a new consumer from a memory map.
    #[must_use]
    pub fn new(mmap: MmapMut) -> Self {
        Self { mmap }
    }

    /// Reads the next message from the ring buffer.
    ///
    /// # Returns
    /// A slice containing the message data, or `None` if buffer is empty.
    #[inline]
    pub fn read(&mut self) -> Option<Vec<u8>> {
        let header = self.header();
        let tail = header.tail.load(Ordering::Relaxed);
        let head = header.head.load(Ordering::Acquire);

        if tail >= head {
            return None;
        }

        // Read length prefix
        let offset = SharedRingBuffer::HEADER_SIZE + ((tail & header.mask) as usize);
        let len_bytes = self.read_with_wrap(offset, 4, header.capacity as usize);
        let len =
            u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;

        // Read data
        let data_offset = offset + 4;
        let data = self.read_with_wrap(data_offset, len, header.capacity as usize);

        // Update tail
        let consumed = 4 + len as u64;
        header.tail.store(tail + consumed, Ordering::Release);

        Some(data)
    }

    /// Reads data handling wrap-around.
    fn read_with_wrap(&self, offset: usize, len: usize, capacity: usize) -> Vec<u8> {
        let data_region =
            &self.mmap[SharedRingBuffer::HEADER_SIZE..SharedRingBuffer::HEADER_SIZE + capacity];
        let relative_offset = (offset - SharedRingBuffer::HEADER_SIZE) % capacity;

        let mut result = vec![0u8; len];

        if relative_offset + len <= capacity {
            // No wrap
            result.copy_from_slice(&data_region[relative_offset..relative_offset + len]);
        } else {
            // Wrap around
            let first_part = capacity - relative_offset;
            result[..first_part].copy_from_slice(&data_region[relative_offset..]);
            result[first_part..].copy_from_slice(&data_region[..len - first_part]);
        }

        result
    }

    /// Returns a reference to the header.
    fn header(&self) -> &SharedRingBuffer {
        unsafe { &*(self.mmap.as_ptr() as *const SharedRingBuffer) }
    }

    /// Returns the number of bytes available for reading.
    #[must_use]
    pub fn available(&self) -> usize {
        let header = self.header();
        let tail = header.tail.load(Ordering::Relaxed);
        let head = header.head.load(Ordering::Acquire);
        (head - tail) as usize
    }

    /// Returns true if the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.available() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_shared_ring_buffer_header_size() {
        assert_eq!(SharedRingBuffer::HEADER_SIZE, 128);
    }

    #[test]
    fn test_shared_ring_buffer_create() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_rb_create");

        let mmap = SharedRingBuffer::create(&path, 1024).unwrap();
        assert!(mmap.len() >= 1024 + SharedRingBuffer::HEADER_SIZE);
    }

    #[test]
    fn test_shared_ring_buffer_open() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_rb_open");

        // Create first
        {
            let _mmap = SharedRingBuffer::create(&path, 1024).unwrap();
        }

        // Open existing
        let mmap = SharedRingBuffer::open(&path).unwrap();
        assert!(mmap.len() >= 1024 + SharedRingBuffer::HEADER_SIZE);
    }

    #[test]
    fn test_shared_producer_new() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_producer");

        let mmap = SharedRingBuffer::create(&path, 1024).unwrap();
        let producer = SharedProducer::new(mmap);
        assert!(producer.available() > 0);
    }

    #[test]
    fn test_shared_consumer_new() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_consumer");

        let mmap = SharedRingBuffer::create(&path, 256).unwrap();
        let consumer = SharedConsumer::new(mmap);

        assert!(consumer.is_empty());
        assert_eq!(consumer.available(), 0);
    }

    #[test]
    fn test_shared_consumer_read_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_read_empty");

        let mmap = SharedRingBuffer::create(&path, 256).unwrap();
        let mut consumer = SharedConsumer::new(mmap);

        assert!(consumer.read().is_none());
    }
}
