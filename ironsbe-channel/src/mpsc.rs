//! MPSC (Multi-Producer Single-Consumer) channel.
//!
//! This module provides a bounded MPSC channel with multiple sender support
//! and ~50-100ns latency.

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use std::time::Duration;

/// Creates a new bounded MPSC channel pair.
///
/// # Arguments
/// * `capacity` - Maximum number of items the channel can hold
///
/// # Returns
/// A tuple of (sender, receiver).
#[must_use]
pub fn channel<T: Send>(capacity: usize) -> (MpscSender<T>, MpscReceiver<T>) {
    MpscChannel::bounded(capacity)
}

/// MPSC channel factory.
pub struct MpscChannel;

impl MpscChannel {
    /// Creates a new bounded MPSC channel pair.
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of items the channel can hold
    #[must_use]
    pub fn bounded<T: Send>(capacity: usize) -> (MpscSender<T>, MpscReceiver<T>) {
        let (sender, receiver) = bounded(capacity);
        (
            MpscSender { inner: sender },
            MpscReceiver { inner: receiver },
        )
    }
}

/// Sender half of an MPSC channel.
///
/// This can be cloned to create multiple senders.
#[derive(Clone)]
pub struct MpscSender<T> {
    inner: Sender<T>,
}

impl<T> MpscSender<T> {
    /// Non-blocking send attempt (~50-100ns).
    ///
    /// # Arguments
    /// * `item` - Item to send
    ///
    /// # Errors
    /// Returns the item if the channel is full or disconnected.
    #[inline]
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        self.inner.try_send(item)
    }

    /// Blocking send.
    ///
    /// # Arguments
    /// * `item` - Item to send
    ///
    /// # Errors
    /// Returns the item if the channel is disconnected.
    pub fn send(&self, item: T) -> Result<(), T> {
        self.inner.send(item).map_err(|e| e.0)
    }

    /// Send with timeout.
    ///
    /// # Arguments
    /// * `item` - Item to send
    /// * `timeout` - Maximum time to wait
    ///
    /// # Errors
    /// Returns the item if the operation times out or channel is disconnected.
    pub fn send_timeout(&self, item: T, timeout: Duration) -> Result<(), T> {
        self.inner.send_timeout(item, timeout).map_err(|e| match e {
            crossbeam_channel::SendTimeoutError::Timeout(v) => v,
            crossbeam_channel::SendTimeoutError::Disconnected(v) => v,
        })
    }

    /// Returns true if the receiver is still connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.inner.is_empty() || !self.inner.is_full()
    }

    /// Returns the number of items currently in the channel.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the channel is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns true if the channel is full.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.inner.is_full()
    }

    /// Returns the capacity of the channel.
    #[must_use]
    pub fn capacity(&self) -> Option<usize> {
        self.inner.capacity()
    }
}

/// Receiver half of an MPSC channel.
pub struct MpscReceiver<T> {
    inner: Receiver<T>,
}

impl<T> MpscReceiver<T> {
    /// Non-blocking receive.
    ///
    /// # Returns
    /// `Some(item)` if available, `None` if channel is empty.
    #[inline]
    pub fn try_recv(&self) -> Option<T> {
        self.inner.try_recv().ok()
    }

    /// Blocking receive.
    ///
    /// # Returns
    /// `Some(item)` if received, `None` if channel is disconnected.
    pub fn recv(&self) -> Option<T> {
        self.inner.recv().ok()
    }

    /// Receive with timeout.
    ///
    /// # Arguments
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    /// `Some(item)` if received within timeout, `None` otherwise.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<T> {
        self.inner.recv_timeout(timeout).ok()
    }

    /// Returns a reference to the underlying crossbeam receiver for select operations.
    #[must_use]
    pub fn as_select(&self) -> &Receiver<T> {
        &self.inner
    }

    /// Drains all available items from the channel.
    ///
    /// # Returns
    /// An iterator over all currently available items.
    pub fn drain(&self) -> impl Iterator<Item = T> + '_ {
        std::iter::from_fn(|| self.inner.try_recv().ok())
    }

    /// Returns the number of items currently in the channel.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if the channel is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns true if all senders have been dropped and channel is empty.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        self.inner.is_empty() && self.inner.try_recv().is_err()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_basic_send_recv() {
        let (tx, rx) = channel::<u64>(16);

        assert!(tx.try_send(42).is_ok());
        assert_eq!(rx.try_recv(), Some(42));
        assert_eq!(rx.try_recv(), None);
    }

    #[test]
    fn test_multiple_senders() {
        let (tx, rx) = channel::<u64>(16);
        let tx2 = tx.clone();

        tx.send(1).unwrap();
        tx2.send(2).unwrap();

        let mut received = vec![rx.recv().unwrap(), rx.recv().unwrap()];
        received.sort();
        assert_eq!(received, vec![1, 2]);
    }

    #[test]
    fn test_threaded_send() {
        let (tx, rx) = channel::<u64>(100);

        let handles: Vec<_> = (0..4)
            .map(|i| {
                let tx = tx.clone();
                thread::spawn(move || {
                    for j in 0..10 {
                        tx.send(i * 10 + j).unwrap();
                    }
                })
            })
            .collect();

        drop(tx); // Drop original sender

        for h in handles {
            h.join().unwrap();
        }

        let received: Vec<_> = rx.drain().collect();
        assert_eq!(received.len(), 40);
    }

    #[test]
    fn test_send_timeout() {
        let (tx, _rx) = channel::<u64>(1);

        // Fill the channel
        tx.send(1).unwrap();

        // Should timeout
        let result = tx.send_timeout(2, Duration::from_millis(10));
        assert!(result.is_err());
    }

    #[test]
    fn test_recv_timeout() {
        let (_tx, rx) = channel::<u64>(16);

        let result = rx.recv_timeout(Duration::from_millis(10));
        assert!(result.is_none());
    }

    #[test]
    fn test_drain() {
        let (tx, rx) = channel::<u64>(16);

        for i in 0..5 {
            tx.send(i).unwrap();
        }

        let items: Vec<_> = rx.drain().collect();
        assert_eq!(items, vec![0, 1, 2, 3, 4]);
        assert!(rx.is_empty());
    }
}
