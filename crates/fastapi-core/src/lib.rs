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
pub mod bench;
mod context;
pub mod coverage;
mod dependency;
pub mod docs;
pub mod error;
mod extract;
pub mod fixtures;
pub mod health;
pub mod logging;
pub mod middleware;
pub mod ndjson;
mod request;
mod response;
pub mod routing;
pub mod shutdown;
pub mod sse;
pub mod static_files;
pub mod testing;

#[cfg(feature = "proptest")]
pub mod proptest;

pub use context::{
    BodyLimitConfig, CancelledError, DEFAULT_MAX_BODY_SIZE, IntoOutcome, RequestContext,
};
pub use dependency::{
    CircularDependencyError, CleanupFn, CleanupStack, DefaultConfig, DefaultDependencyConfig,
    DependencyCache, DependencyOverrides, DependencyScope, DependencyScopeError, Depends,
    DependsCleanup, DependsConfig, FromDependency, FromDependencyWithCleanup, NoCache,
};
pub use error::{
    DebugConfig, DebugInfo, HttpError, LocItem, ResponseValidationError, ValidationError,
    ValidationErrors, disable_debug_mode, enable_debug_mode, is_debug_mode_enabled,
};
pub use extract::{
    Accept, AcceptEncodingHeader, AcceptEncodingItem, AcceptHeader, AcceptItem,
    AcceptLanguageHeader, AcceptLanguageItem, ApiKeyCookie, ApiKeyCookieConfig, ApiKeyCookieError,
    ApiKeyHeader, ApiKeyHeaderConfig, ApiKeyHeaderError, ApiKeyQuery, ApiKeyQueryConfig,
    ApiKeyQueryError, AppState, Authorization, BackgroundTasks, BackgroundTasksInner, BasicAuth,
    BasicAuthError, BearerToken, BearerTokenError, Bytes, ContentType, Cookie, CookieExtractError,
    CookieName, CookiePrefix, CookiePrefixError, CsrfTokenCookie, DEFAULT_API_KEY_COOKIE,
    DEFAULT_API_KEY_HEADER, DEFAULT_API_KEY_QUERY_PARAM, DEFAULT_FORM_LIMIT, DEFAULT_JSON_LIMIT,
    DEFAULT_MULTIPART_FILE_SIZE, DEFAULT_MULTIPART_MAX_FIELDS, DEFAULT_MULTIPART_TOTAL_SIZE,
    DEFAULT_PAGE, DEFAULT_PER_PAGE, DEFAULT_RAW_BODY_LIMIT, File, FileConfig, Form, FormConfig,
    FormExtractError, FromHeaderValue, FromRequest, Header, HeaderExtractError, HeaderName,
    HeaderValues, Host, Json, JsonConfig, JsonExtractError, MAX_PER_PAGE, MediaType, Multipart,
    MultipartConfig, MultipartExtractError, MultipartPart, NamedHeader, NotAcceptableError,
    OAuth2AuthorizationCodeBearer, OAuth2AuthorizationCodeBearerConfig, OAuth2BearerError,
    OAuth2BearerErrorKind, OAuth2PasswordBearer, OAuth2PasswordBearerConfig,
    OAuth2PasswordFormError, OAuth2PasswordRequestForm, OAuth2PasswordRequestFormStrict, Page,
    Pagination, PaginationConfig, Path, PathExtractError, PathParams, Query, QueryExtractError,
    QueryParams, RawBodyConfig, RawBodyError, RequestCookie, RequestCookies, RequestRef,
    ResponseMut, ResponseMutations, SameSite, SecureCompare, SecurityScopes, SecurityScopesError,
    SessionIdCookie, State, StateExtractError, StringBody, UploadedFile, UserAgent, VaryBuilder,
    XRequestId, constant_time_eq, constant_time_str_eq, snake_to_header_case,
};
pub use middleware::{
    AddResponseHeader, BoxFuture, CompositeKeyExtractor, ConditionalInterceptor, ControlFlow, Cors,
    CorsConfig, CsrfConfig, CsrfMiddleware, CsrfMode, CsrfToken, DebugInfoInterceptor,
    ErrorResponseTransformer, Handler, HeaderKeyExtractor, HeaderTransformInterceptor,
    HttpsRedirectConfig, HttpsRedirectMiddleware, InspectionVerbosity, IpKeyExtractor,
    KeyExtractor, Layer, Layered, Middleware, MiddlewareStack, NoopMiddleware, OriginPattern,
    PathKeyExtractor, PathPrefixFilter, RateLimitAlgorithm, RateLimitBuilder, RateLimitConfig,
    RateLimitMiddleware, RateLimitResult, ReferrerPolicy, RequestId, RequestIdConfig,
    RequestIdMiddleware, RequestInspectionMiddleware, RequestResponseLogger, RequireHeader,
    ResponseBodyTransform, ResponseInterceptor, ResponseInterceptorContext,
    ResponseInterceptorMiddleware, ResponseInterceptorStack, SecurityHeaders,
    SecurityHeadersConfig, ServerTimingBuilder, ServerTimingEntry, TimingHistogram,
    TimingHistogramBucket, TimingInterceptor, TimingMetrics, TimingMetricsConfig,
    TimingMetricsMiddleware, TraceRejectionMiddleware, XFrameOptions,
};
#[cfg(feature = "compression")]
pub use middleware::{CompressionConfig, CompressionMiddleware};
pub use ndjson::{
    NDJSON_CONTENT_TYPE, NDJSON_CONTENT_TYPE_ALT, NdjsonConfig, NdjsonResponse, NdjsonStream,
    ndjson_iter, ndjson_response,
};
pub use request::{
    Body, Headers, HttpVersion, Method, Request, RequestBodyStream, RequestBodyStreamError,
};
pub use response::{
    Binary, BinaryWithType, BodyStream, FileResponse, Html, IntoResponse, NoContent, Redirect,
    Response, ResponseBody, ResponseModel, ResponseModelConfig, ResponseProduces, StatusCode, Text,
    ValidatedResponse, exclude_fields, include_fields, mime_type_for_extension,
};
pub use sse::{SseConfig, SseEvent, SseResponse, SseStream, sse_response};
pub use static_files::{StaticFiles, StaticFilesConfig};

// Re-export key asupersync types for convenience
pub use asupersync::{Budget, Cx, Outcome, RegionId, TaskId};

// Re-export testing utilities
pub use testing::{
    CapturedLog, CookieJar, E2ECapture, E2EReport, E2EScenario, E2EStep, E2EStepResult,
    FixtureGuard, IntegrationTest, IntegrationTestContext, LogCapture, RequestBuilder,
    ResponseDiff, TestClient, TestFixture, TestLogger, TestResponse, TestServer, TestServerConfig,
    TestServerLogEntry, TestTimings, json_contains,
};
// Note: e2e_test!, assert_with_logs!, assert_eq_with_logs! macros are automatically exported
// at crate root via #[macro_export]

// Re-export assertion macros (defined via #[macro_export] in testing module)
// Note: The macros assert_status!, assert_header!, assert_body_contains!,
// assert_json!, and assert_body_matches! are automatically exported at the crate root
// due to #[macro_export]. Users can import them with `use fastapi_core::assert_status;`

// Re-export coverage utilities
pub use coverage::{
    BranchHits, CoverageConfig, CoverageReport, CoverageTracker, EndpointHits, OutputFormat,
};

// Re-export fixture factories
pub use fixtures::{
    AuthFactory, CommonFixtures, JsonArrayFactory, JsonFactory, JsonObjectFactory, JwtFactory,
    RequestFactory, ResponseFactory, UserFactory,
};

// Re-export logging utilities
pub use logging::{AutoSpan, LogConfig, LogEntry, LogLevel, Span};

// Re-export health check utilities
pub use health::{
    HealthCheckRegistry, HealthCheckResult, HealthReport, HealthStatus, basic_health_handler,
    detailed_health_handler, liveness_handler, readiness_handler,
};

// Re-export documentation utilities
pub use docs::{
    DocsConfig, oauth2_redirect_html, oauth2_redirect_response, redoc_html, redoc_response,
    swagger_ui_html, swagger_ui_response,
};

// Re-export api_router utilities
pub use api_router::{APIRouter, IncludeConfig, ResponseDef, RouterDependency, RouterRoute};

// Re-export app utilities
pub use app::{
    App, AppBuilder, AppConfig, BoxLifespanFn, ConfigError, ExceptionHandlers, HasState,
    LifespanError, LifespanScope, MountedApp, RequiresState, RouteEntry, StartupHook,
    StartupHookError, StartupOutcome, StateContainer, StateRegistry,
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
