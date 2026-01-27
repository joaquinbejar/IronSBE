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
