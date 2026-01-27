//! Latency measurement utilities.

use std::time::{Duration, Instant};

/// Latency statistics.
#[derive(Debug, Clone)]
pub struct LatencyStats {
    /// Minimum latency.
    pub min: Duration,
    /// Maximum latency.
    pub max: Duration,
    /// Mean latency.
    pub mean: Duration,
    /// Median latency (p50).
    pub median: Duration,
    /// 99th percentile latency.
    pub p99: Duration,
    /// 99.9th percentile latency.
    pub p999: Duration,
    /// Sample count.
    pub count: usize,
}

/// Collects latency samples and computes statistics.
pub struct LatencyCollector {
    samples: Vec<Duration>,
}

impl LatencyCollector {
    /// Creates a new latency collector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
        }
    }

    /// Creates a new latency collector with pre-allocated capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
        }
    }

    /// Records a latency sample.
    pub fn record(&mut self, latency: Duration) {
        self.samples.push(latency);
    }

    /// Measures the latency of a function.
    pub fn measure<F, T>(&mut self, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let start = Instant::now();
        let result = f();
        self.samples.push(start.elapsed());
        result
    }

    /// Computes statistics from collected samples.
    #[must_use]
    pub fn stats(&mut self) -> Option<LatencyStats> {
        if self.samples.is_empty() {
            return None;
        }

        self.samples.sort();

        let count = self.samples.len();
        let min = self.samples[0];
        let max = self.samples[count - 1];
        let median = self.samples[count / 2];
        let p99 = self.samples[(count as f64 * 0.99) as usize];
        let p999 = self.samples[(count as f64 * 0.999).min((count - 1) as f64) as usize];

        let total: Duration = self.samples.iter().sum();
        let mean = total / count as u32;

        Some(LatencyStats {
            min,
            max,
            mean,
            median,
            p99,
            p999,
            count,
        })
    }

    /// Clears all samples.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Returns the number of samples.
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Returns true if no samples have been collected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

impl Default for LatencyCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_stats() {
        let mut collector = LatencyCollector::new();

        for i in 1..=100 {
            collector.record(Duration::from_nanos(i * 100));
        }

        let stats = collector.stats().unwrap();
        assert_eq!(stats.count, 100);
        assert_eq!(stats.min, Duration::from_nanos(100));
        assert_eq!(stats.max, Duration::from_nanos(10000));
    }

    #[test]
    fn test_measure() {
        let mut collector = LatencyCollector::new();

        let result = collector.measure(|| 42);
        assert_eq!(result, 42);
        assert_eq!(collector.len(), 1);
    }
}
