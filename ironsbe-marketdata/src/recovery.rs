//! Recovery mechanisms for market data gaps.

use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Recovery request for missing sequences.
#[derive(Debug, Clone)]
pub struct RecoveryRequest {
    /// Instrument identifier.
    pub instrument_id: u64,
    /// Start sequence (inclusive).
    pub start_seq: u64,
    /// End sequence (inclusive).
    pub end_seq: u64,
    /// Time when request was created.
    pub created_at: Instant,
}

/// Manages recovery state and requests.
pub struct RecoveryManager {
    /// Pending recovery requests.
    pending: Vec<RecoveryRequest>,
    /// Instruments currently in recovery.
    recovering: HashSet<u64>,
    /// Recovery timeout.
    timeout: Duration,
}

impl RecoveryManager {
    /// Creates a new recovery manager.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            pending: Vec::new(),
            recovering: HashSet::new(),
            timeout,
        }
    }

    /// Requests recovery for a gap.
    pub fn request_recovery(&mut self, instrument_id: u64, start_seq: u64, end_seq: u64) {
        if !self.recovering.contains(&instrument_id) {
            self.recovering.insert(instrument_id);
            self.pending.push(RecoveryRequest {
                instrument_id,
                start_seq,
                end_seq,
                created_at: Instant::now(),
            });
        }
    }

    /// Marks recovery as complete for an instrument.
    pub fn complete_recovery(&mut self, instrument_id: u64) {
        self.recovering.remove(&instrument_id);
        self.pending.retain(|r| r.instrument_id != instrument_id);
    }

    /// Returns pending recovery requests.
    #[must_use]
    pub fn pending_requests(&self) -> &[RecoveryRequest] {
        &self.pending
    }

    /// Checks for timed out requests and returns them.
    pub fn check_timeouts(&mut self) -> Vec<RecoveryRequest> {
        let now = Instant::now();
        let mut timed_out = Vec::new();

        self.pending.retain(|r| {
            if now.duration_since(r.created_at) > self.timeout {
                timed_out.push(r.clone());
                false
            } else {
                true
            }
        });

        for r in &timed_out {
            self.recovering.remove(&r.instrument_id);
        }

        timed_out
    }

    /// Returns true if an instrument is in recovery.
    #[must_use]
    pub fn is_recovering(&self, instrument_id: u64) -> bool {
        self.recovering.contains(&instrument_id)
    }

    /// Returns the number of instruments in recovery.
    #[must_use]
    pub fn recovery_count(&self) -> usize {
        self.recovering.len()
    }
}

impl Default for RecoveryManager {
    fn default() -> Self {
        Self::new(Duration::from_secs(5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_recovery() {
        let mut manager = RecoveryManager::new(Duration::from_secs(5));

        manager.request_recovery(1, 10, 20);
        assert!(manager.is_recovering(1));
        assert_eq!(manager.pending_requests().len(), 1);

        // Duplicate request should be ignored
        manager.request_recovery(1, 10, 20);
        assert_eq!(manager.pending_requests().len(), 1);
    }

    #[test]
    fn test_complete_recovery() {
        let mut manager = RecoveryManager::new(Duration::from_secs(5));

        manager.request_recovery(1, 10, 20);
        manager.complete_recovery(1);

        assert!(!manager.is_recovering(1));
        assert!(manager.pending_requests().is_empty());
    }

    #[test]
    fn test_multiple_instruments() {
        let mut manager = RecoveryManager::new(Duration::from_secs(5));

        manager.request_recovery(1, 10, 20);
        manager.request_recovery(2, 5, 15);

        assert_eq!(manager.recovery_count(), 2);

        manager.complete_recovery(1);
        assert_eq!(manager.recovery_count(), 1);
        assert!(manager.is_recovering(2));
    }
}
