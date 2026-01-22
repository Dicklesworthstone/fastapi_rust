//! Test results formatter component.
//!
//! Produces grouped, readable test output with summary statistics
//! and optional progress bar rendering.

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;
use std::fmt::Write;

const ANSI_RESET: &str = "\x1b[0m";

/// Test case status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    /// Test passed successfully.
    Pass,
    /// Test failed.
    Fail,
    /// Test was skipped.
    Skip,
}

impl TestStatus {
    /// Return the plain label for this status.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Skip => "SKIP",
        }
    }

    /// Return the display indicator for this status in a given mode.
    #[must_use]
    pub const fn indicator(self, mode: OutputMode) -> &'static str {
        match (self, mode) {
            (Self::Pass, OutputMode::Rich) => "✓",
            (Self::Fail, OutputMode::Rich) => "✗",
            (Self::Skip, OutputMode::Rich) => "↷",
            _ => self.label(),
        }
    }

    fn color(self, theme: &FastApiTheme) -> crate::themes::Color {
        match self {
            Self::Pass => theme.success,
            Self::Fail => theme.error,
            Self::Skip => theme.warning,
        }
    }
}

/// A single test case result.
#[derive(Debug, Clone)]
pub struct TestCaseResult {
    /// Test name.
    pub name: String,
    /// Test status.
    pub status: TestStatus,
    /// Duration in milliseconds.
    pub duration_ms: Option<u128>,
    /// Optional details (diff, error message, etc.).
    pub details: Option<String>,
}

impl TestCaseResult {
    /// Create a new test case result.
    #[must_use]
    pub fn new(name: impl Into<String>, status: TestStatus) -> Self {
        Self {
            name: name.into(),
            status,
            duration_ms: None,
            details: None,
        }
    }

    /// Set duration in milliseconds.
    #[must_use]
    pub fn duration_ms(mut self, duration_ms: u128) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    /// Add details for failed/skipped tests.
    #[must_use]
    pub fn details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

/// Group of test cases under a module or file.
#[derive(Debug, Clone)]
pub struct TestModuleResult {
    /// Module or file name.
    pub name: String,
    /// Test cases in this module.
    pub cases: Vec<TestCaseResult>,
}

impl TestModuleResult {
    /// Create a new module result.
    #[must_use]
    pub fn new(name: impl Into<String>, cases: Vec<TestCaseResult>) -> Self {
        Self {
            name: name.into(),
            cases,
        }
    }

    /// Add a test case to the module.
    #[must_use]
    pub fn case(mut self, case: TestCaseResult) -> Self {
        self.cases.push(case);
        self
    }
}

/// Aggregate test report.
#[derive(Debug, Clone)]
pub struct TestReport {
    /// All module results.
    pub modules: Vec<TestModuleResult>,
}

impl TestReport {
    /// Create a new report.
    #[must_use]
    pub fn new(modules: Vec<TestModuleResult>) -> Self {
        Self { modules }
    }

    /// Add a module to the report.
    #[must_use]
    pub fn module(mut self, module: TestModuleResult) -> Self {
        self.modules.push(module);
        self
    }

    /// Get summary counts for the report.
    #[must_use]
    pub fn counts(&self) -> TestCounts {
        let mut counts = TestCounts::default();
        for module in &self.modules {
            for case in &module.cases {
                counts.total += 1;
                match case.status {
                    TestStatus::Pass => counts.passed += 1,
                    TestStatus::Fail => counts.failed += 1,
                    TestStatus::Skip => counts.skipped += 1,
                }
                if let Some(duration) = case.duration_ms {
                    counts.duration_ms = Some(counts.duration_ms.unwrap_or(0) + duration);
                }
            }
        }
        counts
    }

    /// Render as TAP (Test Anything Protocol) output.
    #[must_use]
    pub fn to_tap(&self) -> String {
        let counts = self.counts();
        let mut lines = Vec::new();
        lines.push("TAP version 13".to_string());
        lines.push(format!("1..{}", counts.total));

        let mut index = 1;
        for module in &self.modules {
            for case in &module.cases {
                let status = match case.status {
                    TestStatus::Fail => "not ok",
                    TestStatus::Pass | TestStatus::Skip => "ok",
                };
                let mut line = format!("{status} {index} - {}::{}", module.name, case.name);
                if case.status == TestStatus::Skip {
                    line.push_str(" # SKIP");
                }
                lines.push(line);

                if let Some(details) = &case.details {
                    lines.push(format!("# {details}"));
                }

                index += 1;
            }
        }

        lines.join("\n")
    }
}

/// Summary counts for a test report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TestCounts {
    /// Total test cases.
    pub total: usize,
    /// Passed count.
    pub passed: usize,
    /// Failed count.
    pub failed: usize,
    /// Skipped count.
    pub skipped: usize,
    /// Total duration (ms) if available.
    pub duration_ms: Option<u128>,
}

/// Display configuration for test reports.
#[derive(Debug, Clone)]
pub struct TestReportDisplay {
    mode: OutputMode,
    theme: FastApiTheme,
    show_timings: bool,
    show_summary: bool,
    show_progress: bool,
    progress_width: usize,
    title: Option<String>,
}

impl TestReportDisplay {
    /// Create a new display for a given mode.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            show_timings: true,
            show_summary: true,
            show_progress: true,
            progress_width: 24,
            title: Some("Test Results".to_string()),
        }
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Hide per-test timings.
    #[must_use]
    pub fn hide_timings(mut self) -> Self {
        self.show_timings = false;
        self
    }

    /// Hide summary footer.
    #[must_use]
    pub fn hide_summary(mut self) -> Self {
        self.show_summary = false;
        self
    }

    /// Hide progress bar.
    #[must_use]
    pub fn hide_progress(mut self) -> Self {
        self.show_progress = false;
        self
    }

    /// Set progress bar width.
    #[must_use]
    pub fn progress_width(mut self, width: usize) -> Self {
        self.progress_width = width.max(8);
        self
    }

    /// Set a custom title (None to disable).
    #[must_use]
    pub fn title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    /// Render the report to a string.
    #[must_use]
    pub fn render(&self, report: &TestReport) -> String {
        let mut lines = Vec::new();

        if let Some(title) = &self.title {
            lines.push(title.clone());
            lines.push("-".repeat(title.len()));
        }

        for module in &report.modules {
            lines.push(self.render_module_header(&module.name));
            for case in &module.cases {
                lines.push(self.render_case_line(case));
                if case.status == TestStatus::Fail {
                    if let Some(details) = &case.details {
                        lines.push(format!("    -> {details}"));
                    }
                }
            }
            lines.push(String::new());
        }

        let counts = report.counts();
        if self.show_summary {
            lines.push(Self::render_summary(&counts));
        }
        if self.show_progress && counts.total > 0 {
            lines.push(self.render_progress(&counts));
        }

        lines.join("\n").trim_end().to_string()
    }

    fn render_module_header(&self, name: &str) -> String {
        if self.mode.uses_ansi() {
            let mut line = format!(
                "{}Module:{} {}{}",
                self.theme.accent.to_ansi_fg(),
                ANSI_RESET,
                self.theme.primary.to_ansi_fg(),
                name
            );
            line.push_str(ANSI_RESET);
            line
        } else {
            format!("Module: {name}")
        }
    }

    fn render_case_line(&self, case: &TestCaseResult) -> String {
        let indicator = case.status.indicator(self.mode);
        let indicator = if self.mode.uses_ansi() {
            format!(
                "{}{}{}",
                case.status.color(&self.theme).to_ansi_fg(),
                indicator,
                ANSI_RESET
            )
        } else {
            indicator.to_string()
        };

        let timing = if self.show_timings {
            match case.duration_ms {
                Some(ms) => format!(" ({ms}ms)"),
                None => String::new(),
            }
        } else {
            String::new()
        };

        format!("  {indicator} {}{timing}", case.name)
    }

    fn render_summary(counts: &TestCounts) -> String {
        let mut summary = format!(
            "Summary: {} passed, {} failed, {} skipped ({} total)",
            counts.passed, counts.failed, counts.skipped, counts.total
        );
        if let Some(duration) = counts.duration_ms {
            let _ = write!(summary, " in {duration}ms");
        }
        summary
    }

    fn render_progress(&self, counts: &TestCounts) -> String {
        let bar = progress_bar(
            counts.passed,
            counts.failed,
            counts.skipped,
            counts.total,
            self.progress_width,
            self.mode,
            &self.theme,
        );
        format!("Progress: {bar}")
    }
}

fn progress_bar(
    passed: usize,
    failed: usize,
    skipped: usize,
    total: usize,
    width: usize,
    mode: OutputMode,
    theme: &FastApiTheme,
) -> String {
    if total == 0 {
        return "[no tests]".to_string();
    }

    let width = width.max(8);
    let pass_len = passed.saturating_mul(width) / total;
    let fail_len = failed.saturating_mul(width) / total;
    let skip_len = skipped.saturating_mul(width) / total;
    let used = pass_len.saturating_add(fail_len).saturating_add(skip_len);
    let remaining = width.saturating_sub(used);

    let mut bar = String::new();
    bar.push('[');

    if mode.uses_ansi() {
        if pass_len > 0 {
            bar.push_str(&theme.success.to_ansi_fg());
            bar.push_str(&"=".repeat(pass_len));
            bar.push_str(ANSI_RESET);
        }
        if fail_len > 0 {
            bar.push_str(&theme.error.to_ansi_fg());
            bar.push_str(&"!".repeat(fail_len));
            bar.push_str(ANSI_RESET);
        }
        if skip_len > 0 {
            bar.push_str(&theme.warning.to_ansi_fg());
            bar.push_str(&"-".repeat(skip_len));
            bar.push_str(ANSI_RESET);
        }
        if remaining > 0 {
            bar.push_str(&theme.muted.to_ansi_fg());
            bar.push_str(&"-".repeat(remaining));
            bar.push_str(ANSI_RESET);
        }
    } else {
        bar.push_str(&"=".repeat(pass_len));
        bar.push_str(&"!".repeat(fail_len));
        bar.push_str(&"-".repeat(skip_len + remaining));
    }

    bar.push(']');
    let _ = write!(bar, " {passed}/{total} passed");

    if failed > 0 {
        let _ = write!(bar, ", {failed} failed");
    }
    if skipped > 0 {
        let _ = write!(bar, ", {skipped} skipped");
    }

    bar
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_contains, assert_no_ansi};

    #[test]
    fn renders_plain_report() {
        let module = TestModuleResult::new(
            "core::routing",
            vec![
                TestCaseResult::new("test_match", TestStatus::Pass).duration_ms(12),
                TestCaseResult::new("test_conflict", TestStatus::Fail)
                    .duration_ms(3)
                    .details("expected 2 routes, got 3"),
            ],
        );
        let report = TestReport::new(vec![module]);
        let display = TestReportDisplay::new(OutputMode::Plain);
        let output = display.render(&report);

        assert_contains(&output, "Test Results");
        assert_contains(&output, "Module: core::routing");
        assert_contains(&output, "PASS test_match");
        assert_contains(&output, "FAIL test_conflict");
        assert_contains(&output, "expected 2 routes");
        assert_contains(&output, "Summary:");
        assert_contains(&output, "Progress:");
        assert_no_ansi(&output);
    }

    #[test]
    fn renders_tap_output() {
        let report = TestReport::new(vec![TestModuleResult::new(
            "module",
            vec![
                TestCaseResult::new("ok_case", TestStatus::Pass),
                TestCaseResult::new("skip_case", TestStatus::Skip),
            ],
        )]);

        let tap = report.to_tap();
        assert_contains(&tap, "TAP version 13");
        assert_contains(&tap, "1..2");
        assert_contains(&tap, "ok 1 - module::ok_case");
        assert_contains(&tap, "ok 2 - module::skip_case # SKIP");
    }
}
