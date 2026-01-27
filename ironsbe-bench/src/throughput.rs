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
