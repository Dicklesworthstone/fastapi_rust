//! Core types and traits for fastapi_rust.
//!
//! This crate provides the fundamental building blocks:
//! - [`Request`] and [`Response`] types
//! - [`RequestContext`] wrapping asupersync's [`Cx`](asupersync::Cx)
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

pub mod app;
mod context;
mod dependency;
pub mod error;
mod extract;
pub mod logging;
pub mod middleware;
mod request;
mod response;
pub mod shutdown;
pub mod testing;

pub use context::{CancelledError, IntoOutcome, RequestContext};
pub use dependency::{
    DefaultConfig, DefaultDependencyConfig, DependencyCache, DependencyOverrides, DependencyScope,
    Depends, DependsConfig, FromDependency, NoCache,
};
pub use error::{HttpError, LocItem, ValidationError, ValidationErrors};
pub use extract::{
    Accept, AppState, Authorization, ContentType, DEFAULT_JSON_LIMIT, FromHeaderValue, FromRequest,
    Header, HeaderExtractError, HeaderName, HeaderValues, Host, Json, JsonConfig, JsonExtractError,
    NamedHeader, OAuth2BearerError, OAuth2BearerErrorKind, OAuth2PasswordBearer,
    OAuth2PasswordBearerConfig, Path, PathExtractError, PathParams, Query, QueryExtractError,
    QueryParams, State, StateExtractError, UserAgent, XRequestId, snake_to_header_case,
};
pub use middleware::{
    AddResponseHeader, BoxFuture, ControlFlow, Cors, CorsConfig, Handler, Layer, Layered,
    Middleware, MiddlewareStack, NoopMiddleware, OriginPattern, PathPrefixFilter, RequestId,
    RequestIdConfig, RequestIdMiddleware, RequestResponseLogger, RequireHeader,
};
pub use request::{Body, Headers, Method, Request};
pub use response::{
    BodyStream, FileResponse, Html, IntoResponse, NoContent, Redirect, Response, ResponseBody,
    StatusCode, Text, mime_type_for_extension,
};

// Re-export key asupersync types for convenience
pub use asupersync::{Budget, Cx, Outcome, RegionId, TaskId};

// Re-export testing utilities
pub use testing::{CookieJar, RequestBuilder, TestClient, TestResponse, json_contains};

// Re-export assertion macros (defined via #[macro_export] in testing module)
// Note: The macros assert_status!, assert_header!, assert_body_contains!,
// assert_json!, and assert_body_matches! are automatically exported at the crate root
// due to #[macro_export]. Users can import them with `use fastapi_core::assert_status;`

// Re-export logging utilities
pub use logging::{AutoSpan, LogConfig, LogEntry, LogLevel, Span};

// Re-export app utilities
pub use app::{
    App, AppBuilder, AppConfig, ExceptionHandlers, RouteEntry, StartupHook, StartupHookError,
    StartupOutcome, StateContainer,
};

// Re-export shutdown utilities
pub use shutdown::{
    GracefulConfig, GracefulShutdown, InFlightGuard, ShutdownAware, ShutdownController,
    ShutdownHook, ShutdownOutcome, ShutdownPhase, ShutdownReceiver, grace_expired_cancel_reason,
    shutdown_cancel_reason, subdivide_grace_budget,
};
