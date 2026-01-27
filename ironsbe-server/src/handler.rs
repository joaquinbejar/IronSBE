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
