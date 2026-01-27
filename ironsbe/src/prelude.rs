//! Prelude module for convenient imports.
//!
//! This module re-exports the most commonly used types and traits.
//!
//! ```ignore
//! use ironsbe::prelude::*;
//! ```

// Core types
pub use ironsbe_core::buffer::{AlignedBuffer, BufferPool, ReadBuffer, WriteBuffer};
pub use ironsbe_core::decoder::{DecodeError, SbeDecoder};
pub use ironsbe_core::encoder::SbeEncoder;
pub use ironsbe_core::error::{Error as CoreError, Result as CoreResult};
pub use ironsbe_core::header::{GroupHeader, MessageHeader, VarDataHeader};

// Channel types
pub use ironsbe_channel::{MpscReceiver, MpscSender, SpscReceiver, SpscSender};
pub use ironsbe_channel::{broadcast, mpsc, spsc};

// Server types
pub use ironsbe_server::{
    MessageDispatcher, MessageHandler, Responder, Server, ServerBuilder, ServerCommand,
    ServerEvent, ServerHandle, SessionManager,
};

// Client types
pub use ironsbe_client::{
    Client, ClientBuilder, ClientCommand, ClientError, ClientEvent, ClientHandle,
};

// Market data types
pub use ironsbe_marketdata::{
    BookSide, BookSnapshot, BookUpdate, InstrumentState, MarketDataEvent, MarketDataHandler,
    OrderBook, PriceLevel, Side,
};
