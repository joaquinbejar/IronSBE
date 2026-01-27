//! # IronSBE Channel
//!
//! High-performance channel abstractions for messaging.
//!
//! This crate provides:
//! - [`spsc`] - Ultra-low-latency single-producer single-consumer channels (~20ns)
//! - [`mpsc`] - Multi-producer single-consumer channels (~100ns)
//! - [`broadcast`] - One-to-many broadcast channels
//! - [`async_bridge`] - Async/sync bridging utilities

pub mod async_bridge;
pub mod broadcast;
pub mod mpsc;
pub mod spsc;

pub use mpsc::{MpscChannel, MpscReceiver, MpscSender};
pub use spsc::{SpscChannel, SpscReceiver, SpscSender};

/// Error type for channel operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError<T> {
    /// Channel is full, item returned.
    Full(T),
    /// Channel is disconnected, item returned.
    Disconnected(T),
    /// Channel is empty.
    Empty,
    /// Operation timed out.
    Timeout,
}

impl<T> std::fmt::Display for ChannelError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full(_) => write!(f, "channel full"),
            Self::Disconnected(_) => write!(f, "channel disconnected"),
            Self::Empty => write!(f, "channel empty"),
            Self::Timeout => write!(f, "operation timed out"),
        }
    }
}

impl<T: std::fmt::Debug> std::error::Error for ChannelError<T> {}

/// Trait for channel senders.
pub trait ChannelSender<T>: Clone + Send + Sync {
    /// Non-blocking send attempt.
    ///
    /// # Errors
    /// Returns the item if the channel is full or disconnected.
    fn try_send(&self, item: T) -> Result<(), ChannelError<T>>;

    /// Blocking send with timeout.
    ///
    /// # Errors
    /// Returns the item if the operation times out or channel is disconnected.
    fn send_timeout(&self, item: T, timeout: std::time::Duration) -> Result<(), ChannelError<T>>;
}

/// Trait for channel receivers.
pub trait ChannelReceiver<T>: Send {
    /// Non-blocking receive attempt.
    fn try_recv(&self) -> Option<T>;

    /// Blocking receive with timeout.
    fn recv_timeout(&self, timeout: std::time::Duration) -> Option<T>;
}
