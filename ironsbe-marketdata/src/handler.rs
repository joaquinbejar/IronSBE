//! Market data handler with recovery support.

use crate::book::{BookSnapshot, BookUpdate, OrderBook};
use ironsbe_channel::spsc::SpscSender;
use std::collections::HashMap;

/// State of an instrument's market data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentState {
    /// Waiting for initial snapshot.
    Initializing,
    /// Receiving incremental updates normally.
    Active,
    /// Detected gap, waiting for recovery.
    Recovering,
    /// Stale data (no updates for timeout period).
    Stale,
}

/// Events emitted by the market data handler.
#[derive(Debug)]
pub enum MarketDataEvent {
    /// Book was updated.
    BookUpdated(u64),
    /// Top of book changed.
    TopOfBookChanged(u64),
    /// Instrument state changed.
    StateChanged(u64, InstrumentState),
    /// Gap detected in sequence.
    GapDetected(u64, u64, u64),
}

/// Market data handler following CME MDP 3.0 patterns.
pub struct MarketDataHandler {
    books: HashMap<u64, OrderBook>,
    states: HashMap<u64, InstrumentState>,
    expected_seq: HashMap<u64, u64>,
    update_tx: SpscSender<MarketDataEvent>,
    pending_incrementals: HashMap<u64, Vec<BookUpdate>>,
}

impl MarketDataHandler {
    /// Creates a new market data handler.
    #[must_use]
    pub fn new(update_tx: SpscSender<MarketDataEvent>) -> Self {
        Self {
            books: HashMap::new(),
            states: HashMap::new(),
            expected_seq: HashMap::new(),
            update_tx,
            pending_incrementals: HashMap::new(),
        }
    }

    /// Subscribes to an instrument.
    pub fn subscribe(&mut self, instrument_id: u64) {
        self.books
            .insert(instrument_id, OrderBook::new(instrument_id));
        self.states
            .insert(instrument_id, InstrumentState::Initializing);
        self.expected_seq.insert(instrument_id, 0);
    }

    /// Unsubscribes from an instrument.
    pub fn unsubscribe(&mut self, instrument_id: u64) {
        self.books.remove(&instrument_id);
        self.states.remove(&instrument_id);
        self.expected_seq.remove(&instrument_id);
        self.pending_incrementals.remove(&instrument_id);
    }

    /// Processes an incremental update from the feed.
    ///
    /// # Errors
    /// Returns error if processing fails.
    pub fn on_incremental(&mut self, update: BookUpdate) -> Result<(), HandlerError> {
        let instrument_id = update.instrument_id;

        let state = self
            .states
            .get(&instrument_id)
            .copied()
            .unwrap_or(InstrumentState::Initializing);

        match state {
            InstrumentState::Initializing => {
                // Queue incrementals until snapshot arrives
                self.pending_incrementals
                    .entry(instrument_id)
                    .or_default()
                    .push(update);
            }

            InstrumentState::Active => {
                let expected = self.expected_seq.get(&instrument_id).copied().unwrap_or(0);

                if update.seq_num < expected {
                    // Old message, ignore
                    return Ok(());
                }

                if update.seq_num > expected {
                    // Gap detected
                    let _ = self.update_tx.send(MarketDataEvent::GapDetected(
                        instrument_id,
                        expected,
                        update.seq_num,
                    ));

                    self.states
                        .insert(instrument_id, InstrumentState::Recovering);
                    let _ = self.update_tx.send(MarketDataEvent::StateChanged(
                        instrument_id,
                        InstrumentState::Recovering,
                    ));

                    // Queue this update
                    self.pending_incrementals
                        .entry(instrument_id)
                        .or_default()
                        .push(update);

                    return Ok(());
                }

                // Normal case: apply update
                self.apply_update(update)?;
            }

            InstrumentState::Recovering => {
                // Queue during recovery
                self.pending_incrementals
                    .entry(instrument_id)
                    .or_default()
                    .push(update);
            }

            InstrumentState::Stale => {
                // Transition to recovering
                self.states
                    .insert(instrument_id, InstrumentState::Recovering);
                self.pending_incrementals
                    .entry(instrument_id)
                    .or_default()
                    .push(update);
            }
        }

        Ok(())
    }

    /// Processes a snapshot from the recovery feed.
    ///
    /// # Errors
    /// Returns error if processing fails.
    pub fn on_snapshot(&mut self, snapshot: BookSnapshot) -> Result<(), HandlerError> {
        let instrument_id = snapshot.instrument_id;

        if let Some(book) = self.books.get_mut(&instrument_id) {
            book.apply_snapshot(&snapshot);

            // Update expected sequence
            self.expected_seq
                .insert(instrument_id, snapshot.seq_num + 1);

            // Apply any queued incrementals newer than snapshot
            if let Some(pending) = self.pending_incrementals.remove(&instrument_id) {
                for update in pending {
                    if update.seq_num >= snapshot.seq_num {
                        self.apply_update(update)?;
                    }
                }
            }

            // Transition to active
            self.states.insert(instrument_id, InstrumentState::Active);
            let _ = self.update_tx.send(MarketDataEvent::StateChanged(
                instrument_id,
                InstrumentState::Active,
            ));
            let _ = self
                .update_tx
                .send(MarketDataEvent::BookUpdated(instrument_id));
        }

        Ok(())
    }

    fn apply_update(&mut self, update: BookUpdate) -> Result<(), HandlerError> {
        let instrument_id = update.instrument_id;
        let seq = update.seq_num;

        if let Some(book) = self.books.get_mut(&instrument_id) {
            let old_bid = book.bids.top().map(|l| l.price);
            let old_ask = book.asks.top().map(|l| l.price);

            book.apply_update(&update);

            let new_bid = book.bids.top().map(|l| l.price);
            let new_ask = book.asks.top().map(|l| l.price);

            // Update expected sequence
            self.expected_seq.insert(instrument_id, seq + 1);

            // Emit events
            let _ = self
                .update_tx
                .send(MarketDataEvent::BookUpdated(instrument_id));

            if old_bid != new_bid || old_ask != new_ask {
                let _ = self
                    .update_tx
                    .send(MarketDataEvent::TopOfBookChanged(instrument_id));
            }
        }

        Ok(())
    }

    /// Gets the order book for an instrument.
    #[must_use]
    pub fn get_book(&self, instrument_id: u64) -> Option<&OrderBook> {
        self.books.get(&instrument_id)
    }

    /// Gets the state for an instrument.
    #[must_use]
    pub fn get_state(&self, instrument_id: u64) -> Option<InstrumentState> {
        self.states.get(&instrument_id).copied()
    }

    /// Returns all subscribed instrument IDs.
    #[must_use]
    pub fn subscribed_instruments(&self) -> Vec<u64> {
        self.books.keys().copied().collect()
    }

    /// Marks an instrument as stale.
    pub fn mark_stale(&mut self, instrument_id: u64) {
        if let std::collections::hash_map::Entry::Occupied(mut e) = self.states.entry(instrument_id)
        {
            e.insert(InstrumentState::Stale);
            let _ = self.update_tx.send(MarketDataEvent::StateChanged(
                instrument_id,
                InstrumentState::Stale,
            ));
        }
    }
}

/// Error type for handler operations.
#[derive(Debug)]
pub struct HandlerError {
    /// Error message.
    pub message: String,
}

impl std::fmt::Display for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "handler error: {}", self.message)
    }
}

impl std::error::Error for HandlerError {}
