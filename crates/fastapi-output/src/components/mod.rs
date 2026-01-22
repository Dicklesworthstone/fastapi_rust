//! Core output components for fastapi_rust.
//!
//! This module contains the primary visual components:
//! - [`banner`] - Startup banner with ASCII art and server info
//! - [`logging`] - Request/response logging with colors and timing
//! - [`errors`] - Error formatters for validation and HTTP errors
//! - [`routes`] - Route table display with method coloring
//! - [`middleware_stack`] - Middleware execution flow visualization
//! - [`dependency_tree`] - Dependency injection tree display
//! - [`test_results`] - Test results formatter
//! - [`shutdown_progress`] - Graceful shutdown progress indicator

pub mod banner;
pub mod dependency_tree;
pub mod errors;
pub mod logging;
pub mod middleware_stack;
pub mod routes;
pub mod shutdown_progress;
pub mod test_results;

// Re-export main types
pub use banner::{Banner, BannerConfig, ServerInfo};
pub use dependency_tree::{DependencyNode, DependencyTreeDisplay};
pub use errors::{ErrorFormatter, FormattedError};
pub use logging::{LogEntry, RequestLogger, ResponseTiming};
pub use middleware_stack::{MiddlewareInfo, MiddlewareStackDisplay};
pub use routes::{RouteDisplay, RouteTableConfig};
pub use shutdown_progress::{ShutdownPhase, ShutdownProgress};
pub use test_results::{
    TestCaseResult, TestModuleResult, TestReport, TestReportDisplay, TestStatus,
};
