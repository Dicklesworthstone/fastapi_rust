//! Debug logging infrastructure for rich output.
//!
//! This module provides verbose debug logging that can be enabled via
//! `FASTAPI_OUTPUT_DEBUG=1` to help diagnose mode detection and rendering issues.
//!
//! # Overview
//!
//! When things go wrong (wrong mode detected, unexpected output, etc.),
//! debug logging helps by showing:
//! - What mode was detected and why
//! - Which environment variables were checked
//! - The rendering path taken
//! - Whether output was captured correctly in tests
//!
//! # Usage
//!
//! Enable debug logging in one of two ways:
//!
//! 1. Environment variable: `FASTAPI_OUTPUT_DEBUG=1`
//! 2. Programmatically: `debug::enable_debug()`
//!
//! Then use the debug macros in your code:
//!
//! ```rust,ignore
//! use fastapi_output::{debug_log, debug_detection, debug_mode, debug_render, debug_test};
//!
//! debug_log!("General debug message");
//! debug_detection!("Checking for Claude Code environment");
//! debug_mode!("Selected mode: {:?}", mode);
//! debug_render!("Rendering banner with {} lines", line_count);
//! debug_test!("Test assertion: expected {}", expected);
//! ```

use std::env;
use std::sync::atomic::{AtomicBool, Ordering};

/// Global flag for debug logging.
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Global flag to track if init() has been called.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialize debug logging from environment.
///
/// This is automatically called on first use of any debug macro,
/// but can be called explicitly for eager initialization.
///
/// # Environment Variables
///
/// | Variable | Effect |
/// |----------|--------|
/// | `FASTAPI_OUTPUT_DEBUG=1` | Enable all debug logging |
/// | `FASTAPI_OUTPUT_DEBUG=true` | Enable all debug logging |
/// | Unset/other | Debug logging disabled |
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return; // Already initialized
    }

    let enabled = env::var("FASTAPI_OUTPUT_DEBUG")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false);
    DEBUG_ENABLED.store(enabled, Ordering::SeqCst);

    if enabled {
        eprintln!("[FASTAPI_OUTPUT] Debug logging enabled");
    }
}

/// Check if debug logging is enabled.
///
/// This automatically initializes from the environment on first call.
#[must_use]
pub fn is_debug_enabled() -> bool {
    // Ensure initialization has happened
    if !INITIALIZED.load(Ordering::SeqCst) {
        init();
    }
    DEBUG_ENABLED.load(Ordering::SeqCst)
}

/// Enable debug logging programmatically.
///
/// This is useful for tests that want to capture debug output.
///
/// # Example
///
/// ```rust,ignore
/// use fastapi_output::debug;
///
/// debug::enable_debug();
/// // ... run code that produces debug output ...
/// debug::disable_debug();
/// ```
pub fn enable_debug() {
    INITIALIZED.store(true, Ordering::SeqCst);
    DEBUG_ENABLED.store(true, Ordering::SeqCst);
}

/// Disable debug logging programmatically.
///
/// This is useful for tests that need to restore the default state.
pub fn disable_debug() {
    DEBUG_ENABLED.store(false, Ordering::SeqCst);
}

/// Reset the debug state for testing.
///
/// This clears both the enabled flag and the initialized flag,
/// allowing `init()` to re-read the environment variable.
#[doc(hidden)]
pub fn reset_for_test() {
    INITIALIZED.store(false, Ordering::SeqCst);
    DEBUG_ENABLED.store(false, Ordering::SeqCst);
}

/// Log a general debug message if debug logging is enabled.
///
/// # Example
///
/// ```rust,ignore
/// use fastapi_output::debug_log;
///
/// debug_log!("Processing request: {}", request_id);
/// ```
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("[FASTAPI_OUTPUT] {}", format!($($arg)*));
        }
    };
}

/// Log detection-related debug info.
///
/// Use this in detection.rs for environment variable checks and agent detection.
///
/// # Example
///
/// ```rust,ignore
/// use fastapi_output::debug_detection;
///
/// debug_detection!("Checking env var: {}", var_name);
/// debug_detection!("Found Claude Code indicator");
/// ```
#[macro_export]
macro_rules! debug_detection {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("[FASTAPI_OUTPUT:DETECT] {}", format!($($arg)*));
        }
    };
}

/// Log mode-related debug info.
///
/// Use this in mode.rs for mode selection and configuration.
///
/// # Example
///
/// ```rust,ignore
/// use fastapi_output::debug_mode;
///
/// debug_mode!("Mode override: {:?}", mode);
/// debug_mode!("Auto-detected mode: Rich");
/// ```
#[macro_export]
macro_rules! debug_mode {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("[FASTAPI_OUTPUT:MODE] {}", format!($($arg)*));
        }
    };
}

/// Log rendering-related debug info.
///
/// Use this in facade.rs and components for rendering operations.
///
/// # Example
///
/// ```rust,ignore
/// use fastapi_output::debug_render;
///
/// debug_render!("Rendering banner in {:?} mode", self.mode);
/// debug_render!("Output {} lines", lines.len());
/// ```
#[macro_export]
macro_rules! debug_render {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("[FASTAPI_OUTPUT:RENDER] {}", format!($($arg)*));
        }
    };
}

/// Log test-related debug info.
///
/// Use this in testing.rs and test modules for test execution.
///
/// # Example
///
/// ```rust,ignore
/// use fastapi_output::debug_test;
///
/// debug_test!("Captured {} output lines", output.len());
/// debug_test!("Assertion: expected {:?}", expected);
/// ```
#[macro_export]
macro_rules! debug_test {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("[FASTAPI_OUTPUT:TEST] {}", format!($($arg)*));
        }
    };
}

/// Log component-related debug info.
///
/// Use this in components/* for component-specific operations.
///
/// # Example
///
/// ```rust,ignore
/// use fastapi_output::debug_component;
///
/// debug_component!("Banner: rendering ASCII art");
/// debug_component!("RouteDisplay: {} routes", routes.len());
/// ```
#[macro_export]
macro_rules! debug_component {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!("[FASTAPI_OUTPUT:COMPONENT] {}", format!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests must run serially because they mutate global state.
    // Use `cargo test -- --test-threads=1` or the serial_test crate.

    fn setup() {
        reset_for_test();
    }

    #[test]
    fn test_debug_disabled_by_default() {
        setup();
        // Don't call init() - check raw state
        assert!(!DEBUG_ENABLED.load(Ordering::SeqCst));
    }

    #[test]
    fn test_enable_disable_debug() {
        setup();

        assert!(!is_debug_enabled());

        enable_debug();
        assert!(is_debug_enabled());

        disable_debug();
        assert!(!is_debug_enabled());
    }

    #[test]
    fn test_debug_log_macro_when_disabled() {
        setup();
        disable_debug();
        // Should not panic - just silently do nothing
        debug_log!("This should not appear");
    }

    #[test]
    fn test_debug_log_macro_when_enabled() {
        setup();
        enable_debug();
        // Should print to stderr (visually verify with --nocapture)
        debug_log!("Test message: {}", 42);
        disable_debug();
    }

    #[test]
    fn test_all_debug_macros() {
        setup();
        enable_debug();

        debug_log!("general log test");
        debug_detection!("detection test");
        debug_mode!("mode test");
        debug_render!("render test");
        debug_test!("test test");
        debug_component!("component test");

        disable_debug();
    }

    #[test]
    fn test_init_idempotent() {
        setup();

        // First init should set INITIALIZED
        init();
        assert!(INITIALIZED.load(Ordering::SeqCst));

        // Second init should be a no-op
        init();
        assert!(INITIALIZED.load(Ordering::SeqCst));
    }

    #[test]
    fn test_is_debug_enabled_auto_initializes() {
        setup();

        // INITIALIZED should be false
        assert!(!INITIALIZED.load(Ordering::SeqCst));

        // Calling is_debug_enabled should trigger init
        let _ = is_debug_enabled();

        // Now INITIALIZED should be true
        assert!(INITIALIZED.load(Ordering::SeqCst));
    }

    #[test]
    fn test_enable_debug_marks_initialized() {
        setup();

        assert!(!INITIALIZED.load(Ordering::SeqCst));

        enable_debug();

        // enable_debug should set INITIALIZED to prevent env var check
        assert!(INITIALIZED.load(Ordering::SeqCst));
        assert!(is_debug_enabled());
    }
}
