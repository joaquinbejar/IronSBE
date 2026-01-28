//! Message handler traits.

use ironsbe_core::header::MessageHeader;

/// Trait for handling incoming SBE messages.
pub trait MessageHandler: Send + Sync {
    /// Called when a complete SBE message is received.
    ///
    /// # Arguments
    /// * `session_id` - ID of the session that sent the message
    /// * `header` - Decoded message header
    /// * `buffer` - Full message buffer (including header)
    /// * `responder` - Interface for sending responses
    fn on_message(
        &self,
        session_id: u64,
        header: &MessageHeader,
        buffer: &[u8],
        responder: &dyn Responder,
    );

    /// Called when a new session is established.
    ///
    /// # Arguments
    /// * `session_id` - ID of the new session
    fn on_session_start(&self, _session_id: u64) {}

    /// Called when a session ends.
    ///
    /// # Arguments
    /// * `session_id` - ID of the ended session
    fn on_session_end(&self, _session_id: u64) {}

    /// Called on decode error.
    ///
    /// # Arguments
    /// * `session_id` - ID of the session
    /// * `error` - Description of the error
    fn on_error(&self, _session_id: u64, _error: &str) {}
}

/// Responder for sending messages back to clients.
pub trait Responder: Send + Sync {
    /// Sends a message to the current session.
    ///
    /// # Arguments
    /// * `message` - Message bytes to send
    ///
    /// # Errors
    /// Returns error if send fails.
    fn send(&self, message: &[u8]) -> Result<(), SendError>;

    /// Sends a message to a specific session.
    ///
    /// # Arguments
    /// * `session_id` - Target session ID
    /// * `message` - Message bytes to send
    ///
    /// # Errors
    /// Returns error if send fails.
    fn send_to(&self, session_id: u64, message: &[u8]) -> Result<(), SendError>;
}

/// Error type for send operations.
#[derive(Debug, Clone)]
pub struct SendError {
    /// Error message.
    pub message: String,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "send error: {}", self.message)
    }
}

impl std::error::Error for SendError {}

/// Handler for specific message type.
pub trait TypedHandler: Send + Sync {
    /// Handles a message of the specific type.
    ///
    /// # Arguments
    /// * `session_id` - ID of the session
    /// * `buffer` - Message buffer (after header)
    /// * `responder` - Interface for sending responses
    fn handle(&self, session_id: u64, buffer: &[u8], responder: &dyn Responder);
}

/// Wrapper to convert a closure into a TypedHandler.
pub struct FnHandler<F> {
    handler: F,
}

impl<F> FnHandler<F>
where
    F: Fn(u64, &[u8], &dyn Responder) + Send + Sync,
{
    /// Creates a new function handler.
    pub fn new(handler: F) -> Self {
        Self { handler }
    }
}

impl<F> TypedHandler for FnHandler<F>
where
    F: Fn(u64, &[u8], &dyn Responder) + Send + Sync,
{
    fn handle(&self, session_id: u64, buffer: &[u8], responder: &dyn Responder) {
        (self.handler)(session_id, buffer, responder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_send_error_display() {
        let err = SendError {
            message: "connection lost".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("connection lost"));
        assert!(msg.contains("send error"));
    }

    #[test]
    fn test_send_error_clone() {
        let err = SendError {
            message: "test".to_string(),
        };
        let cloned = err.clone();
        assert_eq!(err.message, cloned.message);
    }

    #[test]
    fn test_send_error_debug() {
        let err = SendError {
            message: "debug test".to_string(),
        };
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("SendError"));
        assert!(debug_str.contains("debug test"));
    }

    struct MockResponder;

    impl Responder for MockResponder {
        fn send(&self, _message: &[u8]) -> Result<(), SendError> {
            Ok(())
        }

        fn send_to(&self, _session_id: u64, _message: &[u8]) -> Result<(), SendError> {
            Ok(())
        }
    }

    #[test]
    fn test_fn_handler() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let handler = FnHandler::new(move |_session_id, _buffer, _responder| {
            called_clone.store(true, Ordering::SeqCst);
        });

        let responder = MockResponder;
        handler.handle(1, &[1, 2, 3], &responder);

        assert!(called.load(Ordering::SeqCst));
    }

    struct TestMessageHandler {
        session_started: Arc<AtomicBool>,
        session_ended: Arc<AtomicBool>,
        error_received: Arc<AtomicBool>,
    }

    impl MessageHandler for TestMessageHandler {
        fn on_message(
            &self,
            _session_id: u64,
            _header: &MessageHeader,
            _buffer: &[u8],
            _responder: &dyn Responder,
        ) {
        }

        fn on_session_start(&self, _session_id: u64) {
            self.session_started.store(true, Ordering::SeqCst);
        }

        fn on_session_end(&self, _session_id: u64) {
            self.session_ended.store(true, Ordering::SeqCst);
        }

        fn on_error(&self, _session_id: u64, _error: &str) {
            self.error_received.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_message_handler_callbacks() {
        let handler = TestMessageHandler {
            session_started: Arc::new(AtomicBool::new(false)),
            session_ended: Arc::new(AtomicBool::new(false)),
            error_received: Arc::new(AtomicBool::new(false)),
        };

        handler.on_session_start(1);
        assert!(handler.session_started.load(Ordering::SeqCst));

        handler.on_session_end(1);
        assert!(handler.session_ended.load(Ordering::SeqCst));

        handler.on_error(1, "test error");
        assert!(handler.error_received.load(Ordering::SeqCst));
    }
}
