//! Reconnection logic for client connections.

use std::time::Duration;

/// Configuration for reconnection behavior.
#[derive(Debug, Clone)]
pub struct ReconnectConfig {
    /// Whether reconnection is enabled.
    pub enabled: bool,
    /// Initial delay before first reconnect attempt.
    pub initial_delay: Duration,
    /// Maximum delay between reconnect attempts.
    pub max_delay: Duration,
    /// Multiplier for exponential backoff.
    pub backoff_multiplier: f64,
    /// Maximum number of reconnect attempts (0 = unlimited).
    pub max_attempts: usize,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            max_attempts: 10,
        }
    }
}

/// Tracks reconnection state and calculates delays.
pub struct ReconnectState {
    config: ReconnectConfig,
    attempts: usize,
    current_delay: Duration,
}

impl ReconnectState {
    /// Creates a new reconnect state with the given configuration.
    #[must_use]
    pub fn new(config: ReconnectConfig) -> Self {
        let initial_delay = config.initial_delay;
        Self {
            config,
            attempts: 0,
            current_delay: initial_delay,
        }
    }

    /// Records a failed connection attempt and returns the delay before next attempt.
    ///
    /// Returns `None` if max attempts reached or reconnection is disabled.
    pub fn on_failure(&mut self) -> Option<Duration> {
        if !self.config.enabled {
            return None;
        }

        self.attempts += 1;

        if self.config.max_attempts > 0 && self.attempts >= self.config.max_attempts {
            return None;
        }

        let delay = self.current_delay;

        // Calculate next delay with exponential backoff
        let next_delay = Duration::from_secs_f64(
            self.current_delay.as_secs_f64() * self.config.backoff_multiplier,
        );
        self.current_delay = next_delay.min(self.config.max_delay);

        Some(delay)
    }

    /// Resets the reconnection state after a successful connection.
    pub fn on_success(&mut self) {
        self.attempts = 0;
        self.current_delay = self.config.initial_delay;
    }

    /// Returns the number of reconnection attempts made.
    #[must_use]
    pub fn attempts(&self) -> usize {
        self.attempts
    }

    /// Returns true if more reconnection attempts are allowed.
    #[must_use]
    pub fn can_retry(&self) -> bool {
        self.config.enabled
            && (self.config.max_attempts == 0 || self.attempts < self.config.max_attempts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconnect_backoff() {
        let config = ReconnectConfig {
            enabled: true,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            max_attempts: 5,
        };

        let mut state = ReconnectState::new(config);

        // First failure
        let delay = state.on_failure().unwrap();
        assert_eq!(delay, Duration::from_millis(100));

        // Second failure
        let delay = state.on_failure().unwrap();
        assert_eq!(delay, Duration::from_millis(200));

        // Third failure
        let delay = state.on_failure().unwrap();
        assert_eq!(delay, Duration::from_millis(400));
    }

    #[test]
    fn test_reconnect_max_attempts() {
        let config = ReconnectConfig {
            enabled: true,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            max_attempts: 2,
        };

        let mut state = ReconnectState::new(config);

        assert!(state.on_failure().is_some());
        assert!(state.on_failure().is_none()); // Max reached
    }

    #[test]
    fn test_reconnect_reset() {
        let config = ReconnectConfig::default();
        let mut state = ReconnectState::new(config);

        state.on_failure();
        state.on_failure();
        assert_eq!(state.attempts(), 2);

        state.on_success();
        assert_eq!(state.attempts(), 0);
    }

    #[test]
    fn test_reconnect_disabled() {
        let config = ReconnectConfig {
            enabled: false,
            ..Default::default()
        };

        let mut state = ReconnectState::new(config);
        assert!(state.on_failure().is_none());
    }
}
