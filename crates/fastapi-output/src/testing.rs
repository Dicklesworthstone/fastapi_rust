//! Test utilities for output component testing.
//!
//! This module provides infrastructure for capturing and asserting on
//! output from RichOutput components in tests.

use crate::facade::RichOutput;
use crate::mode::OutputMode;
use regex::Regex;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

/// Test output buffer that captures all output for assertions.
#[derive(Debug, Clone)]
pub struct TestOutput {
    mode: OutputMode,
    buffer: Rc<RefCell<Vec<OutputEntry>>>,
    terminal_width: usize,
}

/// A single captured output entry with metadata.
#[derive(Debug, Clone)]
pub struct OutputEntry {
    /// Stripped/plain content for assertions.
    pub content: String,
    /// Capture time.
    pub timestamp: Instant,
    /// Output level.
    pub level: OutputLevel,
    /// Optional component identifier.
    pub component: Option<String>,
    /// Raw output including ANSI codes.
    pub raw_ansi: String,
}

/// Output classification for test assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputLevel {
    /// Debug output.
    Debug,
    /// Informational output.
    Info,
    /// Success output.
    Success,
    /// Warning output.
    Warning,
    /// Error output.
    Error,
}

impl TestOutput {
    /// Create a new test output buffer with specified mode.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            buffer: Rc::new(RefCell::new(Vec::new())),
            terminal_width: 80,
        }
    }

    /// Create with custom terminal width for width-dependent tests.
    #[must_use]
    pub fn with_width(mode: OutputMode, width: usize) -> Self {
        Self {
            mode,
            buffer: Rc::new(RefCell::new(Vec::new())),
            terminal_width: width,
        }
    }

    /// Get the current output mode.
    #[must_use]
    pub const fn mode(&self) -> OutputMode {
        self.mode
    }

    /// Get configured terminal width.
    #[must_use]
    pub const fn terminal_width(&self) -> usize {
        self.terminal_width
    }

    /// Add an entry to the buffer (called by RichOutput facade).
    pub fn push(&self, entry: OutputEntry) {
        self.buffer.borrow_mut().push(entry);
    }

    /// Get all captured output as a single string (stripped of ANSI).
    #[must_use]
    pub fn captured(&self) -> String {
        self.buffer
            .borrow()
            .iter()
            .map(|entry| entry.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Get all captured output with ANSI codes preserved.
    #[must_use]
    pub fn captured_raw(&self) -> String {
        self.buffer
            .borrow()
            .iter()
            .map(|entry| entry.raw_ansi.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Get captured entries for detailed inspection.
    #[must_use]
    pub fn entries(&self) -> Vec<OutputEntry> {
        self.buffer.borrow().clone()
    }

    /// Clear the buffer.
    pub fn clear(&self) {
        self.buffer.borrow_mut().clear();
    }

    /// Get count of entries by level.
    #[must_use]
    pub fn count_by_level(&self, level: OutputLevel) -> usize {
        self.buffer
            .borrow()
            .iter()
            .filter(|entry| entry.level == level)
            .count()
    }
}

/// Capture output from a closure in the specified mode.
///
/// # Example
/// ```rust
/// use fastapi_output::prelude::*;
/// use fastapi_output::testing::*;
///
/// let output = capture(OutputMode::Plain, || {
///     let out = RichOutput::plain();
///     out.success("Hello");
/// });
///
/// assert_contains(&output, "Hello");
/// ```
pub fn capture<F: FnOnce()>(mode: OutputMode, f: F) -> String {
    let test_output = TestOutput::new(mode);
    let original_mode = { RichOutput::global().mode() };
    {
        let mut global = RichOutput::global_mut();
        global.set_mode(mode);
    }
    RichOutput::with_test_output(&test_output, f);
    {
        let mut global = RichOutput::global_mut();
        global.set_mode(original_mode);
    }
    test_output.captured()
}

/// Capture with custom terminal width.
pub fn capture_with_width<F: FnOnce()>(mode: OutputMode, width: usize, f: F) -> String {
    let test_output = TestOutput::with_width(mode, width);
    let original_mode = { RichOutput::global().mode() };
    {
        let mut global = RichOutput::global_mut();
        global.set_mode(mode);
    }
    RichOutput::with_test_output(&test_output, f);
    {
        let mut global = RichOutput::global_mut();
        global.set_mode(original_mode);
    }
    test_output.captured()
}

/// Capture both plain and rich output for comparison.
pub fn capture_both<F: FnOnce() + Clone>(f: F) -> (String, String) {
    let plain = capture(OutputMode::Plain, f.clone());
    let rich = capture(OutputMode::Rich, f);
    (plain, rich)
}

// =============================================================================
// Assertion Utilities
// =============================================================================

/// Strip ANSI escape codes from a string.
#[must_use]
pub fn strip_ansi_codes(input: &str) -> String {
    let re = Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("invalid ANSI regex");
    re.replace_all(input, "").to_string()
}

/// Assert output contains text (after stripping ANSI codes).
#[track_caller]
pub fn assert_contains(output: &str, expected: &str) {
    let stripped = strip_ansi_codes(output);
    assert!(
        stripped.contains(expected),
        "Expected output to contain: '{expected}'\nActual output (stripped):\n{stripped}\n---"
    );
}

/// Assert output does NOT contain text.
#[track_caller]
pub fn assert_not_contains(output: &str, unexpected: &str) {
    let stripped = strip_ansi_codes(output);
    assert!(
        !stripped.contains(unexpected),
        "Expected output to NOT contain: '{unexpected}'\nActual output (stripped):\n{stripped}"
    );
}

/// Assert output has no ANSI codes (for plain mode testing).
#[track_caller]
pub fn assert_no_ansi(output: &str) {
    assert!(
        !output.contains("\x1b["),
        "Found ANSI escape codes in output that should be plain:\n{output}\n---"
    );
}

/// Assert output has ANSI codes (for rich mode testing).
#[track_caller]
pub fn assert_has_ansi(output: &str) {
    assert!(
        output.contains("\x1b["),
        "Expected ANSI escape codes in rich output but found none:\n{output}\n---"
    );
}

/// Assert all lines are within max width.
#[track_caller]
pub fn assert_max_width(output: &str, max_width: usize) {
    let stripped = strip_ansi_codes(output);
    for (idx, line) in stripped.lines().enumerate() {
        let width = UnicodeWidthStr::width(line);
        assert!(
            width <= max_width,
            "Line {} exceeds max width {}. Width: {}, Content: '{}'",
            idx + 1,
            max_width,
            width,
            line
        );
    }
}

/// Assert output contains all expected substrings in order.
#[track_caller]
pub fn assert_contains_in_order(output: &str, expected: &[&str]) {
    let stripped = strip_ansi_codes(output);
    let mut last_pos = 0;

    for (idx, exp) in expected.iter().enumerate() {
        match stripped[last_pos..].find(exp) {
            Some(pos) => {
                last_pos += pos + exp.len();
            }
            None => {
                panic!(
                    "Expected '{exp}' (item {idx}) not found after position {last_pos}\nOutput:\n{stripped}\n---"
                );
            }
        }
    }
}

// =============================================================================
// Debug Logging for Tests
// =============================================================================

/// Enable verbose test logging (set FASTAPI_TEST_VERBOSE=1).
#[must_use]
pub fn is_verbose() -> bool {
    std::env::var("FASTAPI_TEST_VERBOSE").is_ok()
}

/// Log message if verbose mode is enabled.
#[macro_export]
macro_rules! test_log {
    ($($arg:tt)*) => {
        if $crate::testing::is_verbose() {
            eprintln!("[TEST] {}", format!($($arg)*));
        }
    };
}

/// Log captured output for debugging.
pub fn debug_output(label: &str, output: &str) {
    if is_verbose() {
        eprintln!(
            "\n=== {} (raw) ===\n{}\n=== {} (stripped) ===\n{}\n=== END ===\n",
            label,
            output,
            label,
            strip_ansi_codes(output)
        );
    }
}

/// Test fixtures for common scenarios.
pub mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_captures_output() {
        let output = capture(OutputMode::Plain, || {
            RichOutput::global().success("Hello, World!");
        });

        assert_contains(&output, "Hello, World!");
    }

    #[test]
    fn test_strip_ansi_removes_codes() {
        let with_ansi = "\x1b[31mRed Text\x1b[0m";
        let stripped = strip_ansi_codes(with_ansi);
        assert_eq!(stripped, "Red Text");
    }

    #[test]
    fn test_assert_no_ansi_passes_for_plain() {
        let plain = "Just plain text";
        assert_no_ansi(plain);
    }

    #[test]
    #[should_panic(expected = "Found ANSI escape codes")]
    fn test_assert_no_ansi_fails_for_rich() {
        let with_ansi = "\x1b[31mColored\x1b[0m";
        assert_no_ansi(with_ansi);
    }

    #[test]
    fn test_capture_both_modes() {
        let (plain, rich) = capture_both(|| {
            RichOutput::global().success("Success!");
        });

        assert_no_ansi(&plain);
        assert_contains(&plain, "Success");
        assert_contains(&rich, "Success");
    }

    #[test]
    fn test_assert_contains_in_order() {
        let output = "First line\nSecond line\nThird line";
        assert_contains_in_order(output, &["First", "Second", "Third"]);
    }

    #[test]
    fn test_max_width_assertion() {
        let output = "Short\nAlso short";
        assert_max_width(output, 20);
    }
}
