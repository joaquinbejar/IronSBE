//! # IronSBE Core
//!
//! Core types and traits for Simple Binary Encoding (SBE) implementation.
//!
//! This crate provides:
//! - Buffer traits for zero-copy read/write operations
//! - Message header types (MessageHeader, GroupHeader, VarDataHeader)
//! - Decoder and Encoder traits for SBE messages
//! - Error types for encoding/decoding operations
//! - Aligned buffer implementations for optimal performance

pub mod buffer;
pub mod decoder;
pub mod encoder;
pub mod error;
pub mod header;
pub mod types;

pub use buffer::{AlignedBuffer, BufferPool, ReadBuffer, WriteBuffer};
pub use decoder::{DecodeError, SbeDecoder};
pub use encoder::SbeEncoder;
pub use error::{Error, Result};
pub use header::{GroupHeader, MessageHeader, VarDataHeader};
