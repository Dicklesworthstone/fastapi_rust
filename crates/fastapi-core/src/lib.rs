//! Core types and traits for fastapi_rust.
//!
//! This crate provides the fundamental building blocks:
//! - [`Request`] and [`Response`] types
//! - [`RequestContext`] wrapping asupersync's `Cx`
//! - [`FromRequest`] trait for extractors
//! - Error types and [`IntoResponse`] trait
//!
//! # Design Principles
//!
//! - Zero-copy where possible
//! - No runtime reflection
//! - All types support `Send + Sync`
//! - Cancel-correct via asupersync integration
//!
//! # Asupersync Integration
//!
//! This crate uses [asupersync](https://github.com/user/asupersync) as its async
//! runtime foundation, providing:
//!
//! - **Structured concurrency**: Request handlers run in regions
//! - **Cancel-correctness**: Graceful cancellation via checkpoints
//! - **Budgeted timeouts**: Request timeouts via budget exhaustion
//! - **Deterministic testing**: Lab runtime for reproducible tests

#![forbid(unsafe_code)]
// Pedantic clippy lints allowed (style suggestions, not correctness issues)
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::single_char_pattern)]
#![allow(clippy::unused_async)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::unreadable_literal)]
#![allow(clippy::type_complexity)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::missing_fields_in_debug)]
#![allow(clippy::single_match)]
#![allow(clippy::unused_self)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::semicolon_if_nothing_returned)]
#![allow(clippy::never_loop)]
#![allow(clippy::needless_lifetimes)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::manual_strip)]
#![allow(clippy::format_push_string)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::single_match_else)]
#![allow(clippy::elidable_lifetime_names)]
#![allow(clippy::map_unwrap_or)]

pub mod api_router;
pub mod app;
mod context;
mod dependency;
pub mod error;
mod extract;
pub mod logging;
pub mod middleware;
mod request;
mod response;
pub mod routing;
pub mod shutdown;
pub mod testing;

pub use context::{CancelledError, IntoOutcome, RequestContext};
pub use dependency::{
    CleanupFn, CleanupStack, DefaultConfig, DefaultDependencyConfig, DependencyCache,
    DependencyOverrides, DependencyScope, Depends, DependsCleanup, DependsConfig, FromDependency,
    FromDependencyWithCleanup, NoCache,
};
pub use error::{HttpError, LocItem, ValidationError, ValidationErrors};
pub use extract::{
    Accept, AppState, Authorization, BackgroundTasks, BackgroundTasksInner, BearerToken,
    BearerTokenError, ContentType, Cookie, CookiePrefix, CookiePrefixError, DEFAULT_JSON_LIMIT,
    FromHeaderValue, FromRequest, Header, HeaderExtractError, HeaderName, HeaderValues, Host, Json,
    JsonConfig, JsonExtractError, NamedHeader, OAuth2BearerError, OAuth2BearerErrorKind,
    OAuth2PasswordBearer, OAuth2PasswordBearerConfig, Path, PathExtractError, PathParams, Query,
    QueryExtractError, QueryParams, RequestRef, ResponseMut, ResponseMutations, SameSite, State,
    StateExtractError, UserAgent, XRequestId, snake_to_header_case,
};
pub use middleware::{
    AddResponseHeader, BoxFuture, ControlFlow, Cors, CorsConfig, Handler, Layer, Layered,
    Middleware, MiddlewareStack, NoopMiddleware, OriginPattern, PathPrefixFilter, ReferrerPolicy,
    RequestId, RequestIdConfig, RequestIdMiddleware, RequestResponseLogger, RequireHeader,
    SecurityHeaders, SecurityHeadersConfig, XFrameOptions,
};
pub use request::{Body, Headers, Method, Request};
pub use response::{
    BodyStream, FileResponse, Html, IntoResponse, NoContent, Redirect, Response, ResponseBody,
    StatusCode, Text, mime_type_for_extension,
};

// Re-export key asupersync types for convenience
pub use asupersync::{Budget, Cx, Outcome, RegionId, TaskId};

// Re-export testing utilities
pub use testing::{
    CapturedLog, CookieJar, E2ECapture, E2EReport, E2EScenario, E2EStep, E2EStepResult, LogCapture,
    RequestBuilder, ResponseDiff, TestClient, TestLogger, TestResponse, TestTimings, json_contains,
};
// Note: e2e_test!, assert_with_logs!, assert_eq_with_logs! macros are automatically exported
// at crate root via #[macro_export]

// Re-export assertion macros (defined via #[macro_export] in testing module)
// Note: The macros assert_status!, assert_header!, assert_body_contains!,
// assert_json!, and assert_body_matches! are automatically exported at the crate root
// due to #[macro_export]. Users can import them with `use fastapi_core::assert_status;`

// Re-export logging utilities
pub use logging::{AutoSpan, LogConfig, LogEntry, LogLevel, Span};

// Re-export api_router utilities
pub use api_router::{APIRouter, IncludeConfig, ResponseDef, RouterDependency, RouterRoute};

// Re-export app utilities
pub use app::{
    App, AppBuilder, AppConfig, ConfigError, ExceptionHandlers, RouteEntry, StartupHook,
    StartupHookError, StartupOutcome, StateContainer,
};

// Re-export shutdown utilities
pub use shutdown::{
    GracefulConfig, GracefulShutdown, InFlightGuard, ShutdownAware, ShutdownController,
    ShutdownHook, ShutdownOutcome, ShutdownPhase, ShutdownReceiver, grace_expired_cancel_reason,
    shutdown_cancel_reason, subdivide_grace_budget,
};

// Re-export routing utilities
pub use routing::{
    Converter, ParamInfo, PathSegment, RouteLookup, RoutePattern, RouteTable, format_allow_header,
};
