//! Shared memory utilities.

use memmap2::{MmapMut, MmapOptions};
use std::fs::OpenOptions;
use std::path::Path;

/// Configuration for shared memory.
#[derive(Debug, Clone)]
pub struct SharedMemoryConfig {
    /// Size of the shared memory region in bytes.
    pub size: usize,
    /// Whether to create the file if it doesn't exist.
    pub create: bool,
}

impl Default for SharedMemoryConfig {
    fn default() -> Self {
        Self {
            size: 1024 * 1024, // 1MB
            create: true,
        }
    }
}

/// Shared memory region backed by a file.
pub struct SharedMemory {
    mmap: MmapMut,
    size: usize,
}

impl SharedMemory {
    /// Creates or opens a shared memory region.
    ///
    /// # Arguments
    /// * `path` - Path to the backing file
    /// * `config` - Configuration options
    ///
    /// # Errors
    /// Returns IO error if file operations fail.
    pub fn open(path: &Path, config: SharedMemoryConfig) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(config.create)
            .open(path)?;

        file.set_len(config.size as u64)?;

        let mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        Ok(Self {
            mmap,
            size: config.size,
        })
    }

    /// Creates a new shared memory region, initializing it to zeros.
    ///
    /// # Arguments
    /// * `path` - Path to the backing file
    /// * `size` - Size in bytes
    ///
    /// # Errors
    /// Returns IO error if file operations fail.
    pub fn create(path: &Path, size: usize) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;

        file.set_len(size as u64)?;

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // Initialize to zeros
        mmap.fill(0);

        Ok(Self { mmap, size })
    }

    /// Returns the size of the shared memory region.
    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns a slice of the shared memory.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.mmap
    }

    /// Returns a mutable slice of the shared memory.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.mmap
    }

    /// Flushes changes to the backing file.
    ///
    /// # Errors
    /// Returns IO error if flush fails.
    pub fn flush(&self) -> std::io::Result<()> {
        self.mmap.flush()
    }

    /// Flushes changes asynchronously.
    ///
    /// # Errors
    /// Returns IO error if flush fails.
    pub fn flush_async(&self) -> std::io::Result<()> {
        self.mmap.flush_async()
    }
}
