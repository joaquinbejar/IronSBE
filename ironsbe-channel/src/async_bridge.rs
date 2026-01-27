//! Async/sync bridging utilities.
//!
//! This module provides utilities for bridging between synchronous
//! and asynchronous code paths.

use parking_lot::Mutex;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// A simple async notifier for waking async tasks from sync code.
#[derive(Clone)]
pub struct AsyncNotifier {
    state: Arc<Mutex<NotifierState>>,
}

struct NotifierState {
    notified: bool,
    waker: Option<Waker>,
}

impl AsyncNotifier {
    /// Creates a new async notifier.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(NotifierState {
                notified: false,
                waker: None,
            })),
        }
    }

    /// Notifies the waiting task (sync side).
    pub fn notify(&self) {
        let mut state = self.state.lock();
        state.notified = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// Waits for notification (async side).
    pub fn wait(&self) -> NotifyFuture {
        NotifyFuture {
            state: Arc::clone(&self.state),
        }
    }

    /// Resets the notification state.
    pub fn reset(&self) {
        let mut state = self.state.lock();
        state.notified = false;
    }

    /// Checks if notified without waiting.
    #[must_use]
    pub fn is_notified(&self) -> bool {
        self.state.lock().notified
    }
}

impl Default for AsyncNotifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Future returned by `AsyncNotifier::wait()`.
pub struct NotifyFuture {
    state: Arc<Mutex<NotifierState>>,
}

impl Future for NotifyFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.lock();
        if state.notified {
            state.notified = false;
            Poll::Ready(())
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// A oneshot channel for sending a single value from sync to async code.
#[allow(dead_code)]
pub struct AsyncOneshot<T> {
    state: Arc<Mutex<OneshotState<T>>>,
}

struct OneshotState<T> {
    value: Option<T>,
    waker: Option<Waker>,
    closed: bool,
}

impl<T> AsyncOneshot<T> {
    /// Creates a new oneshot channel pair.
    pub fn channel() -> (OneshotSender<T>, OneshotReceiver<T>) {
        let state = Arc::new(Mutex::new(OneshotState {
            value: None,
            waker: None,
            closed: false,
        }));

        (
            OneshotSender {
                state: Arc::clone(&state),
            },
            OneshotReceiver { state },
        )
    }
}

impl<T> Default for AsyncOneshot<T> {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(OneshotState {
                value: None,
                waker: None,
                closed: false,
            })),
        }
    }
}

/// Sender half of an async oneshot channel.
pub struct OneshotSender<T> {
    state: Arc<Mutex<OneshotState<T>>>,
}

impl<T> OneshotSender<T> {
    /// Sends a value through the channel.
    ///
    /// # Arguments
    /// * `value` - Value to send
    ///
    /// # Returns
    /// `Ok(())` on success, `Err(value)` if receiver is dropped.
    pub fn send(self, value: T) -> Result<(), T> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(value);
        }
        state.value = Some(value);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
        Ok(())
    }

    /// Checks if the receiver is still waiting.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.state.lock().closed
    }
}

impl<T> Drop for OneshotSender<T> {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        state.closed = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

/// Receiver half of an async oneshot channel.
pub struct OneshotReceiver<T> {
    state: Arc<Mutex<OneshotState<T>>>,
}

impl<T> OneshotReceiver<T> {
    /// Tries to receive the value without waiting.
    pub fn try_recv(&mut self) -> Option<T> {
        self.state.lock().value.take()
    }
}

impl<T> Future for OneshotReceiver<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.lock();
        if let Some(value) = state.value.take() {
            Poll::Ready(Some(value))
        } else if state.closed {
            Poll::Ready(None)
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl<T> Drop for OneshotReceiver<T> {
    fn drop(&mut self) {
        self.state.lock().closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notifier_sync() {
        let notifier = AsyncNotifier::new();
        assert!(!notifier.is_notified());

        notifier.notify();
        assert!(notifier.is_notified());

        notifier.reset();
        assert!(!notifier.is_notified());
    }

    #[test]
    fn test_oneshot_sync() {
        let (tx, mut rx) = AsyncOneshot::<u64>::channel();

        assert!(rx.try_recv().is_none());
        assert!(tx.send(42).is_ok());
        assert_eq!(rx.try_recv(), Some(42));
    }

    #[test]
    fn test_oneshot_sender_dropped() {
        let (tx, mut rx) = AsyncOneshot::<u64>::channel();
        drop(tx);
        assert!(rx.try_recv().is_none());
    }
}
