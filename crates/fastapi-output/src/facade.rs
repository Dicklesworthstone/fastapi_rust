//! Rich output facade.
//!
//! Provides a unified interface for console output that automatically
//! adapts to the current environment.

use crate::mode::OutputMode;
use crate::testing::{OutputEntry, OutputLevel, TestOutput};
use crate::themes::FastApiTheme;
use std::cell::RefCell;
use std::sync::{LazyLock, RwLock};
use std::time::Instant;

const ANSI_RESET: &str = "\x1b[0m";

/// Global instance of `RichOutput` for convenient access.
static GLOBAL_OUTPUT: LazyLock<RwLock<RichOutput>> =
    LazyLock::new(|| RwLock::new(RichOutput::auto()));

thread_local! {
    static TEST_OUTPUT: RefCell<Option<TestOutput>> = const { RefCell::new(None) };
}

/// Get the global `RichOutput` instance.
///
/// # Panics
///
/// Panics if the global lock is poisoned.
pub fn get_global() -> std::sync::RwLockReadGuard<'static, RichOutput> {
    GLOBAL_OUTPUT.read().expect("global output lock poisoned")
}

/// Replace the global `RichOutput` instance.
///
/// # Panics
///
/// Panics if the global lock is poisoned.
pub fn set_global(output: RichOutput) {
    *GLOBAL_OUTPUT.write().expect("global output lock poisoned") = output;
}

/// The main facade for rich console output.
///
/// Provides methods for printing styled text, tables, panels,
/// and other visual elements. Automatically adapts output based
/// on the detected environment.
#[derive(Debug, Clone)]
pub struct RichOutput {
    mode: OutputMode,
    theme: FastApiTheme,
}

impl RichOutput {
    /// Create a new `RichOutput` with the specified mode and default theme.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
        }
    }

    /// Create a new `RichOutput` with the specified mode.
    #[must_use]
    pub fn with_mode(mode: OutputMode) -> Self {
        Self::new(mode)
    }

    /// Create a new `RichOutput` with auto-detected mode.
    #[must_use]
    pub fn auto() -> Self {
        Self::new(OutputMode::auto())
    }

    /// Create a new `RichOutput` with rich mode (for humans).
    #[must_use]
    pub fn rich() -> Self {
        Self::new(OutputMode::Rich)
    }

    /// Create a new `RichOutput` with plain mode (for agents).
    #[must_use]
    pub fn plain() -> Self {
        Self::new(OutputMode::Plain)
    }

    /// Create a builder for custom configuration.
    #[must_use]
    pub fn builder() -> RichOutputBuilder {
        RichOutputBuilder::new()
    }

    /// Get the current output mode.
    #[must_use]
    pub const fn mode(&self) -> OutputMode {
        self.mode
    }

    /// Set the output mode.
    pub fn set_mode(&mut self, mode: OutputMode) {
        self.mode = mode;
    }

    /// Get the current theme.
    #[must_use]
    pub const fn theme(&self) -> &FastApiTheme {
        &self.theme
    }

    /// Set the theme.
    pub fn set_theme(&mut self, theme: FastApiTheme) {
        self.theme = theme;
    }

    /// Print a success message.
    ///
    /// In rich mode: Green checkmark with styled text.
    /// In plain mode: `[OK] message`
    pub fn success(&self, message: &str) {
        self.status(StatusKind::Success, message);
    }

    /// Print an error message.
    ///
    /// In rich mode: Red X with styled text.
    /// In plain mode: `[ERROR] message`
    pub fn error(&self, message: &str) {
        self.status(StatusKind::Error, message);
    }

    /// Print a warning message.
    ///
    /// In rich mode: Yellow warning symbol with styled text.
    /// In plain mode: `[WARN] message`
    pub fn warning(&self, message: &str) {
        self.status(StatusKind::Warning, message);
    }

    /// Print an info message.
    ///
    /// In rich mode: Blue info symbol with styled text.
    /// In plain mode: `[INFO] message`
    pub fn info(&self, message: &str) {
        self.status(StatusKind::Info, message);
    }

    /// Print a debug message (only in non-minimal modes).
    ///
    /// In rich mode: Gray text.
    /// In plain mode: `[DEBUG] message`
    /// In minimal mode: Nothing printed.
    pub fn debug(&self, message: &str) {
        self.status(StatusKind::Debug, message);
    }

    /// Print a status message with the given kind.
    pub fn status(&self, kind: StatusKind, message: &str) {
        if self.mode == OutputMode::Minimal && kind == StatusKind::Debug {
            return;
        }

        let (level, plain, raw, use_stderr) = self.format_status(kind, message);
        Self::write_line(level, &plain, &raw, use_stderr);
    }

    fn format_status(
        &self,
        kind: StatusKind,
        message: &str,
    ) -> (OutputLevel, String, String, bool) {
        let plain = format!("{} {}", kind.plain_prefix(), message);
        let level = kind.level();
        let use_stderr = kind.use_stderr();

        match self.mode {
            OutputMode::Plain => (level, plain.clone(), plain, use_stderr),
            OutputMode::Minimal => {
                let color = kind.color(&self.theme).to_ansi_fg();
                let raw = format!("{color}{}{} {message}", kind.plain_prefix(), ANSI_RESET);
                (level, plain, raw, use_stderr)
            }
            OutputMode::Rich => {
                let color = kind.color(&self.theme).to_ansi_fg();
                let icon = kind.rich_icon();
                let raw = format!("{color}{icon}{ANSI_RESET} {message}");
                (level, plain, raw, use_stderr)
            }
        }
    }

    /// Print a horizontal rule/divider.
    pub fn rule(&self, title: Option<&str>) {
        let plain = match title {
            Some(value) => format!("--- {value} ---"),
            None => "---".to_string(),
        };

        let raw = if self.mode.uses_ansi() {
            format!("{}{}{}", self.theme.border.to_ansi_fg(), plain, ANSI_RESET)
        } else {
            plain.clone()
        };

        Self::write_line(OutputLevel::Info, &plain, &raw, false);
    }

    /// Print content in a panel/box.
    pub fn panel(&self, content: &str, title: Option<&str>) {
        let plain = match title {
            Some(value) => format!("[{value}]\n{content}"),
            None => content.to_string(),
        };

        let raw = if self.mode.uses_ansi() {
            match title {
                Some(value) => format!(
                    "{}[{}]{}\n{content}",
                    self.theme.header.to_ansi_fg(),
                    value,
                    ANSI_RESET
                ),
                None => content.to_string(),
            }
        } else {
            plain.clone()
        };

        Self::write_line(OutputLevel::Info, &plain, &raw, false);
    }

    /// Print raw text.
    pub fn print(&self, text: &str) {
        Self::write_line(OutputLevel::Info, text, text, false);
    }

    /// Run a closure with test output capture enabled.
    pub fn with_test_output<F: FnOnce()>(test: &TestOutput, f: F) {
        TEST_OUTPUT.with(|cell| {
            *cell.borrow_mut() = Some(test.clone());
        });
        f();
        TEST_OUTPUT.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }

    fn write_line(level: OutputLevel, content: &str, raw: &str, use_stderr: bool) {
        let captured = TEST_OUTPUT.with(|cell| {
            if let Some(test_output) = cell.borrow().as_ref() {
                let entry = OutputEntry {
                    content: content.to_string(),
                    timestamp: Instant::now(),
                    level,
                    component: None,
                    raw_ansi: raw.to_string(),
                };
                test_output.push(entry);
                true
            } else {
                false
            }
        });

        if captured {
            return;
        }

        if use_stderr {
            eprintln!("{raw}");
        } else {
            println!("{raw}");
        }
    }

    /// Get the global `RichOutput` instance.
    ///
    /// # Panics
    ///
    /// Panics if the global lock is poisoned.
    pub fn global() -> std::sync::RwLockReadGuard<'static, RichOutput> {
        get_global()
    }

    /// Get mutable access to the global `RichOutput` instance.
    ///
    /// # Panics
    ///
    /// Panics if the global lock is poisoned.
    pub fn global_mut() -> std::sync::RwLockWriteGuard<'static, RichOutput> {
        GLOBAL_OUTPUT.write().expect("global output lock poisoned")
    }
}

impl Default for RichOutput {
    fn default() -> Self {
        Self::auto()
    }
}

/// Builder for RichOutput with custom configuration.
pub struct RichOutputBuilder {
    mode: Option<OutputMode>,
    theme: Option<FastApiTheme>,
}

impl RichOutputBuilder {
    /// Create a new builder with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            mode: None,
            theme: None,
        }
    }

    /// Set the output mode.
    #[must_use]
    pub fn mode(mut self, mode: OutputMode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Set the output theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Build the configured `RichOutput`.
    #[must_use]
    pub fn build(self) -> RichOutput {
        let mode = self.mode.unwrap_or_else(OutputMode::auto);
        let mut output = RichOutput::with_mode(mode);
        if let Some(theme) = self.theme {
            output.set_theme(theme);
        }
        output
    }
}

impl Default for RichOutputBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Status message kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    /// Success message.
    Success,
    /// Error message.
    Error,
    /// Warning message.
    Warning,
    /// Informational message.
    Info,
    /// Debug message.
    Debug,
    /// Pending status.
    Pending,
    /// In-progress status.
    InProgress,
}

impl StatusKind {
    /// Get the plain prefix used for this status kind.
    #[must_use]
    pub const fn plain_prefix(&self) -> &'static str {
        match self {
            Self::Success => "[OK]",
            Self::Error => "[ERROR]",
            Self::Warning => "[WARN]",
            Self::Info => "[INFO]",
            Self::Debug => "[DEBUG]",
            Self::Pending => "[PENDING]",
            Self::InProgress => "[...]",
        }
    }

    /// Get the icon used for rich mode output.
    #[must_use]
    pub const fn rich_icon(&self) -> &'static str {
        match self {
            Self::Success => "✓",
            Self::Error => "✗",
            Self::Warning => "⚠",
            Self::Info => "ℹ",
            Self::Debug => "●",
            Self::Pending => "○",
            Self::InProgress => "◐",
        }
    }

    /// Map to the output level for capture.
    #[must_use]
    pub const fn level(&self) -> OutputLevel {
        match self {
            Self::Success => OutputLevel::Success,
            Self::Error => OutputLevel::Error,
            Self::Warning => OutputLevel::Warning,
            Self::Info | Self::Pending | Self::InProgress => OutputLevel::Info,
            Self::Debug => OutputLevel::Debug,
        }
    }

    /// Whether this status should be printed to stderr.
    #[must_use]
    pub const fn use_stderr(&self) -> bool {
        matches!(self, Self::Error | Self::Warning)
    }

    fn color(self, theme: &FastApiTheme) -> crate::themes::Color {
        match self {
            Self::Success => theme.success,
            Self::Error => theme.error,
            Self::Warning => theme.warning,
            Self::Info => theme.info,
            Self::Debug | Self::Pending => theme.muted,
            Self::InProgress => theme.accent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_contains, assert_has_ansi, assert_no_ansi};
    use serial_test::serial;

    #[test]
    fn test_rich_output_new() {
        let output = RichOutput::new(OutputMode::Plain);
        assert_eq!(output.mode(), OutputMode::Plain);
    }

    #[test]
    fn test_rich_output_mode_setters() {
        let rich = RichOutput::rich();
        assert_eq!(rich.mode(), OutputMode::Rich);

        let plain = RichOutput::plain();
        assert_eq!(plain.mode(), OutputMode::Plain);
    }

    #[test]
    fn test_rich_output_set_mode() {
        let mut output = RichOutput::rich();
        output.set_mode(OutputMode::Plain);
        assert_eq!(output.mode(), OutputMode::Plain);
    }

    #[test]
    fn test_builder_with_mode() {
        let output = RichOutput::builder().mode(OutputMode::Minimal).build();
        assert_eq!(output.mode(), OutputMode::Minimal);
    }

    #[test]
    fn test_builder_with_theme() {
        let theme = FastApiTheme::neon();
        let output = RichOutput::builder()
            .mode(OutputMode::Plain)
            .theme(theme.clone())
            .build();
        assert_eq!(output.theme(), &theme);
    }

    #[test]
    fn test_status_plain_success() {
        let output = RichOutput::plain();
        let test_output = TestOutput::new(OutputMode::Plain);
        RichOutput::with_test_output(&test_output, || {
            output.success("Operation completed");
        });
        let captured = test_output.captured();
        assert_contains(&captured, "[OK]");
        assert_contains(&captured, "Operation completed");
        assert_no_ansi(&captured);
    }

    #[test]
    fn test_status_rich_has_ansi() {
        let output = RichOutput::rich();
        let test_output = TestOutput::new(OutputMode::Rich);
        RichOutput::with_test_output(&test_output, || {
            output.info("Server starting");
        });
        let raw = test_output.captured_raw();
        assert_contains(&raw, "Server starting");
        assert_has_ansi(&raw);
    }

    #[test]
    fn test_rule_and_panel_capture() {
        let output = RichOutput::plain();
        let test_output = TestOutput::new(OutputMode::Plain);
        RichOutput::with_test_output(&test_output, || {
            output.rule(Some("Configuration"));
            output.panel("Content", Some("Title"));
        });
        let captured = test_output.captured();
        assert_contains(&captured, "Configuration");
        assert_contains(&captured, "[Title]");
    }

    #[test]
    fn test_print_capture() {
        let output = RichOutput::plain();
        let test_output = TestOutput::new(OutputMode::Plain);
        RichOutput::with_test_output(&test_output, || {
            output.print("Raw text");
        });
        let captured = test_output.captured();
        assert_contains(&captured, "Raw text");
    }

    #[test]
    #[serial]
    fn test_get_set_global() {
        let original = RichOutput::global().clone();
        set_global(RichOutput::plain());
        assert_eq!(get_global().mode(), OutputMode::Plain);
        set_global(original);
    }
}
