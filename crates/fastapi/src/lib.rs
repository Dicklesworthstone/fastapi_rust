//! Ultra-optimized Rust web framework inspired by FastAPI.
//!
//! fastapi_rust provides a type-safe, high-performance web framework with:
//!
//! - **Type-driven API design** — Route handlers declare types, framework extracts/validates automatically
//! - **Dependency injection** — Composable, testable request handling
//! - **Automatic OpenAPI** — Schema generation from type definitions
//! - **First-class async** — Built on asupersync for structured concurrency
//! - **Minimal dependencies** — Only asupersync + serde
//!
//! # Quick Start
//!
//! ```ignore
//! use fastapi::prelude::*;
//!
//! #[derive(Serialize, Deserialize, JsonSchema)]
//! struct Item {
//!     id: i64,
//!     name: String,
//! }
//!
//! #[get("/items/{id}")]
//! async fn get_item(cx: &Cx, id: Path<i64>) -> Json<Item> {
//!     Json(Item { id: id.0, name: "Example".into() })
//! }
//!
//! fn main() {
//!     let app = App::new()
//!         .title("My API")
//!         .route(get_item);
//!
//!     // Run with asupersync
//!     // asupersync::block_on(app.serve("0.0.0.0:8000"));
//! }
//! ```
//!
//! # Design Philosophy
//!
//! This framework is built with the following principles:
//!
//! 1. **Zero-cost abstractions** — No runtime reflection, everything at compile time
//! 2. **Cancel-correct** — Leverages asupersync's structured concurrency
//! 3. **Minimal allocations** — Zero-copy parsing where possible
//! 4. **Familiar API** — FastAPI users will recognize the patterns
//!
//! # Crate Structure
//!
//! - [`fastapi_core`] — Core types (Request, Response, Error)
//! - [`fastapi_http`] — Zero-copy HTTP/1.1 parser
//! - [`fastapi_router`] — Trie-based router
//! - [`fastapi_macros`] — Procedural macros (`#[get]`, `#[derive(Validate)]`)
//! - [`fastapi_openapi`] — OpenAPI 3.1 types and generation

#![forbid(unsafe_code)]
// Design doc at PROPOSED_RUST_ARCHITECTURE.md (not embedded - too many conceptual code examples)

// Re-export crates
pub use fastapi_core as core;
pub use fastapi_http as http;
pub use fastapi_macros as macros;
pub use fastapi_openapi as openapi;
pub use fastapi_router as router;

// Re-export commonly used types
pub use fastapi_core::{
    App, AppBuilder, AppConfig, ConfigError, Cors, CorsConfig, DefaultConfig,
    DefaultDependencyConfig, DependencyOverrides, DependencyScope, Depends, DependsConfig,
    FromDependency, FromRequest, HttpError, IntoResponse, Method, NoCache, Request, RequestId,
    RequestIdConfig, RequestIdMiddleware, Response, ResponseBody, StateContainer, StatusCode,
    ValidationError, ValidationErrors,
};

// Re-export extractors
pub use fastapi_core::{
    // Common header types
    Accept,
    AddResponseHeader,
    AppState,
    Authorization,
    // Background tasks
    BackgroundTasks,
    BackgroundTasksInner,
    // Auth extractors
    BasicAuth,
    BasicAuthError,
    BearerToken,
    BearerTokenError,
    ContentType,
    // Cookies
    Cookie,
    CookiePrefix,
    CookiePrefixError,
    DEFAULT_PAGE,
    DEFAULT_PER_PAGE,
    // Headers
    Header,
    HeaderExtractError,
    HeaderValues,
    Host,
    // Body extractors
    Json,
    JsonConfig,
    JsonExtractError,
    MAX_PER_PAGE,
    NamedHeader,
    OAuth2BearerError,
    OAuth2PasswordBearer,
    OAuth2PasswordBearerConfig,
    Page,
    // Pagination
    Pagination,
    PaginationConfig,
    // Path parameters
    Path,
    PathExtractError,
    PathParams,
    // Query string
    Query,
    QueryExtractError,
    QueryParams,
    RequestContext,
    RequestCookies,
    // Request utilities
    RequestRef,
    // Response mutations
    ResponseMut,
    ResponseMutations,
    SameSite,
    // State
    State,
    UserAgent,
    XRequestId,
};

// Re-export testing utilities
pub use fastapi_core::{CookieJar, RequestBuilder, TestClient, TestResponse};
pub use fastapi_macros::{JsonSchema, Validate, delete, get, head, options, patch, post, put};
pub use fastapi_openapi::{OpenApi, OpenApiBuilder, SchemaRegistry};
pub use fastapi_router::{
    // Route matching
    AllowedMethods,
    ConversionError,
    // Path parameter types
    Converter,
    // Error types
    InvalidRouteError,
    ParamInfo,
    ParamValue,
    // Core router types
    Route,
    RouteAddError,
    RouteConflictError,
    RouteLookup,
    RouteMatch,
    Router,
};

// Re-export HTTP server types
pub use fastapi_http::{
    GracefulOutcome, ServeError, Server, ServerConfig, ServerError, ShutdownController,
    ShutdownReceiver, TcpServer, serve, serve_with_config,
};

/// Prelude module for convenient imports.
pub mod prelude {
    pub use crate::{
        // Core types
        App,
        AppBuilder,
        AppConfig,
        // Auth
        BasicAuth,
        BearerToken,
        ConfigError,
        Cookie,
        Cors,
        CorsConfig,
        DefaultConfig,
        DefaultDependencyConfig,
        DependencyOverrides,
        DependencyScope,
        Depends,
        DependsConfig,
        FromDependency,
        FromRequest,
        Header,
        HttpError,
        IntoResponse,
        // Extractors
        Json,
        // Macros
        JsonSchema,
        Method,
        NoCache,
        OAuth2PasswordBearer,
        // OpenAPI
        OpenApi,
        OpenApiBuilder,
        Page,
        // Pagination
        Pagination,
        Path,
        Query,
        Request,
        RequestContext,
        RequestId,
        RequestIdMiddleware,
        Response,
        Route,
        Router,
        // Server
        Server,
        ServerConfig,
        State,
        StatusCode,
        Validate,
        ValidationError,
        ValidationErrors,
        delete,
        get,
        head,
        options,
        patch,
        post,
        put,
        serve,
    };
    pub use serde::{Deserialize, Serialize};
}

/// Testing utilities module.
pub mod testing {
    pub use fastapi_core::testing::{CookieJar, RequestBuilder, TestClient, TestResponse};
}

/// Extractors module for type-safe request data extraction.
pub mod extractors {
    pub use fastapi_core::{
        Accept, AppState, Authorization, BackgroundTasks, BasicAuth, BearerToken, ContentType,
        Cookie, Header, HeaderValues, Host, Json, JsonConfig, NamedHeader, OAuth2PasswordBearer,
        Page, Pagination, PaginationConfig, Path, PathParams, Query, QueryParams, RequestRef,
        ResponseMut, ResponseMutations, State, UserAgent, XRequestId,
    };
}

/// HTTP server module with server types and configuration.
pub mod server {
    pub use fastapi_http::{
        // Configuration constants
        DEFAULT_DRAIN_TIMEOUT_SECS,
        DEFAULT_KEEP_ALIVE_TIMEOUT_SECS,
        DEFAULT_MAX_CONNECTIONS,
        DEFAULT_MAX_REQUESTS_PER_CONNECTION,
        DEFAULT_READ_BUFFER_SIZE,
        DEFAULT_REQUEST_TIMEOUT_SECS,
        // Shutdown coordination
        GracefulOutcome,
        // Error types
        ServeError,
        // Server types
        Server,
        ServerConfig,
        ServerError,
        ShutdownController,
        ShutdownReceiver,
        TcpServer,
        // Server functions
        serve,
        serve_with_config,
    };
}
