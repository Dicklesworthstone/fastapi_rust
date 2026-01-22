//! Agent-aware rich console output for fastapi_rust.
//!
//! This crate provides beautiful terminal output that automatically
//! detects whether it's running in an AI agent environment and switches
//! to plain text mode accordingly.
//!
//! # Features
//!
//! - Automatic agent detection (Claude Code, Codex, Cursor, etc.)
//! - Dual-mode output (Rich for humans, Plain for agents)
//! - FastAPI-themed color palette
//! - Tables, panels, progress bars, and more
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use fastapi_output::prelude::*;
//!
//! // Auto-detects mode based on environment
//! let output = RichOutput::auto();
//!
//! // Print styled text (rendered appropriately for mode)
//! output.success("Server started successfully");
//! output.error("Failed to bind to port 8000");
//! ```

// SAFETY: We use deny instead of forbid to allow unsafe in test modules.
// The only unsafe code is for env::set_var/remove_var in tests, which
// became unsafe in Rust 2024 edition due to thread-safety concerns.
// Our tests use serial_test to ensure sequential execution.
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod components;
pub mod detection;
pub mod facade;
pub mod mode;
pub mod testing;
pub mod themes;

// Re-exports for convenience
pub use detection::{
    DetectionResult, OutputPreference, OverrideMode, detect_environment, detected_preference,
    detection_diagnostics, is_agent_environment,
};
pub use facade::{RichOutput, RichOutputBuilder, StatusKind, get_global, set_global};
pub use mode::{OutputMode, feature_info, has_rich_support};
pub use testing::{
    OutputEntry, OutputLevel, TestOutput, assert_contains, assert_contains_in_order,
    assert_has_ansi, assert_max_width, assert_no_ansi, assert_not_contains, capture, capture_both,
    capture_with_width, debug_output, is_verbose, strip_ansi_codes,
};
pub use themes::{FastApiTheme, ThemePreset};

// Re-export component types
pub use components::banner::{Banner, BannerConfig, ServerInfo};
pub use components::dependency_tree::{DependencyNode, DependencyTreeDisplay};
pub use components::errors::{
    ErrorFormatter, FormattedError, HttpErrorInfo, LocItem, ValidationErrorDetail,
};
pub use components::logging::{HttpMethod, LogEntry, RequestLogger, ResponseTiming};
pub use components::middleware_stack::{MiddlewareInfo, MiddlewareStackDisplay};
pub use components::routes::{RouteDisplay, RouteEntry, RouteTableConfig};
pub use components::shutdown_progress::{ShutdownPhase, ShutdownProgress, ShutdownProgressDisplay};
pub use components::test_results::{
    TestCaseResult, TestModuleResult, TestReport, TestReportDisplay, TestStatus,
};

/// Prelude module for convenient imports.
pub mod prelude {
    // Components
    pub use crate::components::banner::{Banner, BannerConfig, ServerInfo};
    pub use crate::components::dependency_tree::{DependencyNode, DependencyTreeDisplay};
    pub use crate::components::errors::{
        ErrorFormatter, FormattedError, HttpErrorInfo, LocItem, ValidationErrorDetail,
    };
    pub use crate::components::logging::{HttpMethod, LogEntry, RequestLogger, ResponseTiming};
    pub use crate::components::middleware_stack::{MiddlewareInfo, MiddlewareStackDisplay};
    pub use crate::components::routes::{RouteDisplay, RouteEntry, RouteTableConfig};
    pub use crate::components::shutdown_progress::{
        ShutdownPhase, ShutdownProgress, ShutdownProgressDisplay,
    };
    pub use crate::components::test_results::{
        TestCaseResult, TestModuleResult, TestReport, TestReportDisplay, TestStatus,
    };

    // Core types
    pub use crate::detection::{
        DetectionResult, OutputPreference, OverrideMode, detect_environment, detected_preference,
        is_agent_environment,
    };
    pub use crate::facade::{RichOutput, RichOutputBuilder, StatusKind, get_global, set_global};
    pub use crate::mode::{OutputMode, feature_info, has_rich_support};
    pub use crate::testing::{
        OutputEntry, OutputLevel, TestOutput, assert_contains, assert_contains_in_order,
        assert_has_ansi, assert_max_width, assert_no_ansi, assert_not_contains, capture,
        capture_both, capture_with_width, debug_output, is_verbose, strip_ansi_codes,
    };
    pub use crate::themes::{FastApiTheme, ThemePreset};
}
