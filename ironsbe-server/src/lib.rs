//! # IronSBE Server
//!
//! Server-side engine for SBE messaging.
//!
//! This crate provides:
//! - Server builder with configuration options
//! - Session management for connected clients
//! - Message handler traits and dispatcher
//! - Connection acceptor

pub mod builder;
pub mod dispatcher;
pub mod error;
pub mod handler;
pub mod session;

pub use builder::{Server, ServerBuilder, ServerCommand, ServerEvent, ServerHandle};
pub use dispatcher::MessageDispatcher;
pub use error::ServerError;
pub use handler::{MessageHandler, Responder, TypedHandler};
pub use session::SessionManager;
