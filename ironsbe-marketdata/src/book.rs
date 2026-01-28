//! Order book management.

use std::collections::BTreeMap;

/// Price level in order book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceLevel {
    /// Fixed-point price.
    pub price: i64,
    /// Total quantity at this level.
    pub quantity: u64,
    /// Number of orders at this level.
    pub order_count: u32,
}

/// Order book side (bid or ask).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// Bid (buy) side.
    Bid,
    /// Ask (sell) side.
    Ask,
}

/// One side of the order book.
#[derive(Debug)]
pub struct BookSide {
    levels: BTreeMap<i64, PriceLevel>,
    is_bid: bool,
}

impl BookSide {
    /// Creates a new book side.
    #[must_use]
    pub fn new(is_bid: bool) -> Self {
        Self {
            levels: BTreeMap::new(),
            is_bid,
        }
    }

    /// Applies an update to the book side.
    #[inline]
    pub fn update(&mut self, price: i64, quantity: u64, order_count: u32) {
        if quantity == 0 {
            self.levels.remove(&price);
        } else {
            self.levels.insert(
                price,
                PriceLevel {
                    price,
                    quantity,
                    order_count,
                },
            );
        }
    }

    /// Returns the top of book (best price).
    #[inline]
    #[must_use]
    pub fn top(&self) -> Option<&PriceLevel> {
        if self.is_bid {
            self.levels.values().next_back()
        } else {
            self.levels.values().next()
        }
    }

    /// Returns the N best levels.
    #[must_use]
    pub fn best_n(&self, n: usize) -> Vec<&PriceLevel> {
        if self.is_bid {
            self.levels.values().rev().take(n).collect()
        } else {
            self.levels.values().take(n).collect()
        }
    }

    /// Returns the level at a specific price.
    #[must_use]
    pub fn get(&self, price: i64) -> Option<&PriceLevel> {
        self.levels.get(&price)
    }

    /// Clears all levels.
    pub fn clear(&mut self) {
        self.levels.clear();
    }

    /// Returns the number of price levels.
    #[must_use]
    pub fn len(&self) -> usize {
        self.levels.len()
    }

    /// Returns true if there are no levels.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }

    /// Iterates over all levels in price order.
    pub fn iter(&self) -> impl Iterator<Item = &PriceLevel> {
        self.levels.values()
    }
}

/// Full order book for an instrument.
#[derive(Debug)]
pub struct OrderBook {
    /// Instrument identifier.
    pub instrument_id: u64,
    /// Bid side.
    pub bids: BookSide,
    /// Ask side.
    pub asks: BookSide,
    /// Last update sequence number.
    pub last_update_seq: u64,
    /// Last update timestamp (nanoseconds).
    pub last_update_time: u64,
}

impl OrderBook {
    /// Creates a new order book for the given instrument.
    #[must_use]
    pub fn new(instrument_id: u64) -> Self {
        Self {
            instrument_id,
            bids: BookSide::new(true),
            asks: BookSide::new(false),
            last_update_seq: 0,
            last_update_time: 0,
        }
    }

    /// Returns the bid-ask spread.
    #[inline]
    #[must_use]
    pub fn spread(&self) -> Option<i64> {
        match (self.bids.top(), self.asks.top()) {
            (Some(bid), Some(ask)) => Some(ask.price - bid.price),
            _ => None,
        }
    }

    /// Returns the mid price.
    #[inline]
    #[must_use]
    pub fn mid_price(&self) -> Option<i64> {
        match (self.bids.top(), self.asks.top()) {
            (Some(bid), Some(ask)) => Some((bid.price + ask.price) / 2),
            _ => None,
        }
    }

    /// Returns the best bid price.
    #[inline]
    #[must_use]
    pub fn best_bid(&self) -> Option<i64> {
        self.bids.top().map(|l| l.price)
    }

    /// Returns the best ask price.
    #[inline]
    #[must_use]
    pub fn best_ask(&self) -> Option<i64> {
        self.asks.top().map(|l| l.price)
    }

    /// Applies an incremental update.
    pub fn apply_update(&mut self, update: &BookUpdate) {
        match update.side {
            Side::Bid => self
                .bids
                .update(update.price, update.quantity, update.order_count),
            Side::Ask => self
                .asks
                .update(update.price, update.quantity, update.order_count),
        }
        self.last_update_seq = update.seq_num;
    }

    /// Applies a snapshot (replaces entire book).
    pub fn apply_snapshot(&mut self, snapshot: &BookSnapshot) {
        self.bids.clear();
        self.asks.clear();

        for level in &snapshot.bids {
            self.bids
                .update(level.price, level.quantity, level.order_count);
        }

        for level in &snapshot.asks {
            self.asks
                .update(level.price, level.quantity, level.order_count);
        }

        self.last_update_seq = snapshot.seq_num;
    }

    /// Clears the entire book.
    pub fn clear(&mut self) {
        self.bids.clear();
        self.asks.clear();
        self.last_update_seq = 0;
        self.last_update_time = 0;
    }
}

/// Incremental book update.
#[derive(Debug, Clone)]
pub struct BookUpdate {
    /// Instrument identifier.
    pub instrument_id: u64,
    /// Sequence number.
    pub seq_num: u64,
    /// Side (bid or ask).
    pub side: Side,
    /// Price level.
    pub price: i64,
    /// Quantity (0 = delete level).
    pub quantity: u64,
    /// Number of orders.
    pub order_count: u32,
}

/// Book snapshot.
#[derive(Debug, Clone)]
pub struct BookSnapshot {
    /// Instrument identifier.
    pub instrument_id: u64,
    /// Sequence number.
    pub seq_num: u64,
    /// Bid levels.
    pub bids: Vec<PriceLevel>,
    /// Ask levels.
    pub asks: Vec<PriceLevel>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_book_side_update() {
        let mut side = BookSide::new(true);

        side.update(100, 50, 2);
        assert_eq!(side.len(), 1);
        assert_eq!(side.top().unwrap().price, 100);

        side.update(101, 30, 1);
        assert_eq!(side.len(), 2);
        assert_eq!(side.top().unwrap().price, 101); // Best bid is highest

        // Delete level
        side.update(101, 0, 0);
        assert_eq!(side.len(), 1);
        assert_eq!(side.top().unwrap().price, 100);
    }

    #[test]
    fn test_order_book_spread() {
        let mut book = OrderBook::new(1);

        book.bids.update(100, 50, 1);
        book.asks.update(102, 30, 1);

        assert_eq!(book.spread(), Some(2));
        assert_eq!(book.mid_price(), Some(101));
    }

    #[test]
    fn test_order_book_update() {
        let mut book = OrderBook::new(1);

        let update = BookUpdate {
            instrument_id: 1,
            seq_num: 1,
            side: Side::Bid,
            price: 100,
            quantity: 50,
            order_count: 2,
        };

        book.apply_update(&update);
        assert_eq!(book.best_bid(), Some(100));
        assert_eq!(book.last_update_seq, 1);
    }

    #[test]
    fn test_order_book_snapshot() {
        let mut book = OrderBook::new(1);

        // Add some data
        book.bids.update(100, 50, 1);
        book.asks.update(102, 30, 1);

        // Apply snapshot
        let snapshot = BookSnapshot {
            instrument_id: 1,
            seq_num: 100,
            bids: vec![PriceLevel {
                price: 99,
                quantity: 100,
                order_count: 5,
            }],
            asks: vec![PriceLevel {
                price: 101,
                quantity: 80,
                order_count: 3,
            }],
        };

        book.apply_snapshot(&snapshot);

        assert_eq!(book.best_bid(), Some(99));
        assert_eq!(book.best_ask(), Some(101));
        assert_eq!(book.last_update_seq, 100);
    }

    #[test]
    fn test_best_n_levels() {
        let mut side = BookSide::new(false); // Ask side

        side.update(100, 10, 1);
        side.update(101, 20, 1);
        side.update(102, 30, 1);
        side.update(103, 40, 1);

        let best = side.best_n(2);
        assert_eq!(best.len(), 2);
        assert_eq!(best[0].price, 100); // Best ask is lowest
        assert_eq!(best[1].price, 101);
    }

    #[test]
    fn test_side_enum() {
        assert_eq!(Side::Bid, Side::Bid);
        assert_ne!(Side::Bid, Side::Ask);

        let debug_str = format!("{:?}", Side::Bid);
        assert!(debug_str.contains("Bid"));
    }

    #[test]
    fn test_price_level_clone() {
        let level = PriceLevel {
            price: 100,
            quantity: 50,
            order_count: 2,
        };
        let cloned = level;
        assert_eq!(level.price, cloned.price);
        assert_eq!(level.quantity, cloned.quantity);
        assert_eq!(level.order_count, cloned.order_count);
    }

    #[test]
    fn test_book_side_clear() {
        let mut side = BookSide::new(true);
        side.update(100, 50, 1);
        side.update(101, 30, 1);
        assert_eq!(side.len(), 2);

        side.clear();
        assert_eq!(side.len(), 0);
        assert!(side.top().is_none());
    }

    #[test]
    fn test_order_book_clear() {
        let mut book = OrderBook::new(1);
        book.bids.update(100, 50, 1);
        book.asks.update(102, 30, 1);

        book.clear();
        assert!(book.best_bid().is_none());
        assert!(book.best_ask().is_none());
    }

    #[test]
    fn test_book_update_clone() {
        let update = BookUpdate {
            instrument_id: 1,
            seq_num: 100,
            side: Side::Bid,
            price: 1000,
            quantity: 50,
            order_count: 2,
        };
        let cloned = update.clone();
        assert_eq!(update.instrument_id, cloned.instrument_id);
        assert_eq!(update.seq_num, cloned.seq_num);
    }

    #[test]
    fn test_book_snapshot_clone() {
        let snapshot = BookSnapshot {
            instrument_id: 1,
            seq_num: 100,
            bids: vec![PriceLevel {
                price: 99,
                quantity: 100,
                order_count: 5,
            }],
            asks: vec![],
        };
        let cloned = snapshot.clone();
        assert_eq!(snapshot.instrument_id, cloned.instrument_id);
        assert_eq!(snapshot.bids.len(), cloned.bids.len());
    }

    #[test]
    fn test_order_book_empty_spread() {
        let book = OrderBook::new(1);
        assert!(book.spread().is_none());
        assert!(book.mid_price().is_none());
    }

    #[test]
    fn test_book_side_update_existing_level() {
        let mut side = BookSide::new(true);
        side.update(100, 50, 2);
        assert_eq!(side.top().unwrap().quantity, 50);

        // Update same price level
        side.update(100, 75, 3);
        assert_eq!(side.len(), 1);
        assert_eq!(side.top().unwrap().quantity, 75);
        assert_eq!(side.top().unwrap().order_count, 3);
    }
}
