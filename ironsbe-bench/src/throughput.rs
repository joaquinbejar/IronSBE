//! Throughput benchmarks.

use std::time::{Duration, Instant};

/// Result of a throughput benchmark.
#[derive(Debug, Clone)]
pub struct ThroughputResult {
    /// Total messages processed.
    pub messages: u64,
    /// Total bytes processed.
    pub bytes: u64,
    /// Total duration.
    pub duration: Duration,
}

impl ThroughputResult {
    /// Returns messages per second.
    #[must_use]
    pub fn messages_per_second(&self) -> f64 {
        self.messages as f64 / self.duration.as_secs_f64()
    }

    /// Returns bytes per second.
    #[must_use]
    pub fn bytes_per_second(&self) -> f64 {
        self.bytes as f64 / self.duration.as_secs_f64()
    }

    /// Returns megabytes per second.
    #[must_use]
    pub fn mb_per_second(&self) -> f64 {
        self.bytes_per_second() / (1024.0 * 1024.0)
    }
}

/// Runs a throughput benchmark.
pub fn run_throughput_benchmark<F>(
    message_count: u64,
    message_size: usize,
    mut process_fn: F,
) -> ThroughputResult
where
    F: FnMut(),
{
    let start = Instant::now();

    for _ in 0..message_count {
        process_fn();
    }

    let duration = start.elapsed();

    ThroughputResult {
        messages: message_count,
        bytes: message_count * message_size as u64,
        duration,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_throughput_result_messages_per_second() {
        let result = ThroughputResult {
            messages: 1000,
            bytes: 10000,
            duration: Duration::from_secs(1),
        };
        assert!((result.messages_per_second() - 1000.0).abs() < 0.001);
    }

    #[test]
    fn test_throughput_result_bytes_per_second() {
        let result = ThroughputResult {
            messages: 1000,
            bytes: 10000,
            duration: Duration::from_secs(1),
        };
        assert!((result.bytes_per_second() - 10000.0).abs() < 0.001);
    }

    #[test]
    fn test_throughput_result_mb_per_second() {
        let result = ThroughputResult {
            messages: 1000,
            bytes: 1024 * 1024,
            duration: Duration::from_secs(1),
        };
        assert!((result.mb_per_second() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_run_throughput_benchmark() {
        let mut counter = 0u64;
        let result = run_throughput_benchmark(100, 64, || {
            counter += 1;
        });
        assert_eq!(result.messages, 100);
        assert_eq!(result.bytes, 6400);
        assert_eq!(counter, 100);
    }

    #[test]
    fn test_throughput_result_clone_debug() {
        let result = ThroughputResult {
            messages: 100,
            bytes: 1000,
            duration: Duration::from_millis(100),
        };
        let cloned = result.clone();
        assert_eq!(result.messages, cloned.messages);

        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("ThroughputResult"));
    }
}
