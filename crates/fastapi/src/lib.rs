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
    // Body extractors
    Json, JsonConfig, JsonExtractError,
    // Path parameters
    Path, PathExtractError, PathParams, PathParamsDeserializer,
    // Query string
    Query, QueryExtractError, QueryParams,
    // Headers
    Header, HeaderExtractError, HeaderValues, NamedHeader,
    // Common header types
    Accept, Authorization, ContentType, Host, UserAgent, XRequestId,
    // Cookies
    Cookie, CookiePrefix, CookiePrefixError, ResponseCookies, SameSite,
    // State
    State, AppState,
    // Auth extractors
    BasicAuth, BasicAuthError, BearerToken, BearerTokenError,
    OAuth2PasswordBearer, OAuth2PasswordBearerConfig, OAuth2BearerError,
    // Pagination
    Pagination, PaginationConfig, Page, DEFAULT_PAGE, DEFAULT_PER_PAGE, MAX_PER_PAGE,
    // Background tasks
    BackgroundTasks, BackgroundTasksInner,
    // Response mutations
    ResponseMut, ResponseMutations, SetResponseHeader,
    // Request utilities
    RequestRef, RequestContext,
};

// Re-export testing utilities
pub use fastapi_core::{CookieJar, RequestBuilder, TestClient, TestResponse};
pub use fastapi_macros::{JsonSchema, Validate, delete, get, head, options, patch, post, put};
pub use fastapi_openapi::{OpenApi, OpenApiBuilder};
pub use fastapi_router::{Route, Router};

// Re-export HTTP server types
pub use fastapi_http::{
    Server, ServerConfig, TcpServer,
    ServeError, ServerError,
    serve, serve_with_config,
    GracefulOutcome, ShutdownController, ShutdownReceiver,
};

/// Prelude module for convenient imports.
pub mod prelude {
    pub use crate::{
        // Core types
        App, AppBuilder, AppConfig, ConfigError, Cors, CorsConfig, DefaultConfig,
        DefaultDependencyConfig, DependencyOverrides, DependencyScope, Depends, DependsConfig,
        FromDependency, FromRequest, HttpError, IntoResponse, Method, NoCache,
        Request, RequestContext, RequestId, RequestIdMiddleware, Response, Route, Router,
        StatusCode, ValidationError, ValidationErrors,
        // Extractors
        Json, Path, Query, Header, Cookie, State,
        // Auth
        BasicAuth, BearerToken, OAuth2PasswordBearer,
        // Pagination
        Pagination, Page,
        // Macros
        JsonSchema, Validate, delete, get, head, options, patch, post, put,
        // OpenAPI
        OpenApi, OpenApiBuilder,
        // Server
        Server, ServerConfig, serve,
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
        Accept, AppState, Authorization, BackgroundTasks, BasicAuth, BearerToken,
        ContentType, Cookie, Header, HeaderValues, Host, Json, JsonConfig, NamedHeader,
        OAuth2PasswordBearer, Page, Pagination, PaginationConfig, Path, PathParams, Query,
        QueryParams, RequestRef, ResponseMut, ResponseMutations, State, UserAgent, XRequestId,
    };
}
