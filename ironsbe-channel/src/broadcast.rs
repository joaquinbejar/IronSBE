//! Broadcast channel for one-to-many messaging.
//!
//! This module provides a broadcast channel where a single sender can
//! send messages to multiple receivers.

use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Creates a new broadcast channel with the given capacity.
///
/// # Arguments
/// * `capacity` - Maximum number of messages to buffer
///
/// # Returns
/// A sender that can create receivers.
#[must_use]
pub fn channel<T: Clone + Send + Sync>(capacity: usize) -> BroadcastSender<T> {
    BroadcastSender::new(capacity)
}

/// Shared state for the broadcast channel.
struct BroadcastState<T> {
    /// Message buffer.
    buffer: VecDeque<(u64, T)>,
    /// Current sequence number.
    sequence: u64,
    /// Channel capacity.
    capacity: usize,
    /// Whether the sender is closed.
    closed: bool,
}

/// Sender half of a broadcast channel.
pub struct BroadcastSender<T> {
    state: Arc<RwLock<BroadcastState<T>>>,
    sequence: AtomicU64,
}

impl<T: Clone + Send + Sync> BroadcastSender<T> {
    /// Creates a new broadcast sender.
    ///
    /// # Arguments
    /// * `capacity` - Maximum number of messages to buffer
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Arc::new(RwLock::new(BroadcastState {
                buffer: VecDeque::with_capacity(capacity),
                sequence: 0,
                capacity,
                closed: false,
            })),
            sequence: AtomicU64::new(0),
        }
    }

    /// Sends a message to all receivers.
    ///
    /// # Arguments
    /// * `item` - Item to broadcast
    ///
    /// # Returns
    /// The sequence number of the sent message.
    pub fn send(&self, item: T) -> u64 {
        let mut state = self.state.write();
        let seq = state.sequence;
        state.sequence += 1;

        // Remove old messages if at capacity
        while state.buffer.len() >= state.capacity {
            state.buffer.pop_front();
        }

        state.buffer.push_back((seq, item));
        self.sequence.store(seq + 1, Ordering::Release);
        seq
    }

    /// Creates a new receiver subscribed to this sender.
    ///
    /// The receiver will start receiving messages from the next send.
    #[must_use]
    pub fn subscribe(&self) -> BroadcastReceiver<T> {
        let current_seq = self.sequence.load(Ordering::Acquire);
        BroadcastReceiver {
            state: Arc::clone(&self.state),
            next_seq: current_seq,
        }
    }

    /// Creates a new receiver that will receive all buffered messages.
    #[must_use]
    pub fn subscribe_from_start(&self) -> BroadcastReceiver<T> {
        let state = self.state.read();
        let start_seq = state.buffer.front().map(|(s, _)| *s).unwrap_or(0);
        drop(state);

        BroadcastReceiver {
            state: Arc::clone(&self.state),
            next_seq: start_seq,
        }
    }

    /// Returns the current sequence number.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }

    /// Returns the number of buffered messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.read().buffer.len()
    }

    /// Returns true if the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> Drop for BroadcastSender<T> {
    fn drop(&mut self) {
        self.state.write().closed = true;
    }
}

/// Receiver half of a broadcast channel.
pub struct BroadcastReceiver<T> {
    state: Arc<RwLock<BroadcastState<T>>>,
    next_seq: u64,
}

impl<T: Clone> BroadcastReceiver<T> {
    /// Receives the next message.
    ///
    /// # Returns
    /// `Some((sequence, item))` if available, `None` if no new messages.
    pub fn recv(&mut self) -> Option<(u64, T)> {
        let state = self.state.read();

        // Find the message with our expected sequence
        for (seq, item) in &state.buffer {
            if *seq == self.next_seq {
                self.next_seq += 1;
                return Some((*seq, item.clone()));
            }
        }

        None
    }

    /// Receives all available messages.
    ///
    /// # Returns
    /// A vector of (sequence, item) pairs.
    pub fn recv_all(&mut self) -> Vec<(u64, T)> {
        let state = self.state.read();
        let mut result = Vec::new();

        for (seq, item) in &state.buffer {
            if *seq >= self.next_seq {
                result.push((*seq, item.clone()));
                self.next_seq = *seq + 1;
            }
        }

        result
    }

    /// Checks if the sender is still connected.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.state.read().closed
    }

    /// Returns the number of messages this receiver has missed.
    #[must_use]
    pub fn lag(&self) -> u64 {
        let state = self.state.read();
        state.sequence.saturating_sub(self.next_seq)
    }

    /// Returns the next expected sequence number.
    #[must_use]
    pub fn next_sequence(&self) -> u64 {
        self.next_seq
    }
}

impl<T: Clone> Clone for BroadcastReceiver<T> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            next_seq: self.next_seq,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_broadcast() {
        let tx = channel::<u64>(16);
        let mut rx1 = tx.subscribe();
        let mut rx2 = tx.subscribe();

        tx.send(42);

        assert_eq!(rx1.recv(), Some((0, 42)));
        assert_eq!(rx2.recv(), Some((0, 42)));
    }

    #[test]
    fn test_multiple_messages() {
        let tx = channel::<u64>(16);
        let mut rx = tx.subscribe();

        tx.send(1);
        tx.send(2);
        tx.send(3);

        assert_eq!(rx.recv(), Some((0, 1)));
        assert_eq!(rx.recv(), Some((1, 2)));
        assert_eq!(rx.recv(), Some((2, 3)));
        assert_eq!(rx.recv(), None);
    }

    #[test]
    fn test_late_subscriber() {
        let tx = channel::<u64>(16);

        tx.send(1);
        tx.send(2);

        let mut rx = tx.subscribe();
        tx.send(3);

        // Late subscriber only gets message 3
        assert_eq!(rx.recv(), Some((2, 3)));
    }

    #[test]
    fn test_subscribe_from_start() {
        let tx = channel::<u64>(16);

        tx.send(1);
        tx.send(2);

        let mut rx = tx.subscribe_from_start();

        // Gets all buffered messages
        assert_eq!(rx.recv(), Some((0, 1)));
        assert_eq!(rx.recv(), Some((1, 2)));
    }

    #[test]
    fn test_recv_all() {
        let tx = channel::<u64>(16);
        let mut rx = tx.subscribe();

        tx.send(1);
        tx.send(2);
        tx.send(3);

        let all = rx.recv_all();
        assert_eq!(all, vec![(0, 1), (1, 2), (2, 3)]);
    }

    #[test]
    fn test_capacity_overflow() {
        let tx = channel::<u64>(3);
        let mut rx = tx.subscribe_from_start();

        tx.send(1);
        tx.send(2);
        tx.send(3);
        tx.send(4); // This should evict message 1

        let all = rx.recv_all();
        // Message 1 was evicted
        assert_eq!(all, vec![(1, 2), (2, 3), (3, 4)]);
    }

    #[test]
    fn test_lag() {
        let tx = channel::<u64>(16);
        let mut rx = tx.subscribe();

        tx.send(1);
        tx.send(2);
        tx.send(3);

        assert_eq!(rx.lag(), 3);

        rx.recv();
        assert_eq!(rx.lag(), 2);
    }

    #[test]
    fn test_disconnect() {
        let tx = channel::<u64>(16);
        let rx = tx.subscribe();

        assert!(rx.is_connected());
        drop(tx);
        assert!(!rx.is_connected());
    }
}
