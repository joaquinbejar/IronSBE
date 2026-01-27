//! IPC (Inter-Process Communication) transport module.
//!
//! Provides shared memory based transport for ultra-low-latency local communication.

pub mod ringbuffer;
pub mod shm;

pub use ringbuffer::{SharedConsumer, SharedProducer, SharedRingBuffer};
pub use shm::{SharedMemory, SharedMemoryConfig};
