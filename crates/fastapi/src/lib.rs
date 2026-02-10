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
//! # Role In The System
//!
//! `fastapi_rust` is the user-facing facade crate. It re-exports the framework's
//! core types, macros, and utilities from the sub-crates so applications only
//! need a single dependency. All real behavior lives in the sub-crates listed
//! below; this crate exists to provide a cohesive, ergonomic API surface.
//!
//! # Quick Start
//!
//! ```ignore
//! use fastapi_rust::prelude::*;
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
//!     let app = App::builder()
//!         .title("My API")
//!         .route_entry(get_item_route())
//!         .build();
//!
//!     let rt = asupersync::runtime::RuntimeBuilder::current_thread()
//!         .build()
//!         .expect("runtime must build");
//!     rt.block_on(async move {
//!         serve(app, "0.0.0.0:8000").await.expect("server must start");
//!     });
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
//! | Crate | Purpose |
//! |-------|---------|
//! | `fastapi_core` | Core types (Request, Response, Error), extractors, middleware, DI |
//! | `fastapi_http` | Zero-copy HTTP/1.1 parser, TCP server, chunked encoding |
//! | `fastapi_router` | Trie-based router with O(log n) lookups |
//! | `fastapi_macros` | Procedural macros (`#[get]`, `#[derive(Validate)]`, `#[derive(JsonSchema)]`) |
//! | `fastapi_openapi` | OpenAPI 3.1 schema types and generation |
//! | `fastapi_output` | Agent-aware rich console output (optional) |
//!
//! # Feature Flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `output` | **yes** | Rich console output with agent detection (includes `fastapi-output/rich`) |
//! | `output-plain` | no | Plain-text-only output (smaller binary, no ANSI codes) |
//! | `full` | no | All output features including every theme and component |
//!
//! ## Sub-crate Feature Flags
//!
//! **`fastapi-core`:**
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `regex` | Regex support in testing assertions |
//! | `compression` | Response compression middleware (gzip via flate2) |
//! | `proptest` | Property-based testing support |

//!
//! # Cookbook
//!
//! Common patterns for building APIs with fastapi_rust.
//!
//! ## JSON CRUD Handler
//!
//! ```ignore
//! use fastapi_rust::prelude::*;
//!
//! #[get("/items/{id}")]
//! async fn get_item(cx: &Cx, id: Path<i64>, state: State<AppState>) -> Result<Json<Item>, HttpError> {
//!     let item = state.db.find(id.0).await?;
//!     Ok(Json(item))
//! }
//! ```
//!
//! ## Pagination
//!
//! ```ignore
//! use fastapi_rust::prelude::*;
//!
//! #[get("/items")]
//! async fn list_items(cx: &Cx, page: Pagination) -> Json<Page<Item>> {
//!     // page.page() returns current page (default: 1)
//!     // page.per_page() returns items per page (default: 20, max: 100)
//!     let items = db.list(page.offset(), page.limit()).await;
//!     Json(Page::new(items, total_count, page.page(), page.per_page()))
//! }
//! ```
//!
//! ## Bearer Token Authentication
//!
//! ```ignore
//! use fastapi_rust::prelude::*;
//!
//! #[get("/protected")]
//! async fn protected(cx: &Cx, token: BearerToken) -> Json<UserInfo> {
//!     let user = verify_jwt(token.token()).await?;
//!     Json(user)
//! }
//! ```
//!
//! ## Background Tasks
//!
//! ```ignore
//! use fastapi_rust::prelude::*;
//!
//! #[post("/send-email")]
//! async fn send_email(cx: &Cx, body: Json<EmailRequest>, tasks: BackgroundTasks) -> StatusCode {
//!     tasks.add(move || {
//!         // Runs after response is sent
//!         email_service::send(&body.to, &body.subject, &body.body);
//!     });
//!     StatusCode::ACCEPTED
//! }
//! ```
//!
//! ## CORS + Rate Limiting Middleware
//!
//! ```ignore
//! use fastapi_rust::prelude::*;
//!
//! let app = App::new()
//!     .middleware(Cors::new().allow_any_origin(true).allow_credentials(true))
//!     .middleware(RateLimitBuilder::new().max_requests(100).window_secs(60).build());
//! ```
//!
//! ## Error Handling
//!
//! ```ignore
//! use fastapi_rust::prelude::*;
//!
//! // Custom errors implement IntoResponse automatically via HttpError
//! fn not_found(resource: &str, id: u64) -> HttpError {
//!     HttpError::not_found(format!("{} {} not found", resource, id))
//! }
//! ```
//!
//! # Migrating from Python FastAPI
//!
//! ## Key Differences
//!
//! | Python FastAPI | fastapi_rust | Notes |
//! |----------------|--------------|-------|
//! | `@app.get("/")` | `#[get("/")]` | Proc macro instead of decorator |
//! | `async def handler(item: Item)` | `async fn handler(cx: &Cx, item: Json<Item>)` | Explicit `Cx` context + typed extractors |
//! | `Depends(get_db)` | `Depends<DbPool>` | Type-based DI, not function-based |
//! | `HTTPException(404)` | `HttpError::not_found(msg)` | Typed error constructors |
//! | `BackgroundTasks` | `BackgroundTasks` | Same concept, different API |
//! | `Query(q: str)` | `Query<SearchParams>` | Struct-based query extraction |
//! | `Path(item_id: int)` | `Path<i64>` | Type-safe path parameters |
//! | `Body(...)` | `Json<T>` | Explicit JSON extraction |
//! | `Response(status_code=201)` | `StatusCode::CREATED` | Type-safe status codes |
//!
//! ## Async Runtime
//!
//! Python FastAPI uses `asyncio`. fastapi_rust uses `asupersync`, which provides:
//! - **Structured concurrency**: Request handlers run in regions
//! - **Cancel-correctness**: Graceful cancellation via checkpoints
//! - **Budgeted timeouts**: Request timeouts via budget exhaustion
//!
//! Every handler receives `&Cx` as its first parameter for async context.
//!
//! ## Dependency Injection
//!
//! Python uses function-based DI with `Depends(func)`. Rust uses trait-based DI:
//!
//! ```ignore
//! // Python:
//! // async def get_db():
//! //     yield db_session
//! //
//! // @app.get("/")
//! // async def handler(db: Session = Depends(get_db)):
//!
//! // Rust:
//! impl FromDependency for DbPool {
//!     async fn from_dependency(cx: &Cx, cache: &DependencyCache) -> Result<Self, HttpError> {
//!         Ok(DbPool::acquire(cx).await?)
//!     }
//! }
//!
//! #[get("/")]
//! async fn handler(cx: &Cx, db: Depends<DbPool>) -> Json<Data> { ... }
//! ```
//!
//! ## Validation
//!
//! Python uses Pydantic models. Rust uses `#[derive(Validate)]`:
//!
//! ```ignore
//! // Python:
//! // class Item(BaseModel):
//! //     name: str = Field(..., min_length=1, max_length=100)
//! //     price: float = Field(..., gt=0)
//!
//! // Rust:
//! #[derive(Validate)]
//! struct Item {
//!     #[validate(min_length = 1, max_length = 100)]
//!     name: String,
//!     #[validate(range(min = 0.01))]
//!     price: f64,
//! }
//! ```

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
    App, AppBuilder, AppConfig, ConfigError, Cors, CorsConfig, Cx, DefaultConfig,
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
        // asupersync context
        Cx,
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

/// Extractors module for request data extraction (extended).
pub mod extract {
    pub use fastapi_core::{
        Accept, AppState, Authorization, ContentType, FromHeaderValue, Header, HeaderExtractError,
        HeaderName, HeaderValues, Host, Json, JsonConfig, JsonExtractError, NamedHeader,
        OAuth2BearerError, OAuth2BearerErrorKind, OAuth2PasswordBearer, OAuth2PasswordBearerConfig,
        Path, PathExtractError, PathParams, Query, QueryExtractError, QueryParams, State,
        StateExtractError, UserAgent, XRequestId,
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

/// Extension trait for generating OpenAPI specifications from applications.
pub trait OpenApiExt {
    /// Generate an OpenAPI specification from the application.
    ///
    /// This creates an OpenAPI 3.1 document based on the application's
    /// configuration and registered routes.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi::prelude::*;
    /// use fastapi::OpenApiExt;
    ///
    /// let app = App::builder()
    ///     .config(AppConfig::new().name("My API").version("1.0.0"))
    ///     .build();
    ///
    /// let spec = app.openapi();
    /// println!("{}", serde_json::to_string_pretty(&spec).unwrap());
    /// ```
    fn openapi(&self) -> OpenApi;

    /// Generate an OpenAPI specification with custom configuration.
    fn openapi_with<F>(&self, configure: F) -> OpenApi
    where
        F: FnOnce(OpenApiBuilder) -> OpenApiBuilder;
}

impl OpenApiExt for App {
    fn openapi(&self) -> OpenApi {
        self.openapi_with(|b| b)
    }

    fn openapi_with<F>(&self, configure: F) -> OpenApi
    where
        F: FnOnce(OpenApiBuilder) -> OpenApiBuilder,
    {
        let mut builder = OpenApiBuilder::new(&self.config().name, &self.config().version);
        builder = configure(builder);
        builder.build()
    }
}

// Note: operation_id generation lives in `fastapi-router::Route` and in the
// OpenAPI builder layer; keep the facade crate lean.
