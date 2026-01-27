//! Ultra-low-latency SPSC (Single-Producer Single-Consumer) channel.
//!
//! This module provides a lock-free ring buffer based channel optimized
//! for single-producer single-consumer scenarios with ~10-20ns latency.

use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Creates a new SPSC channel pair with the given capacity.
///
/// # Arguments
/// * `capacity` - Maximum number of items the channel can hold
///
/// # Returns
/// A tuple of (sender, receiver).
#[must_use]
pub fn channel<T>(capacity: usize) -> (SpscSender<T>, SpscReceiver<T>) {
    SpscChannel::new(capacity)
}

/// SPSC channel factory.
pub struct SpscChannel;

impl SpscChannel {
    /// Creates a new SPSC channel pair.
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of items the channel can hold
    #[allow(clippy::new_ret_no_self)]
    #[must_use]
    pub fn new<T>(capacity: usize) -> (SpscSender<T>, SpscReceiver<T>) {
        let (producer, consumer) = RingBuffer::new(capacity);
        let closed = Arc::new(AtomicBool::new(false));

        (
            SpscSender {
                producer,
                closed: Arc::clone(&closed),
            },
            SpscReceiver { consumer, closed },
        )
    }
}

/// Sender half of an SPSC channel.
///
/// This is optimized for single-threaded use and provides ~10-20ns send latency.
pub struct SpscSender<T> {
    producer: Producer<T>,
    closed: Arc<AtomicBool>,
}

impl<T> SpscSender<T> {
    /// Sends an item through the channel.
    ///
    /// This is a non-blocking operation with ~10-20ns latency.
    ///
    /// # Arguments
    /// * `item` - Item to send
    ///
    /// # Returns
    /// `Ok(())` on success, `Err(item)` if channel is full or closed.
    #[inline(always)]
    pub fn send(&mut self, item: T) -> Result<(), T> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(item);
        }
        self.producer.push(item).map_err(|e| match e {
            rtrb::PushError::Full(item) => item,
        })
    }

    /// Tries to send an item, returning immediately.
    ///
    /// # Arguments
    /// * `item` - Item to send
    ///
    /// # Returns
    /// `Ok(())` on success, `Err(item)` if channel is full or closed.
    #[inline(always)]
    pub fn try_send(&mut self, item: T) -> Result<(), T> {
        self.send(item)
    }

    /// Checks if the receiver is still connected.
    #[inline(always)]
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    /// Returns the number of items currently in the channel.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.producer.slots()
    }

    /// Returns true if the channel is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the capacity of the channel.
    #[inline]
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.producer.buffer().capacity()
    }
}

impl<T> Drop for SpscSender<T> {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

/// Receiver half of an SPSC channel.
///
/// This is optimized for single-threaded use and provides ~10-20ns receive latency.
pub struct SpscReceiver<T> {
    consumer: Consumer<T>,
    closed: Arc<AtomicBool>,
}

impl<T> SpscReceiver<T> {
    /// Receives an item from the channel.
    ///
    /// This is a non-blocking operation with ~10-20ns latency.
    ///
    /// # Returns
    /// `Some(item)` if available, `None` if channel is empty.
    #[inline(always)]
    pub fn recv(&mut self) -> Option<T> {
        self.consumer.pop().ok()
    }

    /// Tries to receive an item, returning immediately.
    ///
    /// # Returns
    /// `Some(item)` if available, `None` if channel is empty.
    #[inline(always)]
    pub fn try_recv(&mut self) -> Option<T> {
        self.recv()
    }

    /// Busy-poll receive for hot path scenarios.
    ///
    /// This will spin until an item is available. Use with caution
    /// as it will consume 100% CPU while waiting.
    ///
    /// # Returns
    /// The received item.
    #[inline(always)]
    pub fn recv_spin(&mut self) -> T {
        loop {
            if let Ok(item) = self.consumer.pop() {
                return item;
            }
            std::hint::spin_loop();
        }
    }

    /// Receives with a spin count limit before yielding.
    ///
    /// # Arguments
    /// * `spin_count` - Number of spins before returning None
    ///
    /// # Returns
    /// `Some(item)` if received within spin count, `None` otherwise.
    #[inline]
    pub fn recv_spin_limited(&mut self, spin_count: usize) -> Option<T> {
        for _ in 0..spin_count {
            if let Ok(item) = self.consumer.pop() {
                return Some(item);
            }
            std::hint::spin_loop();
        }
        None
    }

    /// Drains all available items from the channel.
    ///
    /// # Returns
    /// An iterator over all currently available items.
    #[inline]
    pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        std::iter::from_fn(|| self.consumer.pop().ok())
    }

    /// Checks if the sender is still connected.
    #[inline(always)]
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.closed.load(Ordering::Relaxed) || !self.is_empty()
    }

    /// Returns the number of items currently in the channel.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.consumer.slots()
    }

    /// Returns true if the channel is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> Drop for SpscReceiver<T> {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_send_recv() {
        let (mut tx, mut rx) = channel::<u64>(16);

        assert!(tx.send(42).is_ok());
        assert_eq!(rx.recv(), Some(42));
        assert_eq!(rx.recv(), None);
    }

    #[test]
    fn test_multiple_items() {
        let (mut tx, mut rx) = channel::<u64>(16);

        for i in 0..10 {
            assert!(tx.send(i).is_ok());
        }

        for i in 0..10 {
            assert_eq!(rx.recv(), Some(i));
        }
    }

    #[test]
    fn test_full_channel() {
        let (mut tx, mut rx) = channel::<u64>(4);

        // Fill the channel
        for i in 0..4 {
            assert!(tx.send(i).is_ok());
        }

        // Should fail when full
        assert!(tx.send(100).is_err());

        // Drain one
        assert_eq!(rx.recv(), Some(0));

        // Should succeed now
        assert!(tx.send(100).is_ok());
    }

    #[test]
    fn test_drain() {
        let (mut tx, mut rx) = channel::<u64>(16);

        for i in 0..5 {
            tx.send(i).unwrap();
        }

        let items: Vec<_> = rx.drain().collect();
        assert_eq!(items, vec![0, 1, 2, 3, 4]);
        assert!(rx.is_empty());
    }

    #[test]
    fn test_disconnect_detection() {
        let (tx, rx) = channel::<u64>(16);

        assert!(rx.is_connected());
        drop(tx);
        assert!(!rx.is_connected());
    }

    #[test]
    fn test_recv_spin_limited() {
        let (mut tx, mut rx) = channel::<u64>(16);

        // Should return None when empty
        assert_eq!(rx.recv_spin_limited(100), None);

        tx.send(42).unwrap();
        assert_eq!(rx.recv_spin_limited(100), Some(42));
    }
}
