//! # IronSBE Market Data
//!
//! Market data handling patterns for financial systems.
//!
//! This crate provides:
//! - Order book management with bid/ask sides
//! - Snapshot and incremental update handling
//! - Gap detection and recovery
//! - A/B feed arbitration

pub mod arbitration;
pub mod book;
pub mod handler;
pub mod instruments;
pub mod recovery;

pub use book::{BookSide, BookSnapshot, BookUpdate, OrderBook, PriceLevel, Side};
pub use handler::{InstrumentState, MarketDataEvent, MarketDataHandler};
