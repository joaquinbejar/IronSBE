//! Instrument definitions and management.

use std::collections::HashMap;

/// Instrument definition.
#[derive(Debug, Clone)]
pub struct Instrument {
    /// Unique instrument identifier.
    pub id: u64,
    /// Symbol/ticker.
    pub symbol: String,
    /// Security type.
    pub security_type: SecurityType,
    /// Price tick size (minimum price increment).
    pub tick_size: i64,
    /// Contract multiplier.
    pub multiplier: i64,
    /// Currency code.
    pub currency: String,
    /// Exchange code.
    pub exchange: String,
    /// Whether the instrument is active.
    pub is_active: bool,
}

/// Security type enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityType {
    /// Equity/Stock.
    Equity,
    /// Future contract.
    Future,
    /// Option contract.
    Option,
    /// Foreign exchange.
    Forex,
    /// Index.
    Index,
    /// Other/Unknown.
    Other,
}

/// Manages instrument definitions.
pub struct InstrumentManager {
    instruments: HashMap<u64, Instrument>,
    symbol_index: HashMap<String, u64>,
}

impl InstrumentManager {
    /// Creates a new instrument manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            instruments: HashMap::new(),
            symbol_index: HashMap::new(),
        }
    }

    /// Adds an instrument.
    pub fn add(&mut self, instrument: Instrument) {
        self.symbol_index
            .insert(instrument.symbol.clone(), instrument.id);
        self.instruments.insert(instrument.id, instrument);
    }

    /// Gets an instrument by ID.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Instrument> {
        self.instruments.get(&id)
    }

    /// Gets an instrument by symbol.
    #[must_use]
    pub fn get_by_symbol(&self, symbol: &str) -> Option<&Instrument> {
        self.symbol_index
            .get(symbol)
            .and_then(|id| self.instruments.get(id))
    }

    /// Removes an instrument.
    pub fn remove(&mut self, id: u64) -> Option<Instrument> {
        if let Some(inst) = self.instruments.remove(&id) {
            self.symbol_index.remove(&inst.symbol);
            Some(inst)
        } else {
            None
        }
    }

    /// Returns all instrument IDs.
    #[must_use]
    pub fn ids(&self) -> Vec<u64> {
        self.instruments.keys().copied().collect()
    }

    /// Returns the number of instruments.
    #[must_use]
    pub fn len(&self) -> usize {
        self.instruments.len()
    }

    /// Returns true if there are no instruments.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instruments.is_empty()
    }

    /// Iterates over all instruments.
    pub fn iter(&self) -> impl Iterator<Item = &Instrument> {
        self.instruments.values()
    }
}

impl Default for InstrumentManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instrument_manager() {
        let mut manager = InstrumentManager::new();

        let inst = Instrument {
            id: 1,
            symbol: "ESH5".to_string(),
            security_type: SecurityType::Future,
            tick_size: 25,
            multiplier: 50,
            currency: "USD".to_string(),
            exchange: "CME".to_string(),
            is_active: true,
        };

        manager.add(inst);

        assert_eq!(manager.len(), 1);
        assert!(manager.get(1).is_some());
        assert!(manager.get_by_symbol("ESH5").is_some());
        assert_eq!(manager.get_by_symbol("ESH5").unwrap().tick_size, 25);
    }

    #[test]
    fn test_remove_instrument() {
        let mut manager = InstrumentManager::new();

        manager.add(Instrument {
            id: 1,
            symbol: "TEST".to_string(),
            security_type: SecurityType::Equity,
            tick_size: 1,
            multiplier: 1,
            currency: "USD".to_string(),
            exchange: "NYSE".to_string(),
            is_active: true,
        });

        assert!(manager.remove(1).is_some());
        assert!(manager.get(1).is_none());
        assert!(manager.get_by_symbol("TEST").is_none());
    }
}
