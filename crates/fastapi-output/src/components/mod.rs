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
//! - [`http_inspector`] - Detailed HTTP request/response inspection (Phase 4)
//! - [`routing_debug`] - Routing decision debug output (Phase 4)
//! - [`openapi_display`] - OpenAPI schema visualization (Phase 5)
//! - [`help_display`] - Help and usage display (Phase 5)

pub mod banner;
pub mod dependency_tree;
pub mod errors;
pub mod help_display;
pub mod http_inspector;
pub mod logging;
pub mod middleware_stack;
pub mod openapi_display;
pub mod routes;
pub mod routing_debug;
pub mod shutdown_progress;
pub mod test_results;

// Re-export main types
pub use banner::{Banner, BannerConfig, ServerInfo};
pub use dependency_tree::{DependencyNode, DependencyTreeDisplay};
pub use errors::{ErrorFormatter, FormattedError, ValidationContext};
pub use help_display::{ArgGroup, ArgInfo, CommandInfo, HelpDisplay, HelpInfo};
pub use http_inspector::{RequestInfo, RequestInspector, ResponseInfo, ResponseInspector};
pub use logging::{LogEntry, RequestLogger, ResponseTiming};
pub use middleware_stack::{MiddlewareInfo, MiddlewareStackDisplay};
pub use openapi_display::{
    EndpointInfo, OpenApiDisplay, OpenApiDisplayConfig, OpenApiSummary, PropertyInfo, SchemaType,
};
pub use routes::{RouteDisplay, RouteTableConfig};
pub use routing_debug::{
    CandidateRoute, ExtractedParams, MatchResult, RoutingDebug, RoutingDebugInfo,
};
pub use shutdown_progress::{ShutdownPhase, ShutdownProgress};
pub use test_results::{
    TestCaseResult, TestModuleResult, TestReport, TestReportDisplay, TestStatus,
};
