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
//!
//! // Check if running in agent-friendly mode
//! if output.is_agent_mode() {
//!     // Output is plain text, safe for agent parsing
//! }
//! ```
//!
//! # Output Modes
//!
//! The library supports three output modes:
//!
//! | Mode | Description | Use Case |
//! |------|-------------|----------|
//! | `Rich` | Full ANSI styling with Unicode symbols | Human terminals |
//! | `Plain` | Zero ANSI codes, ASCII only | AI agents, CI, logs |
//! | `Minimal` | Colors only, no box characters | Simple terminals |
//!
//! # Agent Detection
//!
//! The library automatically detects agent environments by checking for
//! known environment variables set by AI coding assistants:
//!
//! | Variable | Agent |
//! |----------|-------|
//! | `CLAUDE_CODE` | Claude Code CLI |
//! | `CODEX_CLI` | OpenAI Codex CLI |
//! | `CURSOR_SESSION` | Cursor IDE |
//! | `AIDER_SESSION` | Aider |
//! | `AGENT_MODE` | Generic agent flag |
//! | `WINDSURF_SESSION` | Windsurf |
//! | `CLINE_SESSION` | Cline |
//! | `COPILOT_AGENT` | GitHub Copilot agent mode |
//!
//! # CI Detection
//!
//! CI environments are also detected and default to plain mode:
//!
//! | Variable | CI System |
//! |----------|-----------|
//! | `CI` | Generic CI flag |
//! | `GITHUB_ACTIONS` | GitHub Actions |
//! | `GITLAB_CI` | GitLab CI |
//! | `JENKINS_URL` | Jenkins |
//! | `CIRCLECI` | CircleCI |
//! | `TRAVIS` | Travis CI |
//! | `BUILDKITE` | Buildkite |
//!
//! # Environment Variable Precedence
//!
//! Environment variables are checked in the following order (highest to lowest priority):
//!
//! 1. **`FASTAPI_OUTPUT_MODE`** (highest) - Explicit mode selection
//!    - Values: `rich`, `plain`, `minimal`
//!    - Example: `FASTAPI_OUTPUT_MODE=plain cargo run`
//!
//! 2. **`FASTAPI_AGENT_MODE=1`** - Force agent detection (plain mode)
//!    - Useful for testing agent behavior in a human terminal
//!
//! 3. **`FASTAPI_HUMAN_MODE=1`** - Force human detection (rich mode)
//!    - Overrides agent detection when you want rich output
//!
//! 4. **`FORCE_COLOR`** - Standard force-color flag
//!    - Non-zero value forces rich output even in CI
//!
//! 5. **`NO_COLOR`** - Standard no-color flag
//!    - When set (any value), forces plain output
//!
//! 6. **Auto-detection** (lowest) - TTY check + agent/CI env vars
//!
//! ## Precedence Examples
//!
//! ```bash
//! # Explicit mode always wins
//! FASTAPI_OUTPUT_MODE=rich CLAUDE_CODE=1 cargo run  # → Rich mode
//!
//! # Agent override beats CI detection
//! FASTAPI_HUMAN_MODE=1 CI=true cargo run  # → Rich mode
//!
//! # FORCE_COLOR beats NO_COLOR and CI
//! FORCE_COLOR=1 CI=true NO_COLOR=1 cargo run  # → Rich mode
//!
//! # NO_COLOR beats auto-detection
//! NO_COLOR=1 cargo run  # → Plain mode (even in TTY)
//! ```
//!
//! # For Agent Authors
//!
//! If you're building an AI coding agent that invokes fastapi_rust applications:
//!
//! 1. **Set your agent's env var** (e.g., `CLAUDE_CODE=1`) for auto-detection
//! 2. **Or set `FASTAPI_OUTPUT_MODE=plain`** for explicit plain mode
//! 3. **Parse plain output** which uses consistent prefixes:
//!    - `[OK]` for success
//!    - `[ERROR]` for errors
//!    - `[WARN]` for warnings
//!    - `[INFO]` for info
//!    - `[DEBUG]` for debug
//!
//! # Components
//!
//! The library provides several output components:
//!
//! - [`Banner`] - Server startup banner with ASCII art
//! - [`RouteDisplay`] - Route table display
//! - [`ErrorFormatter`] - Validation and HTTP error formatting
//! - [`RequestLogger`] - HTTP request/response logging
//! - [`MiddlewareStackDisplay`] - Middleware stack visualization
//! - [`DependencyTreeDisplay`] - Dependency injection tree
//! - [`ShutdownProgressDisplay`] - Graceful shutdown progress
//! - [`TestReportDisplay`] - Test results formatting
//! - [`OpenApiDisplay`] - OpenAPI endpoint table and schema visualization (Phase 5)
//! - [`HelpDisplay`] - Help and usage display (Phase 5)

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
    ErrorFormatter, FormattedError, HttpErrorInfo, LocItem, ValidationContext,
    ValidationErrorDetail,
};
pub use components::help_display::{ArgGroup, ArgInfo, CommandInfo, HelpDisplay, HelpInfo};
pub use components::http_inspector::{RequestInfo, RequestInspector, ResponseInfo, ResponseInspector};
pub use components::logging::{HttpMethod, LogEntry, RequestLogger, ResponseTiming};
pub use components::middleware_stack::{MiddlewareInfo, MiddlewareStackDisplay};
pub use components::openapi_display::{
    EndpointInfo, OpenApiDisplay, OpenApiDisplayConfig, OpenApiSummary, PropertyInfo, SchemaType,
};
pub use components::routes::{RouteDisplay, RouteEntry, RouteTableConfig};
pub use components::routing_debug::{
    CandidateRoute, ExtractedParams, MatchResult, RoutingDebug, RoutingDebugInfo,
};
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
        ErrorFormatter, FormattedError, HttpErrorInfo, LocItem, ValidationContext,
        ValidationErrorDetail,
    };
    pub use crate::components::help_display::{
        ArgGroup, ArgInfo, CommandInfo, HelpDisplay, HelpInfo,
    };
    pub use crate::components::http_inspector::{
        RequestInfo, RequestInspector, ResponseInfo, ResponseInspector,
    };
    pub use crate::components::logging::{HttpMethod, LogEntry, RequestLogger, ResponseTiming};
    pub use crate::components::middleware_stack::{MiddlewareInfo, MiddlewareStackDisplay};
    pub use crate::components::openapi_display::{
        EndpointInfo, OpenApiDisplay, OpenApiDisplayConfig, OpenApiSummary, PropertyInfo,
        SchemaType,
    };
    pub use crate::components::routes::{RouteDisplay, RouteEntry, RouteTableConfig};
    pub use crate::components::routing_debug::{
        CandidateRoute, ExtractedParams, MatchResult, RoutingDebug, RoutingDebugInfo,
    };
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
