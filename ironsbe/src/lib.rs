//! # IronSBE
//!
//! High-performance SBE (Simple Binary Encoding) server/client for Rust.
//!
//! IronSBE provides a complete implementation of the FIX Simple Binary Encoding
//! protocol optimized for low-latency financial systems.
//!
//! ## Features
//!
//! - **Zero-copy encoding/decoding** - Direct buffer access for minimal overhead
//! - **Schema-driven code generation** - Generate Rust code from FIX SBE XML schemas
//! - **Sub-microsecond latency** - Optimized for high-frequency trading
//! - **Flexible transport** - TCP, UDP multicast, and shared memory IPC
//! - **Market data patterns** - Order book management, gap recovery, A/B arbitration
//!
//! ## Quick Start
//!
//! ```ignore
//! use ironsbe::prelude::*;
//!
//! // Create a server
//! let (mut server, handle) = ServerBuilder::new()
//!     .bind("0.0.0.0:9000".parse().unwrap())
//!     .handler(MyHandler)
//!     .build();
//!
//! // Run the server
//! server.run().await?;
//! ```
//!
//! ## Crate Organization
//!
//! - [`core`] - Buffer traits, headers, encoder/decoder traits
//! - [`schema`] - XML schema parsing and validation
//! - [`codegen`] - Rust code generation from schemas
//! - [`channel`] - High-performance channels (SPSC, MPSC, broadcast)
//! - [`transport`] - Network transports (TCP, UDP, IPC)
//! - [`server`] - Server-side engine
//! - [`client`] - Client-side engine
//! - [`marketdata`] - Market data handling patterns

pub mod prelude;

/// Core types and traits for SBE encoding/decoding.
pub mod core {
    pub use ironsbe_core::*;
}

/// Schema parsing and validation.
pub mod schema {
    pub use ironsbe_schema::*;
}

/// Code generation from SBE schemas.
pub mod codegen {
    pub use ironsbe_codegen::*;
}

/// High-performance channel implementations.
pub mod channel {
    pub use ironsbe_channel::*;
}

/// Network transport layer.
pub mod transport {
    pub use ironsbe_transport::*;
}

/// Server-side engine.
pub mod server {
    pub use ironsbe_server::*;
}

/// Client-side engine.
pub mod client {
    pub use ironsbe_client::*;
}

/// Market data handling patterns.
pub mod marketdata {
    pub use ironsbe_marketdata::*;
}

// Re-export commonly used items at the crate root
pub use ironsbe_core::{
    buffer::{AlignedBuffer, BufferPool, ReadBuffer, WriteBuffer},
    decoder::{DecodeError, SbeDecoder},
    encoder::SbeEncoder,
    header::{GroupHeader, MessageHeader, VarDataHeader},
};

pub use ironsbe_channel::{broadcast, mpsc, spsc};

pub use ironsbe_client::{ClientBuilder, ClientHandle};
pub use ironsbe_server::{ServerBuilder, ServerHandle};
