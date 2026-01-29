//! Code coverage integration for fastapi_rust.
//!
//! This module provides coverage tracking and reporting infrastructure for
//! testing fastapi applications. It integrates with `cargo-llvm-cov` for
//! line-level coverage and provides per-endpoint coverage tracking.
//!
//! # Features
//!
//! - **Endpoint coverage tracking**: Track which routes are tested
//! - **Branch coverage hints**: Track error paths and edge cases
//! - **Threshold enforcement**: Fail tests if coverage drops below threshold
//! - **Report generation**: JSON, HTML, and badge formats
//!
//! # Example
//!
//! ```ignore
//! use fastapi_core::coverage::{CoverageTracker, CoverageConfig};
//!
//! // Create a coverage tracker
//! let tracker = CoverageTracker::new();
//!
//! // Run tests with tracking
//! let client = TestClient::new(app).with_coverage(&tracker);
//! client.get("/users").send();
//! client.post("/users").json(&user).send();
//!
//! // Generate report
//! let report = tracker.report();
//! report.assert_threshold(0.80); // Fail if < 80% coverage
//! report.write_json("coverage.json")?;
//! report.write_html("coverage.html")?;
//! ```
//!
//! # CI Integration
//!
//! Use with `cargo-llvm-cov` for full line-level coverage:
//!
//! ```bash
//! # Install coverage tools
//! cargo install cargo-llvm-cov
//!
//! # Run with coverage
//! cargo llvm-cov --html --open
//!
//! # CI: Check threshold
//! cargo llvm-cov --fail-under-lines 80
//! ```

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io;
use std::sync::Arc;
use parking_lot::Mutex;

use crate::request::Method;

/// Configuration for coverage tracking.
#[derive(Debug, Clone)]
pub struct CoverageConfig {
    /// Minimum line coverage percentage (0.0 - 1.0).
    pub line_threshold: f64,
    /// Minimum branch coverage percentage (0.0 - 1.0).
    pub branch_threshold: f64,
    /// Minimum endpoint coverage percentage (0.0 - 1.0).
    pub endpoint_threshold: f64,
    /// Whether to fail tests below threshold.
    pub fail_on_threshold: bool,
    /// Output formats to generate.
    pub output_formats: Vec<OutputFormat>,
    /// Directory for coverage reports.
    pub output_dir: String,
}

impl Default for CoverageConfig {
    fn default() -> Self {
        Self {
            line_threshold: 0.80,
            branch_threshold: 0.70,
            endpoint_threshold: 0.90,
            fail_on_threshold: true,
            output_formats: vec![OutputFormat::Json, OutputFormat::Html],
            output_dir: "target/coverage".into(),
        }
    }
}

impl CoverageConfig {
    /// Create a new configuration with default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the line coverage threshold.
    #[must_use]
    pub fn line_threshold(mut self, threshold: f64) -> Self {
        self.line_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set the branch coverage threshold.
    #[must_use]
    pub fn branch_threshold(mut self, threshold: f64) -> Self {
        self.branch_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set the endpoint coverage threshold.
    #[must_use]
    pub fn endpoint_threshold(mut self, threshold: f64) -> Self {
        self.endpoint_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Disable failing on threshold violations.
    #[must_use]
    pub fn no_fail(mut self) -> Self {
        self.fail_on_threshold = false;
        self
    }

    /// Set output formats.
    #[must_use]
    pub fn output_formats(mut self, formats: Vec<OutputFormat>) -> Self {
        self.output_formats = formats;
        self
    }

    /// Set output directory.
    #[must_use]
    pub fn output_dir(mut self, dir: impl Into<String>) -> Self {
        self.output_dir = dir.into();
        self
    }
}

/// Output format for coverage reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// JSON format for CI integration.
    Json,
    /// HTML format for human review.
    Html,
    /// SVG badge for README.
    Badge,
    /// LCOV format for external tools.
    Lcov,
}

/// Tracks endpoint coverage during test execution.
///
/// Thread-safe and can be shared across multiple test clients.
#[derive(Debug, Clone)]
pub struct CoverageTracker {
    inner: Arc<Mutex<CoverageTrackerInner>>,
}

#[derive(Debug, Default)]
struct CoverageTrackerInner {
    /// Registered endpoints (method, path pattern).
    registered_endpoints: Vec<(Method, String)>,
    /// Hit counts per endpoint.
    endpoint_hits: HashMap<(Method, String), EndpointHits>,
    /// Branch coverage hints.
    branches: HashMap<String, BranchHits>,
}

/// Endpoint hit statistics.
#[derive(Debug, Clone, Default)]
pub struct EndpointHits {
    /// Total number of times this endpoint was called.
    pub total_calls: u64,
    /// Number of successful responses (2xx).
    pub success_count: u64,
    /// Number of client error responses (4xx).
    pub client_error_count: u64,
    /// Number of server error responses (5xx).
    pub server_error_count: u64,
    /// Status codes observed.
    pub status_codes: HashMap<u16, u64>,
}

/// Branch coverage for specific code paths.
#[derive(Debug, Clone, Default)]
pub struct BranchHits {
    /// Number of times the branch was taken.
    pub taken_count: u64,
    /// Number of times the branch was not taken.
    pub not_taken_count: u64,
}

impl CoverageTracker {
    /// Create a new coverage tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CoverageTrackerInner::default())),
        }
    }

    /// Register an endpoint for coverage tracking.
    pub fn register_endpoint(&self, method: Method, path: impl Into<String>) {
        let mut inner = self.inner.lock();
        inner.registered_endpoints.push((method, path.into()));
    }

    /// Register multiple endpoints from a route table.
    pub fn register_endpoints<'a>(&self, endpoints: impl IntoIterator<Item = (Method, &'a str)>) {
        let mut inner = self.inner.lock();
        for (method, path) in endpoints {
            inner.registered_endpoints.push((method, path.to_string()));
        }
    }

    /// Record a hit on an endpoint.
    pub fn record_hit(&self, method: Method, path: &str, status_code: u16) {
        let mut inner = self.inner.lock();

        let key = (method, path.to_string());
        let hits = inner.endpoint_hits.entry(key).or_default();

        hits.total_calls += 1;
        *hits.status_codes.entry(status_code).or_insert(0) += 1;

        match status_code {
            200..=299 => hits.success_count += 1,
            400..=499 => hits.client_error_count += 1,
            500..=599 => hits.server_error_count += 1,
            _ => {}
        }
    }

    /// Record a branch hit.
    pub fn record_branch(&self, branch_id: impl Into<String>, taken: bool) {
        let mut inner = self.inner.lock();

        let branch = inner.branches.entry(branch_id.into()).or_default();
        if taken {
            branch.taken_count += 1;
        } else {
            branch.not_taken_count += 1;
        }
    }

    /// Generate a coverage report.
    #[must_use]
    pub fn report(&self) -> CoverageReport {
        let inner = self.inner.lock();

        let mut endpoints = BTreeMap::new();
        for (method, path) in &inner.registered_endpoints {
            let key = (*method, path.clone());
            let hits = inner.endpoint_hits.get(&key).cloned().unwrap_or_default();
            endpoints.insert((method.as_str().to_string(), path.clone()), hits);
        }

        // Find endpoints that were hit but not registered
        for ((method, path), hits) in &inner.endpoint_hits {
            let key = (method.as_str().to_string(), path.clone());
            endpoints.entry(key).or_insert_with(|| hits.clone());
        }

        let branches = inner.branches.clone();

        CoverageReport {
            endpoints,
            branches,
        }
    }

    /// Reset all coverage data.
    pub fn reset(&self) {
        let mut inner = self.inner.lock();
        inner.endpoint_hits.clear();
        inner.branches.clear();
    }
}

impl Default for CoverageTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Coverage report with statistics and utilities.
#[derive(Debug, Clone)]
pub struct CoverageReport {
    /// Endpoint coverage: (method, path) -> hits.
    pub endpoints: BTreeMap<(String, String), EndpointHits>,
    /// Branch coverage by identifier.
    pub branches: HashMap<String, BranchHits>,
}

impl CoverageReport {
    /// Calculate endpoint coverage percentage.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn endpoint_coverage(&self) -> f64 {
        if self.endpoints.is_empty() {
            return 1.0;
        }

        let covered = self
            .endpoints
            .values()
            .filter(|h| h.total_calls > 0)
            .count();

        covered as f64 / self.endpoints.len() as f64
    }

    /// Calculate branch coverage percentage.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn branch_coverage(&self) -> f64 {
        if self.branches.is_empty() {
            return 1.0;
        }

        let fully_covered = self
            .branches
            .values()
            .filter(|b| b.taken_count > 0 && b.not_taken_count > 0)
            .count();

        fully_covered as f64 / self.branches.len() as f64
    }

    /// Get endpoints that have not been tested.
    #[must_use]
    pub fn untested_endpoints(&self) -> Vec<(&str, &str)> {
        self.endpoints
            .iter()
            .filter(|(_, hits)| hits.total_calls == 0)
            .map(|((method, path), _)| (method.as_str(), path.as_str()))
            .collect()
    }

    /// Get endpoints with only success responses (no error testing).
    #[must_use]
    pub fn untested_error_paths(&self) -> Vec<(&str, &str)> {
        self.endpoints
            .iter()
            .filter(|(_, hits)| {
                hits.total_calls > 0 && hits.client_error_count == 0 && hits.server_error_count == 0
            })
            .map(|((method, path), _)| (method.as_str(), path.as_str()))
            .collect()
    }

    /// Assert that endpoint coverage meets threshold.
    ///
    /// # Panics
    ///
    /// Panics if coverage is below threshold with a detailed message.
    pub fn assert_threshold(&self, threshold: f64) {
        let coverage = self.endpoint_coverage();
        if coverage < threshold {
            let untested = self.untested_endpoints();
            panic!(
                "Endpoint coverage {:.1}% is below threshold {:.1}%.\n\
                 Untested endpoints ({}):\n{}",
                coverage * 100.0,
                threshold * 100.0,
                untested.len(),
                untested
                    .iter()
                    .map(|(m, p)| format!("  - {} {}", m, p))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }

    /// Write coverage report as JSON.
    ///
    /// # Errors
    ///
    /// Returns error if file cannot be written.
    pub fn write_json(&self, path: &str) -> io::Result<()> {
        let json = self.to_json();
        std::fs::write(path, json)
    }

    /// Generate JSON representation.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut json = String::from("{\n");
        json.push_str("  \"summary\": {\n");
        json.push_str(&format!(
            "    \"endpoint_coverage\": {:.4},\n",
            self.endpoint_coverage()
        ));
        json.push_str(&format!(
            "    \"branch_coverage\": {:.4},\n",
            self.branch_coverage()
        ));
        json.push_str(&format!(
            "    \"total_endpoints\": {},\n",
            self.endpoints.len()
        ));
        json.push_str(&format!(
            "    \"tested_endpoints\": {}\n",
            self.endpoints
                .values()
                .filter(|h| h.total_calls > 0)
                .count()
        ));
        json.push_str("  },\n");

        json.push_str("  \"endpoints\": [\n");
        let endpoint_entries: Vec<_> = self
            .endpoints
            .iter()
            .map(|((method, path), hits)| {
                format!(
                    "    {{\n\
                     \"method\": \"{method}\",\n\
                     \"path\": \"{path}\",\n\
                     \"calls\": {},\n\
                     \"success\": {},\n\
                     \"client_errors\": {},\n\
                     \"server_errors\": {}\n\
                     }}",
                    hits.total_calls,
                    hits.success_count,
                    hits.client_error_count,
                    hits.server_error_count
                )
                .replace('\n', "\n    ")
            })
            .collect();
        json.push_str(&endpoint_entries.join(",\n"));
        json.push_str("\n  ]\n");
        json.push('}');

        json
    }

    /// Write coverage report as HTML.
    ///
    /// # Errors
    ///
    /// Returns error if file cannot be written.
    pub fn write_html(&self, path: &str) -> io::Result<()> {
        let html = self.to_html();
        std::fs::write(path, html)
    }

    /// Generate HTML representation.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn to_html(&self) -> String {
        let coverage_pct = self.endpoint_coverage() * 100.0;
        let coverage_class = if coverage_pct >= 80.0 {
            "good"
        } else if coverage_pct >= 60.0 {
            "warning"
        } else {
            "poor"
        };

        let mut html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>fastapi_rust Coverage Report</title>
    <style>
        body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; margin: 20px; background: #f5f5f5; }}
        .container {{ max-width: 1200px; margin: 0 auto; background: white; padding: 20px; border-radius: 8px; box-shadow: 0 2px 4px rgba(0,0,0,0.1); }}
        h1 {{ color: #333; border-bottom: 2px solid #007bff; padding-bottom: 10px; }}
        .summary {{ display: flex; gap: 20px; margin-bottom: 30px; }}
        .metric {{ flex: 1; padding: 20px; border-radius: 8px; text-align: center; }}
        .metric.good {{ background: #d4edda; color: #155724; }}
        .metric.warning {{ background: #fff3cd; color: #856404; }}
        .metric.poor {{ background: #f8d7da; color: #721c24; }}
        .metric h2 {{ margin: 0 0 10px 0; font-size: 2.5em; }}
        .metric p {{ margin: 0; font-size: 0.9em; }}
        table {{ width: 100%; border-collapse: collapse; }}
        th, td {{ padding: 12px; text-align: left; border-bottom: 1px solid #ddd; }}
        th {{ background: #f8f9fa; font-weight: 600; }}
        tr:hover {{ background: #f5f5f5; }}
        .method {{ font-family: monospace; padding: 2px 6px; border-radius: 4px; font-weight: 600; }}
        .method.GET {{ background: #28a745; color: white; }}
        .method.POST {{ background: #ffc107; color: black; }}
        .method.PUT {{ background: #17a2b8; color: white; }}
        .method.DELETE {{ background: #dc3545; color: white; }}
        .method.PATCH {{ background: #6f42c1; color: white; }}
        .untested {{ color: #dc3545; font-weight: 600; }}
        .path {{ font-family: monospace; }}
        .count {{ text-align: right; }}
    </style>
</head>
<body>
    <div class="container">
        <h1>fastapi_rust Coverage Report</h1>

        <div class="summary">
            <div class="metric {coverage_class}">
                <h2>{coverage_pct:.1}%</h2>
                <p>Endpoint Coverage</p>
            </div>
            <div class="metric">
                <h2>{}</h2>
                <p>Total Endpoints</p>
            </div>
            <div class="metric">
                <h2>{}</h2>
                <p>Tested Endpoints</p>
            </div>
        </div>

        <h2>Endpoint Details</h2>
        <table>
            <thead>
                <tr>
                    <th>Method</th>
                    <th>Path</th>
                    <th class="count">Calls</th>
                    <th class="count">Success</th>
                    <th class="count">4xx</th>
                    <th class="count">5xx</th>
                </tr>
            </thead>
            <tbody>
"#,
            self.endpoints.len(),
            self.endpoints
                .values()
                .filter(|h| h.total_calls > 0)
                .count()
        );

        for ((method, path), hits) in &self.endpoints {
            let tested_class = if hits.total_calls == 0 {
                " class=\"untested\""
            } else {
                ""
            };
            let method_escaped = escape_html(method);
            let path_escaped = escape_html(path);
            html.push_str(&format!(
                r#"                <tr{tested_class}>
                    <td><span class="method {method_escaped}">{method_escaped}</span></td>
                    <td class="path">{path_escaped}</td>
                    <td class="count">{}</td>
                    <td class="count">{}</td>
                    <td class="count">{}</td>
                    <td class="count">{}</td>
                </tr>
"#,
                hits.total_calls,
                hits.success_count,
                hits.client_error_count,
                hits.server_error_count
            ));
        }

        html.push_str(
            r"            </tbody>
        </table>
    </div>
</body>
</html>",
        );

        html
    }

    /// Generate SVG badge.
    #[must_use]
    pub fn to_badge(&self) -> String {
        let coverage_pct = self.endpoint_coverage() * 100.0;
        let color = if coverage_pct >= 80.0 {
            "4c1"
        } else if coverage_pct >= 60.0 {
            "dfb317"
        } else {
            "e05d44"
        };

        // Build SVG programmatically to avoid raw string issues with hex colors
        let mut svg = String::new();
        svg.push_str(r#"<svg xmlns="http://www.w3.org/2000/svg" width="106" height="20">"#);
        svg.push_str("\n  <linearGradient id=\"b\" x2=\"0\" y2=\"100%\">");
        svg.push_str("\n    <stop offset=\"0\" stop-color=\"#bbb\" stop-opacity=\".1\"/>");
        svg.push_str("\n    <stop offset=\"1\" stop-opacity=\".1\"/>");
        svg.push_str("\n  </linearGradient>");
        svg.push_str(
            "\n  <mask id=\"a\"><rect width=\"106\" height=\"20\" rx=\"3\" fill=\"#fff\"/></mask>",
        );
        svg.push_str("\n  <g mask=\"url(#a)\">");
        svg.push_str("\n    <rect width=\"61\" height=\"20\" fill=\"#555\"/>");
        svg.push_str(&format!(
            "\n    <rect x=\"61\" width=\"45\" height=\"20\" fill=\"#{color}\"/>"
        ));
        svg.push_str("\n    <rect width=\"106\" height=\"20\" fill=\"url(#b)\"/>");
        svg.push_str("\n  </g>");
        svg.push_str("\n  <g fill=\"#fff\" text-anchor=\"middle\" font-family=\"DejaVu Sans,Verdana,Geneva,sans-serif\" font-size=\"11\">");
        svg.push_str(
            "\n    <text x=\"31.5\" y=\"15\" fill=\"#010101\" fill-opacity=\".3\">coverage</text>",
        );
        svg.push_str("\n    <text x=\"31.5\" y=\"14\" fill=\"#fff\">coverage</text>");
        svg.push_str(&format!("\n    <text x=\"82.5\" y=\"15\" fill=\"#010101\" fill-opacity=\".3\">{coverage_pct:.0}%</text>"));
        svg.push_str(&format!(
            "\n    <text x=\"82.5\" y=\"14\" fill=\"#fff\">{coverage_pct:.0}%</text>"
        ));
        svg.push_str("\n  </g>");
        svg.push_str("\n</svg>");

        svg
    }

    /// Write SVG badge to file.
    ///
    /// # Errors
    ///
    /// Returns error if file cannot be written.
    pub fn write_badge(&self, path: &str) -> io::Result<()> {
        std::fs::write(path, self.to_badge())
    }
}

impl fmt::Display for CoverageReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Coverage Report")?;
        writeln!(f, "===============")?;
        writeln!(f)?;
        writeln!(
            f,
            "Endpoint Coverage: {:.1}%",
            self.endpoint_coverage() * 100.0
        )?;
        writeln!(
            f,
            "Branch Coverage:   {:.1}%",
            self.branch_coverage() * 100.0
        )?;
        writeln!(f)?;

        let untested = self.untested_endpoints();
        if !untested.is_empty() {
            writeln!(f, "Untested Endpoints ({}):", untested.len())?;
            for (method, path) in untested {
                writeln!(f, "  - {} {}", method, path)?;
            }
        }

        let untested_errors = self.untested_error_paths();
        if !untested_errors.is_empty() {
            writeln!(f)?;
            writeln!(f, "Missing Error Path Tests ({}):", untested_errors.len())?;
            for (method, path) in untested_errors {
                writeln!(f, "  - {} {}", method, path)?;
            }
        }

        Ok(())
    }
}

/// Helper macro for recording branch coverage.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::record_branch;
///
/// let tracker = CoverageTracker::new();
///
/// if some_condition {
///     record_branch!(tracker, "auth_check", true);
///     // handle authenticated
/// } else {
///     record_branch!(tracker, "auth_check", false);
///     // handle unauthenticated
/// }
/// ```
#[macro_export]
macro_rules! record_branch {
    ($tracker:expr, $branch_id:expr, $taken:expr) => {
        $tracker.record_branch($branch_id, $taken)
    };
}

/// Escape HTML special characters to prevent XSS in generated reports.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_basic() {
        let tracker = CoverageTracker::new();

        // Register endpoints
        tracker.register_endpoint(Method::Get, "/users");
        tracker.register_endpoint(Method::Post, "/users");
        tracker.register_endpoint(Method::Get, "/users/{id}");

        // Record some hits
        tracker.record_hit(Method::Get, "/users", 200);
        tracker.record_hit(Method::Get, "/users", 200);
        tracker.record_hit(Method::Post, "/users", 201);
        tracker.record_hit(Method::Post, "/users", 400); // Error case

        let report = tracker.report();

        // 3 endpoints, 2 tested
        assert_eq!(report.endpoints.len(), 3);
        assert!((report.endpoint_coverage() - 2.0 / 3.0).abs() < 0.001);

        // Check untested
        let untested = report.untested_endpoints();
        assert_eq!(untested.len(), 1);
        assert_eq!(untested[0], ("GET", "/users/{id}"));
    }

    #[test]
    fn test_tracker_error_paths() {
        let tracker = CoverageTracker::new();

        tracker.register_endpoint(Method::Get, "/users");
        tracker.register_endpoint(Method::Post, "/users");

        // Only success for GET
        tracker.record_hit(Method::Get, "/users", 200);

        // Both success and error for POST
        tracker.record_hit(Method::Post, "/users", 201);
        tracker.record_hit(Method::Post, "/users", 400);

        let report = tracker.report();
        let untested_errors = report.untested_error_paths();

        assert_eq!(untested_errors.len(), 1);
        assert_eq!(untested_errors[0], ("GET", "/users"));
    }

    #[test]
    fn test_branch_coverage() {
        let tracker = CoverageTracker::new();

        // Fully covered branch
        tracker.record_branch("auth", true);
        tracker.record_branch("auth", false);

        // Partially covered branch (only true)
        tracker.record_branch("admin", true);

        let report = tracker.report();

        // 2 branches, 1 fully covered
        assert_eq!(report.branches.len(), 2);
        assert!((report.branch_coverage() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_report_json() {
        let tracker = CoverageTracker::new();
        tracker.register_endpoint(Method::Get, "/test");
        tracker.record_hit(Method::Get, "/test", 200);

        let report = tracker.report();
        let json = report.to_json();

        assert!(json.contains("\"endpoint_coverage\""));
        assert!(json.contains("\"/test\""));
    }

    #[test]
    fn test_report_html() {
        let tracker = CoverageTracker::new();
        tracker.register_endpoint(Method::Get, "/test");

        let report = tracker.report();
        let html = report.to_html();

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Coverage Report"));
        assert!(html.contains("/test"));
    }

    #[test]
    fn test_report_badge() {
        let tracker = CoverageTracker::new();
        tracker.register_endpoint(Method::Get, "/test");
        tracker.record_hit(Method::Get, "/test", 200);

        let report = tracker.report();
        let badge = report.to_badge();

        assert!(badge.contains("<svg"));
        assert!(badge.contains("coverage"));
        assert!(badge.contains("100%"));
    }

    #[test]
    fn test_config_builder() {
        let config = CoverageConfig::new()
            .line_threshold(0.90)
            .branch_threshold(0.85)
            .endpoint_threshold(0.95)
            .no_fail()
            .output_dir("custom/path");

        assert!((config.line_threshold - 0.90).abs() < 0.001);
        assert!((config.branch_threshold - 0.85).abs() < 0.001);
        assert!((config.endpoint_threshold - 0.95).abs() < 0.001);
        assert!(!config.fail_on_threshold);
        assert_eq!(config.output_dir, "custom/path");
    }

    #[test]
    fn test_threshold_clamp() {
        let config = CoverageConfig::new()
            .line_threshold(1.5) // Over 1.0
            .branch_threshold(-0.5); // Under 0.0

        assert!((config.line_threshold - 1.0).abs() < 0.001);
        assert!((config.branch_threshold - 0.0).abs() < 0.001);
    }

    #[test]
    #[should_panic(expected = "coverage")]
    fn test_assert_threshold_panics() {
        let tracker = CoverageTracker::new();
        tracker.register_endpoint(Method::Get, "/a");
        tracker.register_endpoint(Method::Get, "/b");
        // Only test one endpoint
        tracker.record_hit(Method::Get, "/a", 200);

        let report = tracker.report();
        report.assert_threshold(0.90); // Should panic, only 50% coverage
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_reset() {
        let tracker = CoverageTracker::new();
        tracker.register_endpoint(Method::Get, "/test");
        tracker.record_hit(Method::Get, "/test", 200);

        let report1 = tracker.report();
        assert_eq!(report1.endpoint_coverage(), 1.0);

        tracker.reset();

        let report2 = tracker.report();
        // Endpoint still registered but no hits
        assert_eq!(report2.endpoints.len(), 1);
        let hits = report2
            .endpoints
            .get(&("GET".to_string(), "/test".to_string()))
            .unwrap();
        assert_eq!(hits.total_calls, 0);
    }
}
