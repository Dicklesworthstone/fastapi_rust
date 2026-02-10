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

pub mod app;
mod context;
mod dependency;
pub mod digest;
pub mod docs;
pub mod error;
mod extract;
pub mod logging;
pub mod middleware;
pub mod multipart;
mod password;
mod request;
mod response;
pub mod routing;
pub mod shutdown;
pub mod testing;
pub mod validation;
pub mod websocket;

pub use context::{CancelledError, IntoOutcome, RequestContext};
pub use dependency::{
    DefaultConfig, DefaultDependencyConfig, DependencyCache, DependencyOverrides, DependencyScope,
    Depends, DependsCleanup, DependsConfig, FromDependency, FromDependencyWithCleanup, NoCache,
};
pub use digest::{DigestAlgorithm, DigestAuth, DigestAuthError, DigestAuthErrorKind, DigestQop};
pub use error::{HttpError, LocItem, ValidationError, ValidationErrors};
pub use extract::{
    Accept, ApiKey, ApiKeyConfig, ApiKeyError, ApiKeyErrorKind, ApiKeyLocation, AppState,
    Authorization, BasicAuth, BasicAuthError, BasicAuthErrorKind, BearerToken, BearerTokenError,
    BearerTokenErrorKind, ContentType, Cookie, CookieExtractError, CookieExtractErrorKind,
    CookieName, CsrfToken, CsrfTokenCookie, DEFAULT_JSON_LIMIT, DEFAULT_PAGE, DEFAULT_PER_PAGE,
    Form, FormExtractError, FormExtractErrorKind, FromHeaderValue, FromRequest, Header,
    HeaderExtractError, HeaderName, HeaderValues, Host, Json, JsonConfig, JsonExtractError,
    MAX_PER_PAGE, MultipartExtractError, NamedHeader, OAuth2BearerError, OAuth2BearerErrorKind,
    OAuth2PasswordBearer, OAuth2PasswordBearerConfig, Page, Pagination, PaginationConfig, Path,
    PathExtractError, PathParams, Query, QueryExtractError, QueryParams, SessionId, State,
    StateExtractError, UserAgent, Valid, ValidExtractError, Validate, XRequestId,
    snake_to_header_case,
};
pub use middleware::{
    AddResponseHeader, BoxFuture, ControlFlow, Cors, CorsConfig, Handler, Layer, Layered,
    Middleware, MiddlewareStack, NoopMiddleware, OriginPattern, PathPrefixFilter, ReferrerPolicy,
    RequestId, RequestIdConfig, RequestIdMiddleware, RequestResponseLogger, RequireHeader,
    SecurityHeaders, SecurityHeadersConfig, XFrameOptions,
};
pub use multipart::{
    DEFAULT_MAX_FIELDS, DEFAULT_MAX_FILE_SIZE, DEFAULT_MAX_TOTAL_SIZE, MultipartConfig,
    MultipartError, MultipartForm, MultipartParser, Part, UploadFile, parse_boundary,
};
pub use request::{
    BackgroundTasks, BackgroundTasksInner, Body, Headers, HttpVersion, Method, Request,
    RequestBodyStream, RequestBodyStreamError,
};
pub use response::{
    Binary, BodyStream, FileResponse, Html, IntoResponse, Link, LinkHeader, LinkRel, NoContent,
    Redirect, Response, ResponseBody, ResponseModelAliases, ResponseModelConfig, ResponseProduces,
    SameSite, SetCookie, StatusCode, Text, ValidatedResponse, apply_conditional, check_if_match,
    check_if_none_match, exclude_fields, include_fields, mime_type_for_extension,
};
pub use websocket::{
    Frame as WebSocketFrame, OpCode as WebSocketOpCode, WS_GUID, WebSocket, WebSocketError,
    WebSocketHandshakeError, websocket_accept_from_key,
};

// Re-export interactive docs helpers.
pub use docs::{
    DocsConfig, oauth2_redirect_html, oauth2_redirect_response, redoc_html, redoc_response,
    swagger_ui_html, swagger_ui_response,
};

// Re-export key asupersync types for convenience
pub use asupersync::{Budget, Cx, Outcome, RegionId, TaskId};

// Re-export security helpers
pub use password::{Algorithm, HashConfig, PasswordHasher, SecureCompare, constant_time_eq};

// Re-export testing utilities
pub use testing::{
    CookieJar, FixtureGuard, IntegrationTest, RequestBuilder, TestClient, TestFixture,
    TestResponse, json_contains,
};

// Re-export assertion macros (defined via #[macro_export] in testing module)
// Note: The macros assert_status!, assert_header!, assert_body_contains!,
// assert_json!, and assert_body_matches! are automatically exported at the crate root
// due to #[macro_export]. Users can import them with `use fastapi_core::assert_status;`

// Re-export logging utilities
pub use logging::{AutoSpan, LogConfig, LogEntry, LogLevel, Span};

// Re-export app utilities
pub use app::{
    App, AppBuilder, AppConfig, ExceptionHandlers, OpenApiConfig, RouteEntry, StartupHook,
    StartupHookError, StartupOutcome, StateContainer,
};

// Re-export shutdown utilities
pub use shutdown::{
    GracefulConfig, GracefulShutdown, InFlightGuard, ShutdownAware, ShutdownController,
    ShutdownHook, ShutdownOutcome, ShutdownPhase, ShutdownReceiver, grace_expired_cancel_reason,
    shutdown_cancel_reason, subdivide_grace_budget,
};
