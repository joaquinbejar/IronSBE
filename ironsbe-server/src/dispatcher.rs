//! Message dispatcher for routing messages to handlers.

use crate::handler::{MessageHandler, Responder, TypedHandler};
use ironsbe_core::header::MessageHeader;
use std::collections::HashMap;
use std::sync::Arc;

/// Dispatcher that routes messages to specific handlers based on template ID.
pub struct MessageDispatcher {
    handlers: HashMap<u16, Arc<dyn TypedHandler>>,
    default_handler: Option<Arc<dyn MessageHandler>>,
}

impl MessageDispatcher {
    /// Creates a new empty dispatcher.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            default_handler: None,
        }
    }

    /// Registers a handler for a specific template ID.
    pub fn register<H: TypedHandler + 'static>(&mut self, template_id: u16, handler: H) {
        self.handlers.insert(template_id, Arc::new(handler));
    }

    /// Sets the default handler for unregistered template IDs.
    pub fn set_default<H: MessageHandler + 'static>(&mut self, handler: H) {
        self.default_handler = Some(Arc::new(handler));
    }

    /// Returns true if a handler is registered for the given template ID.
    #[must_use]
    pub fn has_handler(&self, template_id: u16) -> bool {
        self.handlers.contains_key(&template_id)
    }
}

impl Default for MessageDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageHandler for MessageDispatcher {
    fn on_message(
        &self,
        session_id: u64,
        header: &MessageHeader,
        buffer: &[u8],
        responder: &dyn Responder,
    ) {
        let template_id = { header.template_id };
        if let Some(handler) = self.handlers.get(&template_id) {
            handler.handle(session_id, buffer, responder);
        } else if let Some(default) = &self.default_handler {
            default.on_message(session_id, header, buffer, responder);
        } else {
            tracing::warn!(
                "No handler for template_id={} from session={}",
                template_id,
                session_id
            );
        }
    }

    fn on_session_start(&self, session_id: u64) {
        if let Some(default) = &self.default_handler {
            default.on_session_start(session_id);
        }
    }

    fn on_session_end(&self, session_id: u64) {
        if let Some(default) = &self.default_handler {
            default.on_session_end(session_id);
        }
    }

    fn on_error(&self, session_id: u64, error: &str) {
        if let Some(default) = &self.default_handler {
            default.on_error(session_id, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::{FnHandler, SendError};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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
    fn test_dispatcher_new() {
        let dispatcher = MessageDispatcher::new();
        assert!(!dispatcher.has_handler(1));
    }

    #[test]
    fn test_dispatcher_default() {
        let dispatcher = MessageDispatcher::default();
        assert!(!dispatcher.has_handler(1));
    }

    #[test]
    fn test_dispatcher_register() {
        let mut dispatcher = MessageDispatcher::new();

        let handler = FnHandler::new(|_session_id, _buffer, _responder| {});
        dispatcher.register(1, handler);

        assert!(dispatcher.has_handler(1));
        assert!(!dispatcher.has_handler(2));
    }

    #[test]
    fn test_dispatcher_on_message_with_handler() {
        let mut dispatcher = MessageDispatcher::new();

        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        let handler = FnHandler::new(move |_session_id, _buffer, _responder| {
            called_clone.store(true, Ordering::SeqCst);
        });
        dispatcher.register(1, handler);

        let header = MessageHeader::new(16, 1, 100, 1);
        let responder = MockResponder;
        dispatcher.on_message(1, &header, &[0u8; 24], &responder);

        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_dispatcher_on_message_no_handler() {
        let dispatcher = MessageDispatcher::new();

        let header = MessageHeader::new(16, 99, 100, 1);
        let responder = MockResponder;

        // Should not panic, just log warning
        dispatcher.on_message(1, &header, &[0u8; 24], &responder);
    }

    struct TestDefaultHandler {
        session_started: Arc<AtomicU64>,
        session_ended: Arc<AtomicU64>,
    }

    impl MessageHandler for TestDefaultHandler {
        fn on_message(
            &self,
            _session_id: u64,
            _header: &MessageHeader,
            _buffer: &[u8],
            _responder: &dyn Responder,
        ) {
        }

        fn on_session_start(&self, session_id: u64) {
            self.session_started.store(session_id, Ordering::SeqCst);
        }

        fn on_session_end(&self, session_id: u64) {
            self.session_ended.store(session_id, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_dispatcher_with_default_handler() {
        let mut dispatcher = MessageDispatcher::new();

        let session_started = Arc::new(AtomicU64::new(0));
        let session_ended = Arc::new(AtomicU64::new(0));

        let default_handler = TestDefaultHandler {
            session_started: session_started.clone(),
            session_ended: session_ended.clone(),
        };
        dispatcher.set_default(default_handler);

        dispatcher.on_session_start(42);
        assert_eq!(session_started.load(Ordering::SeqCst), 42);

        dispatcher.on_session_end(43);
        assert_eq!(session_ended.load(Ordering::SeqCst), 43);
    }

    #[test]
    fn test_dispatcher_on_error_with_default() {
        let mut dispatcher = MessageDispatcher::new();

        struct ErrorHandler {
            error_received: Arc<AtomicBool>,
        }

        impl MessageHandler for ErrorHandler {
            fn on_message(
                &self,
                _session_id: u64,
                _header: &MessageHeader,
                _buffer: &[u8],
                _responder: &dyn Responder,
            ) {
            }

            fn on_error(&self, _session_id: u64, _error: &str) {
                self.error_received.store(true, Ordering::SeqCst);
            }
        }

        let error_received = Arc::new(AtomicBool::new(false));
        let handler = ErrorHandler {
            error_received: error_received.clone(),
        };
        dispatcher.set_default(handler);

        dispatcher.on_error(1, "test error");
        assert!(error_received.load(Ordering::SeqCst));
    }
}
