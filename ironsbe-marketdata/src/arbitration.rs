//! A/B feed arbitration for market data.

use std::collections::HashMap;

/// Arbitrator for A/B feed deduplication at the instrument level.
pub struct InstrumentArbitrator {
    /// Last processed sequence per instrument.
    last_seq: HashMap<u64, u64>,
    /// Expected sequence per instrument.
    expected_seq: HashMap<u64, u64>,
}

impl InstrumentArbitrator {
    /// Creates a new instrument arbitrator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            last_seq: HashMap::new(),
            expected_seq: HashMap::new(),
        }
    }

    /// Checks if a message should be processed.
    ///
    /// # Arguments
    /// * `instrument_id` - Instrument identifier
    /// * `seq` - Sequence number
    ///
    /// # Returns
    /// `true` if this is a new message that should be processed.
    pub fn should_process(&mut self, instrument_id: u64, seq: u64) -> bool {
        let last = self.last_seq.get(&instrument_id).copied().unwrap_or(0);

        if seq <= last {
            return false;
        }

        self.last_seq.insert(instrument_id, seq);
        true
    }

    /// Checks for gaps in the sequence.
    ///
    /// # Returns
    /// `Some((start, end))` if a gap was detected.
    pub fn check_gap(&mut self, instrument_id: u64, seq: u64) -> Option<(u64, u64)> {
        let expected = self.expected_seq.get(&instrument_id).copied().unwrap_or(1);

        if seq > expected {
            let gap = (expected, seq - 1);
            self.expected_seq.insert(instrument_id, seq + 1);
            Some(gap)
        } else {
            if seq == expected {
                self.expected_seq.insert(instrument_id, seq + 1);
            }
            None
        }
    }

    /// Resets state for an instrument.
    pub fn reset(&mut self, instrument_id: u64) {
        self.last_seq.remove(&instrument_id);
        self.expected_seq.remove(&instrument_id);
    }

    /// Resets all state.
    pub fn reset_all(&mut self) {
        self.last_seq.clear();
        self.expected_seq.clear();
    }
}

impl Default for InstrumentArbitrator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deduplication() {
        let mut arb = InstrumentArbitrator::new();

        assert!(arb.should_process(1, 1));
        assert!(!arb.should_process(1, 1)); // Duplicate
        assert!(arb.should_process(1, 2));
        assert!(!arb.should_process(1, 1)); // Old
    }

    #[test]
    fn test_multiple_instruments() {
        let mut arb = InstrumentArbitrator::new();

        assert!(arb.should_process(1, 1));
        assert!(arb.should_process(2, 1)); // Different instrument
        assert!(!arb.should_process(1, 1));
        assert!(!arb.should_process(2, 1));
    }

    #[test]
    fn test_gap_detection() {
        let mut arb = InstrumentArbitrator::new();

        assert!(arb.check_gap(1, 1).is_none());
        assert_eq!(arb.check_gap(1, 5), Some((2, 4)));
        assert!(arb.check_gap(1, 6).is_none());
    }
}
