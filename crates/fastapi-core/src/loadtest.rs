//! Load testing utilities for stress testing request handlers.
//!
//! Provides a configurable load generator that spawns concurrent
//! request tasks and collects latency/error statistics.
//!
//! # Example
//!
//! ```
//! use fastapi_core::loadtest::{LoadTest, LoadTestConfig, LoadTestReport};
//! use std::time::Duration;
//!
//! let config = LoadTestConfig::new()
//!     .total_requests(1000)
//!     .concurrency(10);
//!
//! let report = LoadTest::run(&config, |i| {
//!     // Simulate request work (return Ok for success, Err for failure)
//!     if i % 100 == 99 { Err("simulated error".into()) } else { Ok(()) }
//! });
//!
//! assert!(report.success_rate() > 0.95);
//! println!("{report}");
//! ```

use std::fmt;
use std::time::{Duration, Instant};

/// Configuration for a load test.
#[derive(Debug, Clone)]
pub struct LoadTestConfig {
    /// Total number of requests to execute.
    pub total_requests: usize,
    /// Number of concurrent workers.
    pub concurrency: usize,
    /// Optional warmup requests (not counted in results).
    pub warmup: usize,
}

impl Default for LoadTestConfig {
    fn default() -> Self {
        Self {
            total_requests: 1000,
            concurrency: 1,
            warmup: 0,
        }
    }
}

impl LoadTestConfig {
    /// Create a new config with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set total number of requests.
    #[must_use]
    pub fn total_requests(mut self, n: usize) -> Self {
        self.total_requests = n;
        self
    }

    /// Set concurrency level.
    #[must_use]
    pub fn concurrency(mut self, n: usize) -> Self {
        self.concurrency = n.max(1);
        self
    }

    /// Set warmup request count.
    #[must_use]
    pub fn warmup(mut self, n: usize) -> Self {
        self.warmup = n;
        self
    }
}

/// Result of a single request in the load test.
#[derive(Debug)]
struct RequestResult {
    latency: Duration,
    success: bool,
}

/// Report from a completed load test.
#[derive(Debug, Clone)]
pub struct LoadTestReport {
    /// Total requests executed.
    pub total: usize,
    /// Successful requests.
    pub successes: usize,
    /// Failed requests.
    pub failures: usize,
    /// Total elapsed wall-clock time.
    pub elapsed: Duration,
    /// Sorted latency samples.
    latencies: Vec<Duration>,
}

impl LoadTestReport {
    /// Success rate as a fraction [0.0, 1.0].
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.successes as f64 / self.total as f64
    }

    /// Error rate as a fraction [0.0, 1.0].
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        1.0 - self.success_rate()
    }

    /// Requests per second throughput.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn rps(&self) -> f64 {
        if self.elapsed.is_zero() {
            return 0.0;
        }
        self.total as f64 / self.elapsed.as_secs_f64()
    }

    /// Get latency percentile (e.g., 0.50 for p50, 0.99 for p99).
    #[must_use]
    pub fn percentile(&self, p: f64) -> Option<Duration> {
        if self.latencies.is_empty() {
            return None;
        }
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let idx = ((p * self.latencies.len() as f64) as usize).min(self.latencies.len() - 1);
        Some(self.latencies[idx])
    }

    /// Minimum latency.
    #[must_use]
    pub fn min_latency(&self) -> Option<Duration> {
        self.latencies.first().copied()
    }

    /// Maximum latency.
    #[must_use]
    pub fn max_latency(&self) -> Option<Duration> {
        self.latencies.last().copied()
    }

    /// Mean latency.
    #[must_use]
    pub fn mean_latency(&self) -> Option<Duration> {
        if self.latencies.is_empty() {
            return None;
        }
        let sum: Duration = self.latencies.iter().sum();
        Some(sum / self.latencies.len() as u32)
    }
}

impl fmt::Display for LoadTestReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Load Test Report")?;
        writeln!(f, "  Total:    {}", self.total)?;
        writeln!(
            f,
            "  Success:  {} ({:.1}%)",
            self.successes,
            self.success_rate() * 100.0
        )?;
        writeln!(
            f,
            "  Failures: {} ({:.1}%)",
            self.failures,
            self.error_rate() * 100.0
        )?;
        writeln!(f, "  Elapsed:  {:.2?}", self.elapsed)?;
        writeln!(f, "  RPS:      {:.1}", self.rps())?;
        if let Some(p50) = self.percentile(0.50) {
            writeln!(f, "  p50:      {:.2?}", p50)?;
        }
        if let Some(p95) = self.percentile(0.95) {
            writeln!(f, "  p95:      {:.2?}", p95)?;
        }
        if let Some(p99) = self.percentile(0.99) {
            writeln!(f, "  p99:      {:.2?}", p99)?;
        }
        Ok(())
    }
}

/// Load test runner.
pub struct LoadTest;

impl LoadTest {
    /// Run a synchronous load test.
    ///
    /// The `handler` receives a request index and returns `Ok(())` on success
    /// or `Err` on failure. Concurrency is simulated via round-robin
    /// across worker batches.
    pub fn run<F>(config: &LoadTestConfig, mut handler: F) -> LoadTestReport
    where
        F: FnMut(usize) -> Result<(), Box<dyn std::error::Error>>,
    {
        // Warmup phase
        for i in 0..config.warmup {
            let _ = handler(i);
        }

        let mut results = Vec::with_capacity(config.total_requests);
        let start = Instant::now();

        // Execute requests in batches of `concurrency`
        let mut remaining = config.total_requests;
        let mut req_index = 0;
        while remaining > 0 {
            let batch_size = remaining.min(config.concurrency);
            for _ in 0..batch_size {
                let req_start = Instant::now();
                let success = handler(req_index).is_ok();
                results.push(RequestResult {
                    latency: req_start.elapsed(),
                    success,
                });
                req_index += 1;
            }
            remaining -= batch_size;
        }

        let elapsed = start.elapsed();

        let successes = results.iter().filter(|r| r.success).count();
        let failures = results.len() - successes;

        let mut latencies: Vec<Duration> = results.iter().map(|r| r.latency).collect();
        latencies.sort();

        LoadTestReport {
            total: results.len(),
            successes,
            failures,
            elapsed,
            latencies,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_load_test() {
        let config = LoadTestConfig::new().total_requests(100).concurrency(5);
        let report = LoadTest::run(&config, |_| Ok(()));
        assert_eq!(report.total, 100);
        assert_eq!(report.successes, 100);
        assert_eq!(report.failures, 0);
        assert!((report.success_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn load_test_with_failures() {
        let config = LoadTestConfig::new().total_requests(100).concurrency(1);
        let report = LoadTest::run(&config, |i| {
            if i % 10 == 0 {
                Err("fail".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(report.total, 100);
        assert_eq!(report.failures, 10);
        assert_eq!(report.successes, 90);
        assert!((report.error_rate() - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn load_test_percentiles() {
        let config = LoadTestConfig::new().total_requests(100).concurrency(1);
        let report = LoadTest::run(&config, |_| Ok(()));
        assert!(report.percentile(0.50).is_some());
        assert!(report.percentile(0.95).is_some());
        assert!(report.percentile(0.99).is_some());
        assert!(report.min_latency().is_some());
        assert!(report.max_latency().is_some());
        assert!(report.mean_latency().is_some());
    }

    #[test]
    fn load_test_rps() {
        let config = LoadTestConfig::new().total_requests(50).concurrency(10);
        let report = LoadTest::run(&config, |_| Ok(()));
        assert!(report.rps() > 0.0);
    }

    #[test]
    fn load_test_with_warmup() {
        let config = LoadTestConfig::new()
            .total_requests(50)
            .warmup(10)
            .concurrency(1);
        let report = LoadTest::run(&config, |_| Ok(()));
        // Warmup requests aren't counted
        assert_eq!(report.total, 50);
    }

    #[test]
    fn load_test_display() {
        let config = LoadTestConfig::new().total_requests(10).concurrency(1);
        let report = LoadTest::run(&config, |_| Ok(()));
        let display = format!("{report}");
        assert!(display.contains("Load Test Report"));
        assert!(display.contains("RPS:"));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn empty_report() {
        let config = LoadTestConfig::new().total_requests(0).concurrency(1);
        let report = LoadTest::run(&config, |_| Ok(()));
        assert_eq!(report.total, 0);
        assert_eq!(report.success_rate(), 0.0);
        assert_eq!(report.rps(), 0.0);
        assert!(report.percentile(0.50).is_none());
    }

    #[test]
    fn config_defaults() {
        let config = LoadTestConfig::default();
        assert_eq!(config.total_requests, 1000);
        assert_eq!(config.concurrency, 1);
        assert_eq!(config.warmup, 0);
    }

    #[test]
    fn concurrency_minimum_is_one() {
        let config = LoadTestConfig::new().concurrency(0);
        assert_eq!(config.concurrency, 1);
    }
}
