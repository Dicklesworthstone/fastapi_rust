//! Output mode selection and switching.
//!
//! This module defines the three output modes and provides logic
//! for selecting the appropriate mode based on environment detection.

use crate::detection::{OutputPreference, detected_preference};
use std::str::FromStr;

/// Output rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputMode {
    /// Full rich_rust styling: colors, boxes, tables, unicode.
    Rich,

    /// Plain text output with no ANSI codes or special characters.
    #[default]
    Plain,

    /// Colors only, no box characters or complex formatting.
    Minimal,
}

impl OutputMode {
    /// Select the appropriate mode based on environment detection.
    #[must_use]
    pub fn auto() -> Self {
        if let Ok(mode_str) = std::env::var("FASTAPI_OUTPUT_MODE") {
            if let Ok(mode) = mode_str.parse::<OutputMode>() {
                if matches!(mode, OutputMode::Rich) {
                    #[cfg(feature = "rich")]
                    {
                        return OutputMode::Rich;
                    }
                    #[cfg(not(feature = "rich"))]
                    {
                        return OutputMode::Plain;
                    }
                }
                return mode;
            }
        }

        match detected_preference() {
            OutputPreference::Plain => OutputMode::Plain,
            OutputPreference::Rich => {
                #[cfg(feature = "rich")]
                {
                    OutputMode::Rich
                }
                #[cfg(not(feature = "rich"))]
                {
                    OutputMode::Plain
                }
            }
        }
    }

    /// Check if this mode uses ANSI color codes.
    #[must_use]
    pub const fn uses_colors(&self) -> bool {
        matches!(self, Self::Rich | Self::Minimal)
    }

    /// Check if this mode uses box-drawing characters.
    #[must_use]
    pub const fn uses_boxes(&self) -> bool {
        matches!(self, Self::Rich)
    }

    /// Check if this mode supports tables.
    #[must_use]
    pub const fn supports_tables(&self) -> bool {
        matches!(self, Self::Rich)
    }

    /// Get the status indicator for success in this mode.
    #[must_use]
    pub const fn success_indicator(&self) -> &'static str {
        match self {
            Self::Rich => "✓",
            Self::Plain | Self::Minimal => "[OK]",
        }
    }

    /// Get the status indicator for errors in this mode.
    #[must_use]
    pub const fn error_indicator(&self) -> &'static str {
        match self {
            Self::Rich => "✗",
            Self::Plain | Self::Minimal => "[ERROR]",
        }
    }

    /// Get the status indicator for warnings in this mode.
    #[must_use]
    pub const fn warning_indicator(&self) -> &'static str {
        match self {
            Self::Rich => "⚠",
            Self::Plain | Self::Minimal => "[WARN]",
        }
    }

    /// Get the status indicator for info in this mode.
    #[must_use]
    pub const fn info_indicator(&self) -> &'static str {
        match self {
            Self::Rich => "ℹ",
            Self::Plain | Self::Minimal => "[INFO]",
        }
    }

    /// Check if this mode uses ANSI escape codes.
    #[must_use]
    pub const fn uses_ansi(&self) -> bool {
        self.uses_colors()
    }

    /// Check if this mode is minimal.
    #[must_use]
    pub const fn is_minimal(&self) -> bool {
        matches!(self, Self::Minimal)
    }
}

impl std::fmt::Display for OutputMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rich => write!(f, "rich"),
            Self::Plain => write!(f, "plain"),
            Self::Minimal => write!(f, "minimal"),
        }
    }
}

impl FromStr for OutputMode {
    type Err = OutputModeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "rich" => Ok(Self::Rich),
            "plain" => Ok(Self::Plain),
            "minimal" => Ok(Self::Minimal),
            _ => Err(OutputModeParseError(s.to_string())),
        }
    }
}

/// Error returned when parsing an invalid output mode string.
#[derive(Debug, Clone)]
pub struct OutputModeParseError(String);

impl std::fmt::Display for OutputModeParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid output mode '{}', expected: rich, plain, minimal",
            self.0
        )
    }
}

impl std::error::Error for OutputModeParseError {}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

    fn clean_env() {
        // SAFETY: Tests are run serially via #[serial] attribute.
        unsafe {
            env::remove_var("FASTAPI_OUTPUT_MODE");
            env::remove_var("FASTAPI_AGENT_MODE");
            env::remove_var("FASTAPI_HUMAN_MODE");
            env::remove_var("CLAUDE_CODE");
            env::remove_var("NO_COLOR");
            env::remove_var("CI");
        }
    }

    fn with_clean_env<F: FnOnce()>(f: F) {
        clean_env();
        f();
        clean_env();
    }

    fn set_env(key: &str, value: &str) {
        // SAFETY: Tests are run serially via #[serial] attribute.
        unsafe {
            env::set_var(key, value);
        }
    }

    // ========== ENUM BASIC TESTS ==========

    #[test]
    fn test_output_mode_default() {
        let mode = OutputMode::default();
        eprintln!("[TEST] Default OutputMode: {mode:?}");
        assert_eq!(mode, OutputMode::Plain);
    }

    #[test]
    fn test_output_mode_clone_copy() {
        let mode = OutputMode::Rich;
        let cloned = mode;
        let copied = mode;
        eprintln!(
            "[TEST] Clone/Copy test: original={mode:?}, cloned={cloned:?}, copied={copied:?}"
        );
        assert_eq!(mode, cloned);
        assert_eq!(mode, copied);
    }

    #[test]
    fn test_output_mode_equality() {
        assert_eq!(OutputMode::Rich, OutputMode::Rich);
        assert_eq!(OutputMode::Plain, OutputMode::Plain);
        assert_eq!(OutputMode::Minimal, OutputMode::Minimal);
        assert_ne!(OutputMode::Rich, OutputMode::Plain);
        assert_ne!(OutputMode::Plain, OutputMode::Minimal);
    }

    // ========== DISPLAY TESTS ==========

    #[test]
    fn test_display_rich() {
        let s = OutputMode::Rich.to_string();
        eprintln!("[TEST] Display Rich: {s}");
        assert_eq!(s, "rich");
    }

    #[test]
    fn test_display_plain() {
        let s = OutputMode::Plain.to_string();
        eprintln!("[TEST] Display Plain: {s}");
        assert_eq!(s, "plain");
    }

    #[test]
    fn test_display_minimal() {
        let s = OutputMode::Minimal.to_string();
        eprintln!("[TEST] Display Minimal: {s}");
        assert_eq!(s, "minimal");
    }

    // ========== FROMSTR TESTS ==========

    #[test]
    fn test_parse_rich() {
        let mode: OutputMode = "rich".parse().unwrap();
        eprintln!("[TEST] Parse rich: {mode:?}");
        assert_eq!(mode, OutputMode::Rich);
    }

    #[test]
    fn test_parse_plain() {
        let mode: OutputMode = "plain".parse().unwrap();
        eprintln!("[TEST] Parse plain: {mode:?}");
        assert_eq!(mode, OutputMode::Plain);
    }

    #[test]
    fn test_parse_minimal() {
        let mode: OutputMode = "minimal".parse().unwrap();
        eprintln!("[TEST] Parse minimal: {mode:?}");
        assert_eq!(mode, OutputMode::Minimal);
    }

    #[test]
    fn test_parse_case_insensitive() {
        assert_eq!("RICH".parse::<OutputMode>().unwrap(), OutputMode::Rich);
        assert_eq!("Plain".parse::<OutputMode>().unwrap(), OutputMode::Plain);
        assert_eq!(
            "MINIMAL".parse::<OutputMode>().unwrap(),
            OutputMode::Minimal
        );
        eprintln!("[TEST] Case insensitive parsing works");
    }

    #[test]
    fn test_parse_invalid() {
        let result = "invalid".parse::<OutputMode>();
        eprintln!("[TEST] Parse invalid: {result:?}");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid"));
    }

    // ========== CAPABILITY TESTS ==========

    #[test]
    fn test_uses_colors() {
        eprintln!(
            "[TEST] uses_colors: Rich={}, Plain={}, Minimal={}",
            OutputMode::Rich.uses_colors(),
            OutputMode::Plain.uses_colors(),
            OutputMode::Minimal.uses_colors()
        );
        assert!(OutputMode::Rich.uses_colors());
        assert!(!OutputMode::Plain.uses_colors());
        assert!(OutputMode::Minimal.uses_colors());
    }

    #[test]
    fn test_uses_boxes() {
        eprintln!(
            "[TEST] uses_boxes: Rich={}, Plain={}, Minimal={}",
            OutputMode::Rich.uses_boxes(),
            OutputMode::Plain.uses_boxes(),
            OutputMode::Minimal.uses_boxes()
        );
        assert!(OutputMode::Rich.uses_boxes());
        assert!(!OutputMode::Plain.uses_boxes());
        assert!(!OutputMode::Minimal.uses_boxes());
    }

    #[test]
    fn test_supports_tables() {
        eprintln!(
            "[TEST] supports_tables: Rich={}, Plain={}, Minimal={}",
            OutputMode::Rich.supports_tables(),
            OutputMode::Plain.supports_tables(),
            OutputMode::Minimal.supports_tables()
        );
        assert!(OutputMode::Rich.supports_tables());
        assert!(!OutputMode::Plain.supports_tables());
        assert!(!OutputMode::Minimal.supports_tables());
    }

    // ========== INDICATOR TESTS ==========

    #[test]
    fn test_success_indicators() {
        eprintln!(
            "[TEST] success_indicator: Rich={}, Plain={}, Minimal={}",
            OutputMode::Rich.success_indicator(),
            OutputMode::Plain.success_indicator(),
            OutputMode::Minimal.success_indicator()
        );
        assert_eq!(OutputMode::Rich.success_indicator(), "✓");
        assert_eq!(OutputMode::Plain.success_indicator(), "[OK]");
        assert_eq!(OutputMode::Minimal.success_indicator(), "[OK]");
    }

    #[test]
    fn test_error_indicators() {
        eprintln!(
            "[TEST] error_indicator: Rich={}, Plain={}, Minimal={}",
            OutputMode::Rich.error_indicator(),
            OutputMode::Plain.error_indicator(),
            OutputMode::Minimal.error_indicator()
        );
        assert_eq!(OutputMode::Rich.error_indicator(), "✗");
        assert_eq!(OutputMode::Plain.error_indicator(), "[ERROR]");
        assert_eq!(OutputMode::Minimal.error_indicator(), "[ERROR]");
    }

    #[test]
    fn test_warning_indicators() {
        eprintln!(
            "[TEST] warning_indicator: Rich={}, Plain={}, Minimal={}",
            OutputMode::Rich.warning_indicator(),
            OutputMode::Plain.warning_indicator(),
            OutputMode::Minimal.warning_indicator()
        );
        assert_eq!(OutputMode::Rich.warning_indicator(), "⚠");
        assert_eq!(OutputMode::Plain.warning_indicator(), "[WARN]");
        assert_eq!(OutputMode::Minimal.warning_indicator(), "[WARN]");
    }

    #[test]
    fn test_info_indicators() {
        eprintln!(
            "[TEST] info_indicator: Rich={}, Plain={}, Minimal={}",
            OutputMode::Rich.info_indicator(),
            OutputMode::Plain.info_indicator(),
            OutputMode::Minimal.info_indicator()
        );
        assert_eq!(OutputMode::Rich.info_indicator(), "ℹ");
        assert_eq!(OutputMode::Plain.info_indicator(), "[INFO]");
        assert_eq!(OutputMode::Minimal.info_indicator(), "[INFO]");
    }

    // ========== AUTO DETECTION TESTS ==========

    #[test]
    #[serial]
    fn test_auto_explicit_plain_override() {
        with_clean_env(|| {
            set_env("FASTAPI_OUTPUT_MODE", "plain");
            let mode = OutputMode::auto();
            eprintln!("[TEST] Explicit plain override: {mode:?}");
            assert_eq!(mode, OutputMode::Plain);
        });
    }

    #[test]
    #[serial]
    fn test_auto_explicit_minimal_override() {
        with_clean_env(|| {
            set_env("FASTAPI_OUTPUT_MODE", "minimal");
            let mode = OutputMode::auto();
            eprintln!("[TEST] Explicit minimal override: {mode:?}");
            assert_eq!(mode, OutputMode::Minimal);
        });
    }

    #[test]
    #[serial]
    fn test_auto_agent_detected() {
        with_clean_env(|| {
            set_env("CLAUDE_CODE", "1");
            let mode = OutputMode::auto();
            eprintln!("[TEST] Agent detected mode: {mode:?}");
            assert_eq!(mode, OutputMode::Plain);
        });
    }

    #[test]
    #[serial]
    fn test_auto_ci_detected() {
        with_clean_env(|| {
            set_env("CI", "true");
            let mode = OutputMode::auto();
            eprintln!("[TEST] CI detected mode: {mode:?}");
            assert_eq!(mode, OutputMode::Plain);
        });
    }

    #[test]
    #[serial]
    fn test_auto_no_color_detected() {
        with_clean_env(|| {
            set_env("NO_COLOR", "1");
            let mode = OutputMode::auto();
            eprintln!("[TEST] NO_COLOR detected mode: {mode:?}");
            assert_eq!(mode, OutputMode::Plain);
        });
    }

    #[test]
    #[serial]
    fn test_explicit_override_beats_detection() {
        with_clean_env(|| {
            set_env("CLAUDE_CODE", "1");
            set_env("FASTAPI_OUTPUT_MODE", "minimal");
            let mode = OutputMode::auto();
            eprintln!("[TEST] Override beats detection: {mode:?}");
            assert_eq!(mode, OutputMode::Minimal);
        });
    }

    #[test]
    #[serial]
    fn test_auto_deterministic() {
        with_clean_env(|| {
            set_env("CI", "true");
            let mode1 = OutputMode::auto();
            let mode2 = OutputMode::auto();
            let mode3 = OutputMode::auto();
            eprintln!("[TEST] Deterministic: {mode1:?} == {mode2:?} == {mode3:?}");
            assert_eq!(mode1, mode2);
            assert_eq!(mode2, mode3);
        });
    }

    // ========== PARSE ERROR TESTS ==========

    #[test]
    fn test_parse_error_display() {
        let err = OutputModeParseError("foobar".to_string());
        let msg = err.to_string();
        eprintln!("[TEST] Parse error display: {msg}");
        assert!(msg.contains("foobar"));
        assert!(msg.contains("rich"));
        assert!(msg.contains("plain"));
        assert!(msg.contains("minimal"));
    }

    #[test]
    fn test_parse_error_is_error() {
        let err = OutputModeParseError("x".to_string());
        let _: &dyn std::error::Error = &err;
        eprintln!("[TEST] OutputModeParseError implements Error trait");
    }
}
