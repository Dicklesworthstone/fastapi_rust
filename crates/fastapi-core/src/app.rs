//! Application builder and runtime for fastapi_rust.
//!
//! This module provides a fluent API for building web applications with
//! type-safe route registration, middleware ordering, and shared state.
//!
//! # Design Principles
//!
//! - **Fluent Builder API**: Chain methods to configure the application
//! - **Type-Safe State**: Shared state is type-checked at compile time
//! - **Explicit Middleware Order**: Middleware runs in registration order
//! - **Compile-Time Validation**: Invalid configurations fail at compile time
//!
//! # Example
//!
//! ```ignore
//! use fastapi_core::app::{App, AppBuilder};
//! use fastapi_core::{Request, Response, RequestContext};
//!
//! async fn hello(ctx: &RequestContext, req: &mut Request) -> Response {
//!     Response::ok().body_text("Hello, World!")
//! }
//!
//! async fn health(ctx: &RequestContext, req: &mut Request) -> Response {
//!     Response::ok().body_json(&serde_json::json!({"status": "healthy"}))
//! }
//!
//! let app = App::builder()
//!     .route("/", Method::Get, hello)
//!     .route("/health", Method::Get, health)
//!     .middleware(RequestIdMiddleware::new())
//!     .middleware(LoggingMiddleware::new())
//!     .build();
//! ```

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::{fs, io};

use crate::context::RequestContext;
use crate::dependency::{DependencyOverrides, FromDependency};
use crate::extract::PathParams;
use crate::middleware::{BoxFuture, Handler, Middleware, MiddlewareStack};
use crate::request::{Method, Request};
use crate::response::{Response, StatusCode};
use crate::routing::{RouteLookup, RouteTable, format_allow_header};
use crate::shutdown::ShutdownController;
use serde::Deserialize;

// ============================================================================
// Lifecycle Hook Types
// ============================================================================

/// A startup hook that runs before the server starts accepting connections.
pub enum StartupHook {
    /// Synchronous startup function.
    Sync(Box<dyn FnOnce() -> Result<(), StartupHookError> + Send>),
    /// Factory for async startup future.
    AsyncFactory(
        Box<
            dyn FnOnce() -> Pin<Box<dyn Future<Output = Result<(), StartupHookError>> + Send>>
                + Send,
        >,
    ),
}

impl StartupHook {
    /// Create a synchronous startup hook.
    pub fn sync<F>(f: F) -> Self
    where
        F: FnOnce() -> Result<(), StartupHookError> + Send + 'static,
    {
        Self::Sync(Box::new(f))
    }

    /// Create an async startup hook.
    pub fn async_fn<F, Fut>(f: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), StartupHookError>> + Send + 'static,
    {
        Self::AsyncFactory(Box::new(move || Box::pin(f())))
    }

    /// Run the hook synchronously.
    ///
    /// For async hooks, this returns the future to await.
    pub fn run(
        self,
    ) -> Result<
        Option<Pin<Box<dyn Future<Output = Result<(), StartupHookError>> + Send>>>,
        StartupHookError,
    > {
        match self {
            Self::Sync(f) => f().map(|()| None),
            Self::AsyncFactory(f) => Ok(Some(f())),
        }
    }
}

/// Error returned when a startup hook fails.
#[derive(Debug)]
pub struct StartupHookError {
    /// Name of the hook that failed (if provided).
    pub hook_name: Option<String>,
    /// The underlying error message.
    pub message: String,
    /// Whether the application should abort startup.
    pub abort: bool,
}

impl StartupHookError {
    /// Create a new startup hook error.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            hook_name: None,
            message: message.into(),
            abort: true,
        }
    }

    /// Set the hook name.
    #[must_use]
    pub fn with_hook_name(mut self, name: impl Into<String>) -> Self {
        self.hook_name = Some(name.into());
        self
    }

    /// Set whether to abort startup.
    #[must_use]
    pub fn with_abort(mut self, abort: bool) -> Self {
        self.abort = abort;
        self
    }

    /// Create an error that doesn't abort startup (just logs warning).
    pub fn non_fatal(message: impl Into<String>) -> Self {
        Self {
            hook_name: None,
            message: message.into(),
            abort: false,
        }
    }
}

impl std::fmt::Display for StartupHookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(name) = &self.hook_name {
            write!(f, "Startup hook '{}' failed: {}", name, self.message)
        } else {
            write!(f, "Startup hook failed: {}", self.message)
        }
    }
}

impl std::error::Error for StartupHookError {}

/// Outcome of running all startup hooks.
#[derive(Debug)]
pub enum StartupOutcome {
    /// All hooks succeeded.
    Success,
    /// Some hooks had non-fatal errors (logged but continued).
    PartialSuccess {
        /// Number of hooks that failed with non-fatal errors.
        warnings: usize,
    },
    /// A fatal hook error aborted startup.
    Aborted(StartupHookError),
}

impl StartupOutcome {
    /// Returns true if startup can proceed (Success or PartialSuccess).
    #[must_use]
    pub fn can_proceed(&self) -> bool {
        !matches!(self, Self::Aborted(_))
    }

    /// Returns the abort error, if any.
    pub fn into_error(self) -> Option<StartupHookError> {
        match self {
            Self::Aborted(e) => Some(e),
            _ => None,
        }
    }
}

/// A boxed handler function.
///
/// The handler may return a future that borrows from the context/request for
/// the duration of the call.
pub type BoxHandler = Box<
    dyn for<'a> Fn(&'a RequestContext, &'a mut Request) -> BoxFuture<'a, Response> + Send + Sync,
>;

/// A registered route with its handler.
#[derive(Clone)]
pub struct RouteEntry {
    /// The HTTP method for this route.
    pub method: Method,
    /// The path pattern for this route.
    pub path: String,
    /// The handler function.
    handler: Arc<BoxHandler>,
}

impl RouteEntry {
    /// Creates a new route entry.
    ///
    /// Note: The handler's future must not outlive the borrow of the
    /// context/request passed to it.
    pub fn new<H>(method: Method, path: impl Into<String>, handler: H) -> Self
    where
        H: for<'a> Fn(&'a RequestContext, &'a mut Request) -> BoxFuture<'a, Response>
            + Send
            + Sync
            + 'static,
    {
        let handler: BoxHandler = Box::new(move |ctx, req| handler(ctx, req));
        Self {
            method,
            path: path.into(),
            handler: Arc::new(handler),
        }
    }

    /// Calls the handler with the given context and request.
    pub async fn call(&self, ctx: &RequestContext, req: &mut Request) -> Response {
        (self.handler)(ctx, req).await
    }
}

impl std::fmt::Debug for RouteEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteEntry")
            .field("method", &self.method)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Type-safe application state container.
///
/// State is stored by type and can be accessed by handlers through
/// the `State<T>` extractor.
#[derive(Default)]
pub struct StateContainer {
    state: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl StateContainer {
    /// Creates a new empty state container.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: HashMap::new(),
        }
    }

    /// Inserts a value into the state container.
    ///
    /// If a value of the same type already exists, it is replaced.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.state.insert(TypeId::of::<T>(), Arc::new(value));
    }

    /// Gets a reference to a value in the state container.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.state
            .get(&TypeId::of::<T>())
            .and_then(|v| Arc::clone(v).downcast::<T>().ok())
    }

    /// Returns true if the state container contains a value of type T.
    pub fn contains<T: 'static>(&self) -> bool {
        self.state.contains_key(&TypeId::of::<T>())
    }

    /// Returns the number of values in the state container.
    pub fn len(&self) -> usize {
        self.state.len()
    }

    /// Returns true if the state container is empty.
    pub fn is_empty(&self) -> bool {
        self.state.is_empty()
    }
}

impl std::fmt::Debug for StateContainer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateContainer")
            .field("count", &self.state.len())
            .finish()
    }
}

// ============================================================================
// Exception Handler Registry
// ============================================================================

/// A boxed exception handler function.
///
/// The handler receives the RequestContext and a boxed error, and returns a Response.
pub type BoxExceptionHandler = Box<
    dyn Fn(&RequestContext, Box<dyn std::error::Error + Send + Sync>) -> Response + Send + Sync,
>;

/// A boxed panic handler function.
///
/// The handler receives the RequestContext (if available) and panic info string,
/// and returns a Response. This is called by the HTTP server layer when a panic
/// is caught via `catch_unwind`.
pub type BoxPanicHandler = Box<dyn Fn(Option<&RequestContext>, &str) -> Response + Send + Sync>;

/// Registry for custom exception handlers.
///
/// This allows applications to register handlers for specific error types,
/// converting errors into HTTP responses in a customizable way.
///
/// # Default Handlers
///
/// The registry comes with default handlers for common error types:
/// - [`HttpError`](crate::HttpError) → JSON response with status/detail
/// - [`ValidationErrors`](crate::ValidationErrors) → 422 with error list
/// - [`CancelledError`](crate::CancelledError) → 499 Client Closed Request
///
/// # Panic Handler
///
/// The registry also supports a panic handler that is invoked when a panic
/// is caught during request handling. This is typically used by the HTTP
/// server layer via `catch_unwind`. The default panic handler returns a
/// 500 Internal Server Error.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::app::ExceptionHandlers;
/// use fastapi_core::{RequestContext, Response, HttpError};
///
/// #[derive(Debug)]
/// struct MyCustomError(String);
///
/// impl std::fmt::Display for MyCustomError {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         write!(f, "Custom error: {}", self.0)
///     }
/// }
///
/// impl std::error::Error for MyCustomError {}
///
/// let handlers = ExceptionHandlers::new()
///     .handler(|_ctx, err: MyCustomError| {
///         Response::with_status(StatusCode::BAD_REQUEST)
///             .body_json(&serde_json::json!({"error": err.0}))
///     });
/// ```
pub struct ExceptionHandlers {
    handlers: HashMap<TypeId, BoxExceptionHandler>,
    panic_handler: Option<BoxPanicHandler>,
}

impl Default for ExceptionHandlers {
    fn default() -> Self {
        Self::new()
    }
}

impl ExceptionHandlers {
    /// Creates a new empty exception handler registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            panic_handler: None,
        }
    }

    /// Creates a registry with default handlers for common error types.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut handlers = Self::new();

        // Default handler for HttpError
        handlers.register::<crate::HttpError>(|_ctx, err| {
            use crate::IntoResponse;
            err.into_response()
        });

        // Default handler for ValidationErrors
        handlers.register::<crate::ValidationErrors>(|_ctx, err| {
            use crate::IntoResponse;
            err.into_response()
        });

        // Default handler for CancelledError -> 499 Client Closed Request
        handlers.register::<crate::CancelledError>(|_ctx, _err| {
            Response::with_status(StatusCode::CLIENT_CLOSED_REQUEST)
        });

        handlers
    }

    /// Registers a handler for a specific error type.
    ///
    /// The handler receives the error value directly (not boxed) for type safety.
    /// If a handler for the same type already exists, it is replaced.
    pub fn register<E>(
        &mut self,
        handler: impl Fn(&RequestContext, E) -> Response + Send + Sync + 'static,
    ) where
        E: std::error::Error + Send + Sync + 'static,
    {
        let boxed_handler: BoxExceptionHandler = Box::new(move |ctx, err| {
            // Try to downcast the error to the expected type
            match err.downcast::<E>() {
                Ok(typed_err) => handler(ctx, *typed_err),
                Err(_) => {
                    // This shouldn't happen if the registry is used correctly
                    Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        });
        self.handlers.insert(TypeId::of::<E>(), boxed_handler);
    }

    /// Registers a handler for a specific error type (builder pattern).
    #[must_use]
    pub fn handler<E>(
        mut self,
        handler: impl Fn(&RequestContext, E) -> Response + Send + Sync + 'static,
    ) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.register::<E>(handler);
        self
    }

    /// Handles an error by finding and invoking the appropriate handler.
    ///
    /// Returns `Some(Response)` if a handler was found for the error type,
    /// or `None` if no handler is registered.
    pub fn handle<E>(&self, ctx: &RequestContext, err: E) -> Option<Response>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<E>();
        self.handlers
            .get(&type_id)
            .map(|handler| handler(ctx, Box::new(err)))
    }

    /// Handles an error, falling back to a default 500 response if no handler is found.
    pub fn handle_or_default<E>(&self, ctx: &RequestContext, err: E) -> Response
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.handle(ctx, err)
            .unwrap_or_else(|| Response::with_status(StatusCode::INTERNAL_SERVER_ERROR))
    }

    /// Returns true if a handler is registered for the given error type.
    pub fn has_handler<E: 'static>(&self) -> bool {
        self.handlers.contains_key(&TypeId::of::<E>())
    }

    /// Returns the number of registered handlers.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Returns true if no handlers are registered.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Merges another handler registry into this one.
    ///
    /// Handlers from `other` will override handlers in `self` for the same error types.
    pub fn merge(&mut self, other: ExceptionHandlers) {
        self.handlers.extend(other.handlers);
        // Prefer other's panic handler if set
        if other.panic_handler.is_some() {
            self.panic_handler = other.panic_handler;
        }
    }

    // =========================================================================
    // Panic Handler
    // =========================================================================

    /// Sets a custom panic handler.
    ///
    /// The panic handler is called by the HTTP server layer when a panic is caught
    /// during request handling via `catch_unwind`. The handler receives the
    /// `RequestContext` (if available) and a panic message string.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let handlers = ExceptionHandlers::with_defaults()
    ///     .panic_handler(|ctx, panic_msg| {
    ///         // Log the panic
    ///         eprintln!("Request panicked: {}", panic_msg);
    ///
    ///         // Return a custom error response
    ///         Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
    ///             .body_json(&serde_json::json!({
    ///                 "error": "internal_server_error",
    ///                 "message": "An unexpected error occurred"
    ///             }))
    ///     });
    /// ```
    #[must_use]
    pub fn panic_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(Option<&RequestContext>, &str) -> Response + Send + Sync + 'static,
    {
        self.panic_handler = Some(Box::new(handler));
        self
    }

    /// Sets a custom panic handler (mutable reference version).
    pub fn set_panic_handler<F>(&mut self, handler: F)
    where
        F: Fn(Option<&RequestContext>, &str) -> Response + Send + Sync + 'static,
    {
        self.panic_handler = Some(Box::new(handler));
    }

    /// Handles a panic by invoking the configured panic handler.
    ///
    /// If no panic handler is configured, returns a default 500 Internal Server Error.
    ///
    /// This method is intended to be called by the HTTP server layer after catching
    /// a panic via `catch_unwind`.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The request context, if available when the panic occurred
    /// * `panic_info` - A string describing the panic (extracted from the panic payload)
    pub fn handle_panic(&self, ctx: Option<&RequestContext>, panic_info: &str) -> Response {
        if let Some(handler) = &self.panic_handler {
            handler(ctx, panic_info)
        } else {
            Self::default_panic_response()
        }
    }

    /// Returns the default response for panics: 500 Internal Server Error.
    #[must_use]
    pub fn default_panic_response() -> Response {
        Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Returns true if a custom panic handler is registered.
    #[must_use]
    pub fn has_panic_handler(&self) -> bool {
        self.panic_handler.is_some()
    }

    /// Extracts a message string from a panic payload.
    ///
    /// This is a helper for use with `catch_unwind` results.
    #[must_use]
    pub fn extract_panic_message(payload: &(dyn std::any::Any + Send)) -> String {
        if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        }
    }
}

impl std::fmt::Debug for ExceptionHandlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExceptionHandlers")
            .field("count", &self.handlers.len())
            .field("has_panic_handler", &self.panic_handler.is_some())
            .finish()
    }
}

/// Application configuration.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Application name (used in logging and OpenAPI).
    pub name: String,
    /// Application version.
    pub version: String,
    /// Enable debug mode.
    pub debug: bool,
    /// Maximum request body size in bytes.
    pub max_body_size: usize,
    /// Default request timeout in milliseconds.
    pub request_timeout_ms: u64,
}

/// Configuration loading errors.
#[derive(Debug)]
pub enum ConfigError {
    /// Failed to read configuration file.
    Io(io::Error),
    /// Failed to parse JSON configuration.
    Json(serde_json::Error),
    /// Unsupported configuration format.
    UnsupportedFormat { path: PathBuf },
    /// Invalid environment variable value.
    InvalidEnvVar {
        /// Environment variable name.
        key: String,
        /// Raw value.
        value: String,
        /// Expected format.
        expected: String,
    },
    /// Validation failure.
    Validation(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "config I/O error: {err}"),
            Self::Json(err) => write!(f, "config JSON error: {err}"),
            Self::UnsupportedFormat { path } => {
                write!(f, "unsupported config format: {}", path.display())
            }
            Self::InvalidEnvVar {
                key,
                value,
                expected,
            } => write!(f, "invalid env var {key}='{value}' (expected {expected})"),
            Self::Validation(message) => write!(f, "invalid config: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

#[derive(Debug, Deserialize, Default)]
struct AppConfigFile {
    name: Option<String>,
    version: Option<String>,
    debug: Option<bool>,
    max_body_size: Option<usize>,
    request_timeout_ms: Option<u64>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: String::from("fastapi_rust"),
            version: String::from("0.1.0"),
            debug: false,
            max_body_size: 1024 * 1024, // 1MB
            request_timeout_ms: 30_000, // 30 seconds
        }
    }
}

impl AppConfig {
    const DEFAULT_ENV_PREFIX: &'static str = "FASTAPI_";
    const ENV_NAME: &'static str = "NAME";
    const ENV_VERSION: &'static str = "VERSION";
    const ENV_DEBUG: &'static str = "DEBUG";
    const ENV_MAX_BODY_SIZE: &'static str = "MAX_BODY_SIZE";
    const ENV_REQUEST_TIMEOUT_MS: &'static str = "REQUEST_TIMEOUT_MS";

    /// Creates a new configuration with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the application name.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Sets the application version.
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Enables or disables debug mode.
    #[must_use]
    pub fn debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// Sets the maximum request body size.
    #[must_use]
    pub fn max_body_size(mut self, size: usize) -> Self {
        self.max_body_size = size;
        self
    }

    /// Sets the default request timeout in milliseconds.
    #[must_use]
    pub fn request_timeout_ms(mut self, timeout: u64) -> Self {
        self.request_timeout_ms = timeout;
        self
    }

    /// Load configuration from environment variables.
    ///
    /// Variables (prefix `FASTAPI_` by default):
    /// - `FASTAPI_NAME`
    /// - `FASTAPI_VERSION`
    /// - `FASTAPI_DEBUG` (true/false/1/0/yes/no/on/off)
    /// - `FASTAPI_MAX_BODY_SIZE` (bytes)
    /// - `FASTAPI_REQUEST_TIMEOUT_MS`
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_env_with_prefix(Self::DEFAULT_ENV_PREFIX)
    }

    /// Load configuration from environment variables using a custom prefix.
    pub fn from_env_with_prefix(prefix: &str) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        config.apply_env(prefix)?;
        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a JSON file.
    ///
    /// Only JSON is supported for now to keep dependencies minimal.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        if !matches!(path.extension().and_then(|ext| ext.to_str()), Some("json")) {
            return Err(ConfigError::UnsupportedFormat {
                path: path.to_path_buf(),
            });
        }
        let contents = fs::read_to_string(path)?;
        let parsed: AppConfigFile = serde_json::from_str(&contents)?;
        let mut config = Self::default();
        config.apply_file(parsed);
        config.validate()?;
        Ok(config)
    }

    /// Load configuration from a JSON file then override with environment variables.
    pub fn from_env_and_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let mut config = Self::from_file(path)?;
        config.apply_env(Self::DEFAULT_ENV_PREFIX)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.name.trim().is_empty() {
            return Err(ConfigError::Validation(
                "name must not be empty".to_string(),
            ));
        }
        if self.version.trim().is_empty() {
            return Err(ConfigError::Validation(
                "version must not be empty".to_string(),
            ));
        }
        if self.max_body_size == 0 {
            return Err(ConfigError::Validation(
                "max_body_size must be greater than 0".to_string(),
            ));
        }
        if self.request_timeout_ms == 0 {
            return Err(ConfigError::Validation(
                "request_timeout_ms must be greater than 0".to_string(),
            ));
        }
        Ok(())
    }

    fn apply_file(&mut self, file: AppConfigFile) {
        if let Some(name) = file.name {
            self.name = name;
        }
        if let Some(version) = file.version {
            self.version = version;
        }
        if let Some(debug) = file.debug {
            self.debug = debug;
        }
        if let Some(max_body_size) = file.max_body_size {
            self.max_body_size = max_body_size;
        }
        if let Some(request_timeout_ms) = file.request_timeout_ms {
            self.request_timeout_ms = request_timeout_ms;
        }
    }

    fn apply_env(&mut self, prefix: &str) -> Result<(), ConfigError> {
        self.apply_env_with(prefix, fetch_env)
    }

    fn apply_env_with<F>(&mut self, prefix: &str, mut fetch: F) -> Result<(), ConfigError>
    where
        F: FnMut(&str) -> Result<Option<String>, ConfigError>,
    {
        let name_key = env_key(prefix, Self::ENV_NAME);
        let version_key = env_key(prefix, Self::ENV_VERSION);
        let debug_key = env_key(prefix, Self::ENV_DEBUG);
        let max_body_key = env_key(prefix, Self::ENV_MAX_BODY_SIZE);
        let timeout_key = env_key(prefix, Self::ENV_REQUEST_TIMEOUT_MS);

        if let Some(value) = fetch(&name_key)? {
            self.name = value;
        }
        if let Some(value) = fetch(&version_key)? {
            self.version = value;
        }
        if let Some(value) = fetch(&debug_key)? {
            self.debug = parse_bool(&debug_key, &value)?;
        }
        if let Some(value) = fetch(&max_body_key)? {
            self.max_body_size = parse_usize(&max_body_key, &value)?;
        }
        if let Some(value) = fetch(&timeout_key)? {
            self.request_timeout_ms = parse_u64(&timeout_key, &value)?;
        }

        Ok(())
    }
}

fn env_key(prefix: &str, key: &str) -> String {
    if prefix.ends_with('_') {
        format!("{prefix}{key}")
    } else {
        format!("{prefix}_{key}")
    }
}

fn fetch_env(key: &str) -> Result<Option<String>, ConfigError> {
    match env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::InvalidEnvVar {
            key: key.to_string(),
            value: "<non-utf8>".to_string(),
            expected: "valid UTF-8 string".to_string(),
        }),
    }
}

fn parse_bool(key: &str, value: &str) -> Result<bool, ConfigError> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidEnvVar {
            key: key.to_string(),
            value: value.to_string(),
            expected: "boolean (true/false/1/0/yes/no/on/off)".to_string(),
        }),
    }
}

fn parse_usize(key: &str, value: &str) -> Result<usize, ConfigError> {
    value.parse::<usize>().map_err(|_| ConfigError::InvalidEnvVar {
        key: key.to_string(),
        value: value.to_string(),
        expected: "usize".to_string(),
    })
}

fn parse_u64(key: &str, value: &str) -> Result<u64, ConfigError> {
    value.parse::<u64>().map_err(|_| ConfigError::InvalidEnvVar {
        key: key.to_string(),
        value: value.to_string(),
        expected: "u64".to_string(),
    })
}

/// Builder for constructing an [`App`].
///
/// Use this to configure routes, middleware, and shared state before
/// building the final application.
///
/// # Example
///
/// ```ignore
/// let app = App::builder()
///     .config(AppConfig::new().name("My API"))
///     .state(DatabasePool::new())
///     .middleware(LoggingMiddleware::new())
///     .on_startup(|| {
///         println!("Server starting...");
///         Ok(())
///     })
///     .on_shutdown(|| {
///         println!("Server stopping...");
///     })
///     .route("/", Method::Get, index_handler)
///     .route("/items", Method::Get, list_items)
///     .route("/items", Method::Post, create_item)
///     .route("/items/{id}", Method::Get, get_item)
///     .build();
/// ```
pub struct AppBuilder {
    config: AppConfig,
    routes: Vec<RouteEntry>,
    middleware: Vec<Arc<dyn Middleware>>,
    state: StateContainer,
    dependency_overrides: Arc<DependencyOverrides>,
    exception_handlers: ExceptionHandlers,
    startup_hooks: Vec<StartupHook>,
    shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    async_shutdown_hooks: Vec<Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>>,
}

impl Default for AppBuilder {
    fn default() -> Self {
        Self {
            config: AppConfig::default(),
            routes: Vec::new(),
            middleware: Vec::new(),
            state: StateContainer::default(),
            dependency_overrides: Arc::new(DependencyOverrides::new()),
            exception_handlers: ExceptionHandlers::default(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            async_shutdown_hooks: Vec::new(),
        }
    }
}

impl AppBuilder {
    /// Creates a new application builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the application configuration.
    #[must_use]
    pub fn config(mut self, config: AppConfig) -> Self {
        self.config = config;
        self
    }

    /// Adds a route to the application.
    ///
    /// Routes are matched in the order they are added.
    #[must_use]
    pub fn route<H, Fut>(mut self, path: impl Into<String>, method: Method, handler: H) -> Self
    where
        H: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.routes
            .push(RouteEntry::new(method, path, move |ctx, req| {
                Box::pin(handler(ctx, req))
            }));
        self
    }

    /// Adds a GET route.
    #[must_use]
    pub fn get<H, Fut>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.route(path, Method::Get, handler)
    }

    /// Adds a POST route.
    #[must_use]
    pub fn post<H, Fut>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.route(path, Method::Post, handler)
    }

    /// Adds a PUT route.
    #[must_use]
    pub fn put<H, Fut>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.route(path, Method::Put, handler)
    }

    /// Adds a DELETE route.
    #[must_use]
    pub fn delete<H, Fut>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.route(path, Method::Delete, handler)
    }

    /// Adds a PATCH route.
    #[must_use]
    pub fn patch<H, Fut>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.route(path, Method::Patch, handler)
    }

    /// Adds middleware to the application.
    ///
    /// Middleware is executed in the order it is added:
    /// - `before` hooks run first-to-last
    /// - `after` hooks run last-to-first
    #[must_use]
    pub fn middleware<M: Middleware + 'static>(mut self, middleware: M) -> Self {
        self.middleware.push(Arc::new(middleware));
        self
    }

    /// Includes routes from an [`APIRouter`](crate::api_router::APIRouter).
    ///
    /// This adds all routes from the router to the application, applying
    /// the router's prefix, tags, and dependencies.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::api_router::APIRouter;
    ///
    /// let users_router = APIRouter::new()
    ///     .prefix("/users")
    ///     .get("", list_users)
    ///     .get("/{id}", get_user);
    ///
    /// let app = App::builder()
    ///     .include_router(users_router)
    ///     .build();
    /// ```
    #[must_use]
    pub fn include_router(mut self, router: crate::api_router::APIRouter) -> Self {
        for entry in router.into_route_entries() {
            self.routes.push(entry);
        }
        self
    }

    /// Includes routes from an [`APIRouter`](crate::api_router::APIRouter) with configuration.
    ///
    /// This allows applying additional configuration when including a router,
    /// such as prepending a prefix, adding tags, or injecting dependencies.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::api_router::{APIRouter, IncludeConfig};
    ///
    /// let users_router = APIRouter::new()
    ///     .prefix("/users")
    ///     .get("", list_users);
    ///
    /// let config = IncludeConfig::new()
    ///     .prefix("/api/v1")
    ///     .tags(vec!["api"]);
    ///
    /// let app = App::builder()
    ///     .include_router_with_config(users_router, config)
    ///     .build();
    /// ```
    #[must_use]
    pub fn include_router_with_config(
        mut self,
        router: crate::api_router::APIRouter,
        config: crate::api_router::IncludeConfig,
    ) -> Self {
        // Apply config to a temporary router, then include
        let merged_router =
            crate::api_router::APIRouter::new().include_router_with_config(router, config);
        for entry in merged_router.into_route_entries() {
            self.routes.push(entry);
        }
        self
    }

    /// Adds shared state to the application.
    ///
    /// State can be accessed by handlers through the `State<T>` extractor.
    #[must_use]
    pub fn state<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.state.insert(state);
        self
    }

    /// Registers a dependency override for this application (useful in tests).
    #[must_use]
    pub fn override_dependency<T, F, Fut>(self, f: F) -> Self
    where
        T: FromDependency,
        F: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, T::Error>> + Send + 'static,
    {
        self.dependency_overrides.insert::<T, F, Fut>(f);
        self
    }

    /// Registers a fixed dependency override value.
    #[must_use]
    pub fn override_dependency_value<T>(self, value: T) -> Self
    where
        T: FromDependency,
    {
        self.dependency_overrides.insert_value(value);
        self
    }

    /// Clears all registered dependency overrides.
    #[must_use]
    pub fn clear_dependency_overrides(self) -> Self {
        self.dependency_overrides.clear();
        self
    }

    /// Registers a custom exception handler for a specific error type.
    ///
    /// When an error of type `E` occurs during request handling, the registered
    /// handler will be called to convert it into a response.
    ///
    /// # Example
    ///
    /// ```ignore
    /// #[derive(Debug)]
    /// struct AuthError(String);
    ///
    /// impl std::fmt::Display for AuthError {
    ///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    ///         write!(f, "Auth error: {}", self.0)
    ///     }
    /// }
    ///
    /// impl std::error::Error for AuthError {}
    ///
    /// let app = App::builder()
    ///     .exception_handler(|_ctx, err: AuthError| {
    ///         Response::with_status(StatusCode::UNAUTHORIZED)
    ///             .header("www-authenticate", b"Bearer".to_vec())
    ///             .body_json(&json!({"error": err.0}))
    ///     })
    ///     .build();
    /// ```
    #[must_use]
    pub fn exception_handler<E, H>(mut self, handler: H) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
        H: Fn(&RequestContext, E) -> Response + Send + Sync + 'static,
    {
        self.exception_handlers.register::<E>(handler);
        self
    }

    /// Sets the exception handlers registry.
    ///
    /// This replaces any previously registered handlers.
    #[must_use]
    pub fn exception_handlers(mut self, handlers: ExceptionHandlers) -> Self {
        self.exception_handlers = handlers;
        self
    }

    /// Uses default exception handlers for common error types.
    ///
    /// This registers handlers for:
    /// - [`HttpError`](crate::HttpError) → JSON response with status/detail
    /// - [`ValidationErrors`](crate::ValidationErrors) → 422 with error list
    #[must_use]
    pub fn with_default_exception_handlers(mut self) -> Self {
        self.exception_handlers = ExceptionHandlers::with_defaults();
        self
    }

    // =========================================================================
    // Lifecycle Hooks
    // =========================================================================

    /// Registers a synchronous startup hook.
    ///
    /// Startup hooks run before the server starts accepting connections,
    /// in the order they are registered (FIFO).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let app = App::builder()
    ///     .on_startup(|| {
    ///         println!("Connecting to database...");
    ///         Ok(())
    ///     })
    ///     .on_startup(|| {
    ///         println!("Loading configuration...");
    ///         Ok(())
    ///     })
    ///     .build();
    /// ```
    #[must_use]
    pub fn on_startup<F>(mut self, hook: F) -> Self
    where
        F: FnOnce() -> Result<(), StartupHookError> + Send + 'static,
    {
        self.startup_hooks.push(StartupHook::Sync(Box::new(hook)));
        self
    }

    /// Registers an async startup hook.
    ///
    /// Async startup hooks are awaited in registration order.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let app = App::builder()
    ///     .on_startup_async(|| async {
    ///         let pool = connect_to_database().await?;
    ///         Ok(())
    ///     })
    ///     .build();
    /// ```
    #[must_use]
    pub fn on_startup_async<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), StartupHookError>> + Send + 'static,
    {
        self.startup_hooks.push(StartupHook::AsyncFactory(Box::new(
            move || Box::pin(hook()),
        )));
        self
    }

    /// Registers a synchronous shutdown hook.
    ///
    /// Shutdown hooks run after the server stops accepting connections
    /// and all in-flight requests complete (or are cancelled).
    ///
    /// Shutdown hooks run in reverse registration order (LIFO), matching
    /// typical resource cleanup patterns (last acquired, first released).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let app = App::builder()
    ///     .on_shutdown(|| {
    ///         println!("Closing database connections...");
    ///     })
    ///     .build();
    /// ```
    #[must_use]
    pub fn on_shutdown<F>(mut self, hook: F) -> Self
    where
        F: FnOnce() + Send + 'static,
    {
        self.shutdown_hooks.push(Box::new(hook));
        self
    }

    /// Registers an async shutdown hook.
    ///
    /// Async shutdown hooks are awaited in reverse registration order (LIFO).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let app = App::builder()
    ///     .on_shutdown_async(|| async {
    ///         flush_metrics().await;
    ///     })
    ///     .build();
    /// ```
    #[must_use]
    pub fn on_shutdown_async<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.async_shutdown_hooks
            .push(Box::new(move || Box::pin(hook())));
        self
    }

    /// Returns the number of registered startup hooks.
    #[must_use]
    pub fn startup_hook_count(&self) -> usize {
        self.startup_hooks.len()
    }

    /// Returns the number of registered shutdown hooks.
    #[must_use]
    pub fn shutdown_hook_count(&self) -> usize {
        self.shutdown_hooks.len() + self.async_shutdown_hooks.len()
    }

    /// Builds the application.
    ///
    /// This consumes the builder and returns the configured [`App`].
    #[must_use]
    pub fn build(self) -> App {
        let mut middleware_stack = MiddlewareStack::with_capacity(self.middleware.len());
        for mw in self.middleware {
            middleware_stack.push_arc(mw);
        }

        // Build the route table from route entries
        // Each route stores its index into the routes vec
        let mut route_table = RouteTable::new();
        for (idx, route) in self.routes.iter().enumerate() {
            route_table.add(route.method, &route.path, idx);
        }

        App {
            config: self.config,
            routes: self.routes,
            route_table,
            middleware: middleware_stack,
            state: Arc::new(self.state),
            dependency_overrides: Arc::clone(&self.dependency_overrides),
            exception_handlers: Arc::new(self.exception_handlers),
            startup_hooks: parking_lot::Mutex::new(self.startup_hooks),
            shutdown_hooks: parking_lot::Mutex::new(self.shutdown_hooks),
            async_shutdown_hooks: parking_lot::Mutex::new(self.async_shutdown_hooks),
        }
    }
}

impl std::fmt::Debug for AppBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppBuilder")
            .field("config", &self.config)
            .field("routes", &self.routes.len())
            .field("middleware", &self.middleware.len())
            .field("state", &self.state)
            .field("dependency_overrides", &self.dependency_overrides)
            .field("exception_handlers", &self.exception_handlers)
            .field("startup_hooks", &self.startup_hooks.len())
            .field("shutdown_hooks", &self.shutdown_hook_count())
            .finish()
    }
}

/// A configured web application.
///
/// The `App` holds all routes, middleware, state, and lifecycle hooks,
/// and provides methods to handle incoming requests.
pub struct App {
    config: AppConfig,
    routes: Vec<RouteEntry>,
    /// Route table for path matching with parameter extraction.
    route_table: RouteTable<usize>,
    middleware: MiddlewareStack,
    state: Arc<StateContainer>,
    dependency_overrides: Arc<DependencyOverrides>,
    exception_handlers: Arc<ExceptionHandlers>,
    startup_hooks: parking_lot::Mutex<Vec<StartupHook>>,
    shutdown_hooks: parking_lot::Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    async_shutdown_hooks: parking_lot::Mutex<
        Vec<Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>>,
    >,
}

impl App {
    /// Creates a new application builder.
    #[must_use]
    pub fn builder() -> AppBuilder {
        AppBuilder::new()
    }

    /// Returns the application configuration.
    #[must_use]
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// Returns the number of registered routes.
    #[must_use]
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Returns the shared state container.
    #[must_use]
    pub fn state(&self) -> &Arc<StateContainer> {
        &self.state
    }

    /// Gets a reference to shared state of type T.
    pub fn get_state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.state.get::<T>()
    }

    /// Returns the dependency overrides registry.
    #[must_use]
    pub fn dependency_overrides(&self) -> &Arc<DependencyOverrides> {
        &self.dependency_overrides
    }

    /// Registers a dependency override for this application (useful in tests).
    pub fn override_dependency<T, F, Fut>(&self, f: F)
    where
        T: FromDependency,
        F: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, T::Error>> + Send + 'static,
    {
        self.dependency_overrides.insert::<T, F, Fut>(f);
    }

    /// Registers a fixed dependency override value.
    pub fn override_dependency_value<T>(&self, value: T)
    where
        T: FromDependency,
    {
        self.dependency_overrides.insert_value(value);
    }

    /// Clears all registered dependency overrides.
    pub fn clear_dependency_overrides(&self) {
        self.dependency_overrides.clear();
    }

    /// Returns the exception handlers registry.
    #[must_use]
    pub fn exception_handlers(&self) -> &Arc<ExceptionHandlers> {
        &self.exception_handlers
    }

    /// Handles an error using registered exception handlers.
    ///
    /// If a handler is registered for the error type, it will be invoked.
    /// Otherwise, returns `None`.
    pub fn handle_error<E>(&self, ctx: &RequestContext, err: E) -> Option<Response>
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.exception_handlers.handle(ctx, err)
    }

    /// Handles an error, returning a 500 response if no handler is registered.
    pub fn handle_error_or_default<E>(&self, ctx: &RequestContext, err: E) -> Response
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.exception_handlers.handle_or_default(ctx, err)
    }

    /// Handles an incoming request.
    ///
    /// This matches the request against registered routes, runs middleware,
    /// and returns the response. Path parameters are extracted and stored
    /// in request extensions for use by the `Path` extractor.
    ///
    /// # Special Parameter Injection
    ///
    /// - **ResponseMutations**: Headers and cookies set by handlers are automatically
    ///   applied to the final response.
    /// - **BackgroundTasks**: Tasks are stored in request extensions via
    ///   `BackgroundTasksInner`. The HTTP server should retrieve and execute these
    ///   after sending the response using `take_background_tasks()`.
    pub async fn handle(&self, ctx: &RequestContext, req: &mut Request) -> Response {
        // Use the route table for path matching with converter validation
        match self.route_table.lookup(req.path(), req.method()) {
            RouteLookup::Match { route: idx, params } => {
                // Store path parameters in request extensions for the Path extractor
                req.insert_extension(PathParams::from_pairs(params));

                // Initialize response mutations container for handlers to use
                req.insert_extension(crate::extract::ResponseMutations::new());

                // Initialize background tasks container for handlers to use
                req.insert_extension(crate::extract::BackgroundTasksInner::new());

                // Get the route entry by index
                let entry = &self.routes[*idx];
                let handler = RouteHandler { entry };
                let response = self.middleware.execute(&handler, ctx, req).await;

                // Run cleanup functions in LIFO order (even on error)
                ctx.cleanup_stack().run_cleanups().await;

                // Apply any response mutations set by the handler
                if let Some(mutations) = req.get_extension::<crate::extract::ResponseMutations>() {
                    mutations.clone().apply(response)
                } else {
                    response
                }
            }
            RouteLookup::MethodNotAllowed { allowed } => {
                Response::with_status(StatusCode::METHOD_NOT_ALLOWED)
                    .header("Allow", format_allow_header(&allowed).into_bytes())
            }
            RouteLookup::NotFound => Response::with_status(StatusCode::NOT_FOUND),
        }
    }

    /// Take background tasks from a request after handling.
    ///
    /// This should be called by the HTTP server after `handle()` returns
    /// and the response has been sent to the client. The returned tasks
    /// should be executed asynchronously.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let response = app.handle(&ctx, &mut request).await;
    /// // Send response to client...
    /// if let Some(tasks) = App::take_background_tasks(&mut request) {
    ///     tasks.execute_all().await;
    /// }
    /// ```
    #[must_use]
    pub fn take_background_tasks(req: &mut Request) -> Option<crate::extract::BackgroundTasks> {
        req.get_extension::<crate::extract::BackgroundTasksInner>()
            .map(|inner| crate::extract::BackgroundTasks::from_inner(inner.clone()))
    }

    // =========================================================================
    // Lifecycle Hook Execution
    // =========================================================================

    /// Runs all startup hooks.
    ///
    /// Hooks run in registration order (FIFO). If a hook returns an error
    /// with `abort: true`, execution stops and returns `StartupOutcome::Aborted`.
    ///
    /// This consumes the startup hooks - they can only be run once.
    ///
    /// # Returns
    ///
    /// - `StartupOutcome::Success` if all hooks succeeded
    /// - `StartupOutcome::PartialSuccess` if some hooks had non-fatal errors
    /// - `StartupOutcome::Aborted` if a fatal hook error occurred
    pub async fn run_startup_hooks(&self) -> StartupOutcome {
        let hooks: Vec<StartupHook> = std::mem::take(&mut *self.startup_hooks.lock());
        let mut warnings = 0;

        for hook in hooks {
            match hook.run() {
                Ok(None) => {
                    // Sync hook succeeded
                }
                Ok(Some(fut)) => {
                    // Async hook - await it
                    match fut.await {
                        Ok(()) => {}
                        Err(e) if e.abort => {
                            return StartupOutcome::Aborted(e);
                        }
                        Err(_) => {
                            warnings += 1;
                        }
                    }
                }
                Err(e) if e.abort => {
                    return StartupOutcome::Aborted(e);
                }
                Err(_) => {
                    warnings += 1;
                }
            }
        }

        if warnings > 0 {
            StartupOutcome::PartialSuccess { warnings }
        } else {
            StartupOutcome::Success
        }
    }

    /// Runs all shutdown hooks.
    ///
    /// Hooks run in reverse registration order (LIFO). Errors are logged
    /// but do not stop other hooks from running.
    ///
    /// This consumes the shutdown hooks - they can only be run once.
    pub async fn run_shutdown_hooks(&self) {
        // Run async hooks first (LIFO)
        let async_hooks: Vec<_> = std::mem::take(&mut *self.async_shutdown_hooks.lock());
        for hook in async_hooks.into_iter().rev() {
            let fut = hook();
            fut.await;
        }

        // Run sync hooks (LIFO)
        let sync_hooks: Vec<_> = std::mem::take(&mut *self.shutdown_hooks.lock());
        for hook in sync_hooks.into_iter().rev() {
            hook();
        }
    }

    /// Transfers shutdown hooks to a [`ShutdownController`].
    ///
    /// This moves all registered shutdown hooks to the controller, which
    /// will run them during the appropriate shutdown phase.
    ///
    /// Call this when integrating with the server's shutdown mechanism.
    pub fn transfer_shutdown_hooks(&self, controller: &ShutdownController) {
        // Transfer sync hooks (they'll run in LIFO due to how pop_hook works)
        let sync_hooks: Vec<_> = std::mem::take(&mut *self.shutdown_hooks.lock());
        for hook in sync_hooks {
            controller.register_hook(hook);
        }

        // Transfer async hooks
        let async_hooks: Vec<_> = std::mem::take(&mut *self.async_shutdown_hooks.lock());
        for hook in async_hooks {
            controller.register_async_hook(move || hook());
        }
    }

    /// Returns the number of pending startup hooks.
    #[must_use]
    pub fn pending_startup_hooks(&self) -> usize {
        self.startup_hooks.lock().len()
    }

    /// Returns the number of pending shutdown hooks.
    #[must_use]
    pub fn pending_shutdown_hooks(&self) -> usize {
        self.shutdown_hooks.lock().len() + self.async_shutdown_hooks.lock().len()
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("config", &self.config)
            .field("routes", &self.routes.len())
            .field("middleware", &self.middleware.len())
            .field("state", &self.state)
            .field("dependency_overrides", &self.dependency_overrides)
            .field("exception_handlers", &self.exception_handlers)
            .field("startup_hooks", &self.startup_hooks.lock().len())
            .field("shutdown_hooks", &self.pending_shutdown_hooks())
            .finish()
    }
}

impl Handler for App {
    fn call<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, Response> {
        Box::pin(async move { self.handle(ctx, req).await })
    }

    fn dependency_overrides(&self) -> Option<Arc<DependencyOverrides>> {
        Some(Arc::clone(&self.dependency_overrides))
    }
}

/// Handler wrapper for a route entry.
struct RouteHandler<'a> {
    entry: &'a RouteEntry,
}

impl<'a> Handler for RouteHandler<'a> {
    fn call<'b>(
        &'b self,
        ctx: &'b RequestContext,
        req: &'b mut Request,
    ) -> BoxFuture<'b, Response> {
        let handler = self.entry.handler.clone();
        Box::pin(async move { handler(ctx, req).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::response::ResponseBody;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env_lock<F: FnOnce()>(f: F) {
        let _guard = ENV_LOCK.lock().unwrap();
        f();
    }

    fn clear_config_env() {
        for key in [
            "FASTAPI_NAME",
            "FASTAPI_VERSION",
            "FASTAPI_DEBUG",
            "FASTAPI_MAX_BODY_SIZE",
            "FASTAPI_REQUEST_TIMEOUT_MS",
        ] {
            std::env::remove_var(key);
        }
    }

    // Test handlers that return 'static futures (no borrowing from parameters)
    fn test_handler(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
        std::future::ready(Response::ok().body(ResponseBody::Bytes(b"Hello, World!".to_vec())))
    }

    fn health_handler(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
        std::future::ready(Response::ok().body(ResponseBody::Bytes(b"OK".to_vec())))
    }

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 1)
    }

    #[test]
    fn app_builder_creates_app() {
        let app = App::builder()
            .config(AppConfig::new().name("Test App"))
            .get("/", test_handler)
            .get("/health", health_handler)
            .build();

        assert_eq!(app.route_count(), 2);
        assert_eq!(app.config().name, "Test App");
    }

    #[test]
    fn app_config_builder() {
        let config = AppConfig::new()
            .name("My API")
            .version("1.0.0")
            .debug(true)
            .max_body_size(2 * 1024 * 1024)
            .request_timeout_ms(60_000);

        assert_eq!(config.name, "My API");
        assert_eq!(config.version, "1.0.0");
        assert!(config.debug);
        assert_eq!(config.max_body_size, 2 * 1024 * 1024);
        assert_eq!(config.request_timeout_ms, 60_000);
    }

    #[test]
    fn app_config_defaults() {
        let config = AppConfig::default();
        assert_eq!(config.name, "fastapi_rust");
        assert_eq!(config.version, "0.1.0");
        assert!(!config.debug);
        assert_eq!(config.max_body_size, 1024 * 1024);
        assert_eq!(config.request_timeout_ms, 30_000);
    }

    #[test]
    fn app_config_from_env_parses_values() {
        with_env_lock(|| {
            clear_config_env();
            std::env::set_var("FASTAPI_NAME", "Env API");
            std::env::set_var("FASTAPI_VERSION", "9.9.9");
            std::env::set_var("FASTAPI_DEBUG", "true");
            std::env::set_var("FASTAPI_MAX_BODY_SIZE", "4096");
            std::env::set_var("FASTAPI_REQUEST_TIMEOUT_MS", "15000");

            let config = AppConfig::from_env().expect("env config");
            assert_eq!(config.name, "Env API");
            assert_eq!(config.version, "9.9.9");
            assert!(config.debug);
            assert_eq!(config.max_body_size, 4096);
            assert_eq!(config.request_timeout_ms, 15_000);
            clear_config_env();
        });
    }

    #[test]
    fn app_config_from_env_invalid_value() {
        with_env_lock(|| {
            clear_config_env();
            std::env::set_var("FASTAPI_MAX_BODY_SIZE", "not-a-number");
            let err = AppConfig::from_env().expect_err("invalid env should error");
            match err {
                ConfigError::InvalidEnvVar { key, .. } => {
                    assert_eq!(key, "FASTAPI_MAX_BODY_SIZE");
                }
                _ => panic!("expected invalid env var error"),
            }
            clear_config_env();
        });
    }

    #[test]
    fn app_config_validation_rejects_empty_name() {
        let mut config = AppConfig::default();
        config.name = String::new();
        let err = config.validate().expect_err("empty name invalid");
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn app_config_from_file_json() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("fastapi_config_{stamp}.json"));

        let json = r#"{
  "name": "File API",
  "version": "2.1.0",
  "debug": true,
  "max_body_size": 2048,
  "request_timeout_ms": 8000
}"#;
        std::fs::write(&path, json).expect("write temp config");

        let config = AppConfig::from_file(&path).expect("file config");
        assert_eq!(config.name, "File API");
        assert_eq!(config.version, "2.1.0");
        assert!(config.debug);
        assert_eq!(config.max_body_size, 2048);
        assert_eq!(config.request_timeout_ms, 8000);
    }

    #[test]
    fn app_config_env_overrides_file() {
        with_env_lock(|| {
            clear_config_env();
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos();
            let mut path = std::env::temp_dir();
            path.push(format!("fastapi_config_override_{stamp}.json"));

            let json = r#"{
  "name": "File API",
  "version": "1.0.0",
  "debug": false,
  "max_body_size": 1024,
  "request_timeout_ms": 1000
}"#;
            std::fs::write(&path, json).expect("write temp config");

            std::env::set_var("FASTAPI_NAME", "Env API");
            std::env::set_var("FASTAPI_DEBUG", "1");

            let config = AppConfig::from_env_and_file(&path).expect("env+file config");
            assert_eq!(config.name, "Env API");
            assert!(config.debug);
            assert_eq!(config.version, "1.0.0");
            assert_eq!(config.max_body_size, 1024);

            clear_config_env();
        });
    }

    #[test]
    fn state_container_insert_and_get() {
        #[derive(Debug, PartialEq)]
        struct MyState {
            value: i32,
        }

        let mut container = StateContainer::new();
        container.insert(MyState { value: 42 });

        let state = container.get::<MyState>();
        assert!(state.is_some());
        assert_eq!(state.unwrap().value, 42);
    }

    #[test]
    fn state_container_multiple_types() {
        struct TypeA(i32);
        struct TypeB(String);

        let mut container = StateContainer::new();
        container.insert(TypeA(1));
        container.insert(TypeB("hello".to_string()));

        assert!(container.contains::<TypeA>());
        assert!(container.contains::<TypeB>());
        assert!(!container.contains::<i64>());

        assert_eq!(container.get::<TypeA>().unwrap().0, 1);
        assert_eq!(container.get::<TypeB>().unwrap().0, "hello");
    }

    #[test]
    fn app_builder_with_state() {
        struct DbPool {
            connection_count: usize,
        }

        let app = App::builder()
            .state(DbPool {
                connection_count: 10,
            })
            .get("/", test_handler)
            .build();

        let pool = app.get_state::<DbPool>();
        assert!(pool.is_some());
        assert_eq!(pool.unwrap().connection_count, 10);
    }

    #[test]
    fn app_handles_get_request() {
        let app = App::builder().get("/", test_handler).build();

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn app_returns_404_for_unknown_path() {
        let app = App::builder().get("/", test_handler).build();

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/unknown");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 404);
    }

    #[test]
    fn app_returns_405_for_wrong_method() {
        let app = App::builder().get("/", test_handler).build();

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 405);
    }

    #[test]
    fn app_builder_all_methods() {
        let app = App::builder()
            .get("/get", test_handler)
            .post("/post", test_handler)
            .put("/put", test_handler)
            .delete("/delete", test_handler)
            .patch("/patch", test_handler)
            .build();

        assert_eq!(app.route_count(), 5);
    }

    #[test]
    fn route_entry_debug() {
        let entry = RouteEntry::new(Method::Get, "/test", |ctx, req| {
            Box::pin(test_handler(ctx, req))
        });
        let debug = format!("{:?}", entry);
        assert!(debug.contains("RouteEntry"));
        assert!(debug.contains("Get"));
        assert!(debug.contains("/test"));
    }

    #[test]
    fn app_with_middleware() {
        use crate::middleware::NoopMiddleware;

        let app = App::builder()
            .middleware(NoopMiddleware)
            .middleware(NoopMiddleware)
            .get("/", test_handler)
            .build();

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
    }

    // =========================================================================
    // Exception Handlers Tests
    // =========================================================================

    // Custom error type for testing
    #[derive(Debug)]
    struct TestError {
        message: String,
        code: u32,
    }

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "TestError({}): {}", self.code, self.message)
        }
    }

    impl std::error::Error for TestError {}

    // Another custom error type
    #[derive(Debug)]
    struct AnotherError(String);

    impl std::fmt::Display for AnotherError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "AnotherError: {}", self.0)
        }
    }

    impl std::error::Error for AnotherError {}

    // --- Unit Tests: Handler Registration ---

    #[test]
    fn exception_handlers_new_is_empty() {
        let handlers = ExceptionHandlers::new();
        assert!(handlers.is_empty());
        assert_eq!(handlers.len(), 0);
    }

    #[test]
    fn exception_handlers_register_single() {
        let mut handlers = ExceptionHandlers::new();
        handlers.register::<TestError>(|_ctx, err| {
            Response::with_status(StatusCode::BAD_REQUEST)
                .body(ResponseBody::Bytes(err.message.as_bytes().to_vec()))
        });

        assert!(handlers.has_handler::<TestError>());
        assert!(!handlers.has_handler::<AnotherError>());
        assert_eq!(handlers.len(), 1);
    }

    #[test]
    fn exception_handlers_register_multiple() {
        let mut handlers = ExceptionHandlers::new();
        handlers.register::<TestError>(|_ctx, _err| Response::with_status(StatusCode::BAD_REQUEST));
        handlers.register::<AnotherError>(|_ctx, _err| {
            Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
        });

        assert!(handlers.has_handler::<TestError>());
        assert!(handlers.has_handler::<AnotherError>());
        assert_eq!(handlers.len(), 2);
    }

    #[test]
    fn exception_handlers_builder_pattern() {
        let handlers = ExceptionHandlers::new()
            .handler::<TestError>(|_ctx, _err| Response::with_status(StatusCode::BAD_REQUEST))
            .handler::<AnotherError>(|_ctx, _err| {
                Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
            });

        assert!(handlers.has_handler::<TestError>());
        assert!(handlers.has_handler::<AnotherError>());
        assert_eq!(handlers.len(), 2);
    }

    #[test]
    fn exception_handlers_with_defaults() {
        let handlers = ExceptionHandlers::with_defaults();

        assert!(handlers.has_handler::<crate::HttpError>());
        assert!(handlers.has_handler::<crate::ValidationErrors>());
        assert!(handlers.has_handler::<crate::CancelledError>());
        assert_eq!(handlers.len(), 3);
    }

    #[test]
    fn exception_handlers_merge() {
        let mut handlers1 = ExceptionHandlers::new()
            .handler::<TestError>(|_ctx, _err| Response::with_status(StatusCode::BAD_REQUEST));

        let handlers2 = ExceptionHandlers::new().handler::<AnotherError>(|_ctx, _err| {
            Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
        });

        handlers1.merge(handlers2);

        assert!(handlers1.has_handler::<TestError>());
        assert!(handlers1.has_handler::<AnotherError>());
        assert_eq!(handlers1.len(), 2);
    }

    // --- Unit Tests: Handler Invocation ---

    #[test]
    fn exception_handlers_handle_registered_error() {
        let handlers = ExceptionHandlers::new().handler::<TestError>(|_ctx, err| {
            Response::with_status(StatusCode::BAD_REQUEST)
                .body(ResponseBody::Bytes(err.message.as_bytes().to_vec()))
        });

        let ctx = test_context();
        let err = TestError {
            message: "test error".into(),
            code: 42,
        };

        let response = handlers.handle(&ctx, err);
        assert!(response.is_some());

        let response = response.unwrap();
        assert_eq!(response.status().as_u16(), 400);
    }

    #[test]
    fn exception_handlers_handle_unregistered_error() {
        let handlers = ExceptionHandlers::new()
            .handler::<TestError>(|_ctx, _err| Response::with_status(StatusCode::BAD_REQUEST));

        let ctx = test_context();
        let err = AnotherError("unhandled".into());

        let response = handlers.handle(&ctx, err);
        assert!(response.is_none());
    }

    #[test]
    fn exception_handlers_handle_or_default_registered() {
        let handlers = ExceptionHandlers::new()
            .handler::<TestError>(|_ctx, _err| Response::with_status(StatusCode::BAD_REQUEST));

        let ctx = test_context();
        let err = TestError {
            message: "test".into(),
            code: 1,
        };

        let response = handlers.handle_or_default(&ctx, err);
        assert_eq!(response.status().as_u16(), 400);
    }

    #[test]
    fn exception_handlers_handle_or_default_unregistered() {
        let handlers = ExceptionHandlers::new();

        let ctx = test_context();
        let err = TestError {
            message: "test".into(),
            code: 1,
        };

        let response = handlers.handle_or_default(&ctx, err);
        assert_eq!(response.status().as_u16(), 500);
    }

    #[test]
    fn exception_handlers_error_values_passed_to_handler() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let captured_code = Arc::new(AtomicU32::new(0));
        let captured_code_clone = captured_code.clone();

        let handlers = ExceptionHandlers::new().handler::<TestError>(move |_ctx, err| {
            captured_code_clone.store(err.code, Ordering::SeqCst);
            Response::with_status(StatusCode::BAD_REQUEST)
        });

        let ctx = test_context();
        let err = TestError {
            message: "test".into(),
            code: 12345,
        };

        let _ = handlers.handle(&ctx, err);
        assert_eq!(captured_code.load(Ordering::SeqCst), 12345);
    }

    // --- Integration Tests: Custom Error Type Handling ---

    #[test]
    fn app_builder_exception_handler_single() {
        let app = App::builder()
            .exception_handler::<TestError, _>(|_ctx, err| {
                Response::with_status(StatusCode::BAD_REQUEST)
                    .body(ResponseBody::Bytes(err.message.as_bytes().to_vec()))
            })
            .get("/", test_handler)
            .build();

        assert!(app.exception_handlers().has_handler::<TestError>());
    }

    #[test]
    fn app_builder_exception_handler_multiple() {
        let app = App::builder()
            .exception_handler::<TestError, _>(|_ctx, _err| {
                Response::with_status(StatusCode::BAD_REQUEST)
            })
            .exception_handler::<AnotherError, _>(|_ctx, _err| {
                Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
            })
            .get("/", test_handler)
            .build();

        assert!(app.exception_handlers().has_handler::<TestError>());
        assert!(app.exception_handlers().has_handler::<AnotherError>());
    }

    #[test]
    fn app_builder_with_default_exception_handlers() {
        let app = App::builder()
            .with_default_exception_handlers()
            .get("/", test_handler)
            .build();

        assert!(app.exception_handlers().has_handler::<crate::HttpError>());
        assert!(
            app.exception_handlers()
                .has_handler::<crate::ValidationErrors>()
        );
        assert!(
            app.exception_handlers()
                .has_handler::<crate::CancelledError>()
        );
    }

    #[test]
    fn app_builder_exception_handlers_registry() {
        let handlers = ExceptionHandlers::new()
            .handler::<TestError>(|_ctx, _err| Response::with_status(StatusCode::BAD_REQUEST))
            .handler::<AnotherError>(|_ctx, _err| {
                Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
            });

        let app = App::builder()
            .exception_handlers(handlers)
            .get("/", test_handler)
            .build();

        assert!(app.exception_handlers().has_handler::<TestError>());
        assert!(app.exception_handlers().has_handler::<AnotherError>());
    }

    #[test]
    fn app_handle_error_registered() {
        let app = App::builder()
            .exception_handler::<TestError, _>(|_ctx, _err| {
                Response::with_status(StatusCode::BAD_REQUEST)
            })
            .get("/", test_handler)
            .build();

        let ctx = test_context();
        let err = TestError {
            message: "test".into(),
            code: 1,
        };

        let response = app.handle_error(&ctx, err);
        assert!(response.is_some());
        assert_eq!(response.unwrap().status().as_u16(), 400);
    }

    #[test]
    fn app_handle_error_unregistered() {
        let app = App::builder().get("/", test_handler).build();

        let ctx = test_context();
        let err = TestError {
            message: "test".into(),
            code: 1,
        };

        let response = app.handle_error(&ctx, err);
        assert!(response.is_none());
    }

    #[test]
    fn app_handle_error_or_default() {
        let app = App::builder().get("/", test_handler).build();

        let ctx = test_context();
        let err = TestError {
            message: "test".into(),
            code: 1,
        };

        let response = app.handle_error_or_default(&ctx, err);
        assert_eq!(response.status().as_u16(), 500);
    }

    // --- Integration Tests: Override Default Handler ---

    #[test]
    fn exception_handlers_override_on_register() {
        let handlers = ExceptionHandlers::new()
            .handler::<TestError>(|_ctx, _err| Response::with_status(StatusCode::BAD_REQUEST))
            .handler::<TestError>(|_ctx, _err| {
                Response::with_status(StatusCode::UNPROCESSABLE_ENTITY)
            });

        // Only one handler for TestError
        assert_eq!(handlers.len(), 1);

        let ctx = test_context();
        let err = TestError {
            message: "test".into(),
            code: 1,
        };

        // Should use the second (overriding) handler
        let response = handlers.handle(&ctx, err);
        assert!(response.is_some());
        assert_eq!(response.unwrap().status().as_u16(), 422);
    }

    #[test]
    fn exception_handlers_merge_overrides() {
        let mut handlers1 = ExceptionHandlers::new()
            .handler::<TestError>(|_ctx, _err| Response::with_status(StatusCode::BAD_REQUEST));

        let handlers2 = ExceptionHandlers::new().handler::<TestError>(|_ctx, _err| {
            Response::with_status(StatusCode::UNPROCESSABLE_ENTITY)
        });

        handlers1.merge(handlers2);

        // Only one handler for TestError after merge
        assert_eq!(handlers1.len(), 1);

        let ctx = test_context();
        let err = TestError {
            message: "test".into(),
            code: 1,
        };

        // Merged handlers should override
        let response = handlers1.handle(&ctx, err);
        assert!(response.is_some());
        assert_eq!(response.unwrap().status().as_u16(), 422);
    }

    #[test]
    fn exception_handlers_override_default_http_error() {
        // Start with default handlers
        let mut handlers = ExceptionHandlers::with_defaults();

        // Override HttpError handler
        handlers.register::<crate::HttpError>(|_ctx, err| {
            // Custom handler that adds extra header
            let detail = err.detail.as_deref().unwrap_or("Unknown error");
            Response::with_status(err.status)
                .header("x-custom-error", b"true".to_vec())
                .body(ResponseBody::Bytes(detail.as_bytes().to_vec()))
        });

        // Still has 3 handlers (HttpError, ValidationErrors, CancelledError)
        assert_eq!(handlers.len(), 3);

        let ctx = test_context();
        let err = crate::HttpError::bad_request().with_detail("test error");

        let response = handlers.handle(&ctx, err);
        assert!(response.is_some());

        let response = response.unwrap();
        assert_eq!(response.status().as_u16(), 400);

        // Check custom header was added
        let custom_header = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-custom-error"))
            .map(|(_, v)| v.as_slice());
        assert_eq!(custom_header, Some(b"true".as_slice()));
    }

    #[test]
    fn exception_handlers_override_default_validation_errors() {
        // Start with default handlers
        let mut handlers = ExceptionHandlers::with_defaults();

        // Override ValidationErrors handler
        handlers.register::<crate::ValidationErrors>(|_ctx, errs| {
            // Custom handler that returns 400 instead of 422
            Response::with_status(StatusCode::BAD_REQUEST)
                .header("x-error-count", errs.len().to_string().as_bytes().to_vec())
        });

        let ctx = test_context();
        let mut errs = crate::ValidationErrors::new();
        errs.push(crate::ValidationError::missing(
            crate::error::loc::body_field("name"),
        ));
        errs.push(crate::ValidationError::missing(
            crate::error::loc::body_field("email"),
        ));

        let response = handlers.handle(&ctx, errs);
        assert!(response.is_some());

        let response = response.unwrap();
        // Custom handler returns 400 instead of 422
        assert_eq!(response.status().as_u16(), 400);

        // Check custom header
        let count_header = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-error-count"))
            .map(|(_, v)| v.as_slice());
        assert_eq!(count_header, Some(b"2".as_slice()));
    }

    #[test]
    fn exception_handlers_default_cancelled_error() {
        let handlers = ExceptionHandlers::with_defaults();

        let ctx = test_context();
        let err = crate::CancelledError;

        let response = handlers.handle(&ctx, err);
        assert!(response.is_some());

        let response = response.unwrap();
        // CancelledError should return 499 Client Closed Request
        assert_eq!(response.status().as_u16(), 499);
    }

    #[test]
    fn exception_handlers_override_cancelled_error() {
        let mut handlers = ExceptionHandlers::with_defaults();

        // Override CancelledError handler to return 504 Gateway Timeout
        handlers.register::<crate::CancelledError>(|_ctx, _err| {
            Response::with_status(StatusCode::GATEWAY_TIMEOUT)
                .header("x-cancelled", b"true".to_vec())
        });

        let ctx = test_context();
        let err = crate::CancelledError;

        let response = handlers.handle(&ctx, err);
        assert!(response.is_some());

        let response = response.unwrap();
        // Custom handler returns 504 instead of 499
        assert_eq!(response.status().as_u16(), 504);

        // Check custom header
        let cancelled_header = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-cancelled"))
            .map(|(_, v)| v.as_slice());
        assert_eq!(cancelled_header, Some(b"true".as_slice()));
    }

    #[test]
    fn exception_handlers_debug_format() {
        let handlers = ExceptionHandlers::new()
            .handler::<TestError>(|_ctx, _err| Response::with_status(StatusCode::BAD_REQUEST));

        let debug = format!("{:?}", handlers);
        assert!(debug.contains("ExceptionHandlers"));
        assert!(debug.contains("count"));
        assert!(debug.contains("1"));
        assert!(debug.contains("has_panic_handler"));
    }

    // =========================================================================
    // Panic Handler Tests
    // =========================================================================

    #[test]
    fn panic_handler_default_response() {
        let handlers = ExceptionHandlers::new();
        assert!(!handlers.has_panic_handler());

        let response = handlers.handle_panic(None, "test panic");
        assert_eq!(response.status().as_u16(), 500);
    }

    #[test]
    fn panic_handler_custom_handler() {
        let handlers = ExceptionHandlers::new().panic_handler(|_ctx, msg| {
            Response::with_status(StatusCode::SERVICE_UNAVAILABLE)
                .header("x-panic", msg.as_bytes().to_vec())
        });

        assert!(handlers.has_panic_handler());

        let response = handlers.handle_panic(None, "custom panic");
        assert_eq!(response.status().as_u16(), 503);

        let panic_header = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-panic"))
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert_eq!(panic_header, Some("custom panic".to_string()));
    }

    #[test]
    fn panic_handler_with_context() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let ctx_received = Arc::new(AtomicBool::new(false));
        let ctx_received_clone = ctx_received.clone();

        let handlers = ExceptionHandlers::new().panic_handler(move |ctx, _msg| {
            ctx_received_clone.store(ctx.is_some(), Ordering::SeqCst);
            Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
        });

        let ctx = test_context();
        let _ = handlers.handle_panic(Some(&ctx), "panic with context");

        assert!(ctx_received.load(Ordering::SeqCst));
    }

    #[test]
    fn panic_handler_set_panic_handler() {
        let mut handlers = ExceptionHandlers::new();
        assert!(!handlers.has_panic_handler());

        handlers.set_panic_handler(|_ctx, _msg| Response::with_status(StatusCode::GATEWAY_TIMEOUT));

        assert!(handlers.has_panic_handler());

        let response = handlers.handle_panic(None, "test");
        assert_eq!(response.status().as_u16(), 504);
    }

    #[test]
    fn panic_handler_extract_panic_message_str() {
        // Simulate a panic payload with &str
        let payload: Box<dyn std::any::Any + Send> = Box::new("test panic");
        let msg = ExceptionHandlers::extract_panic_message(&*payload);
        assert_eq!(msg, "test panic");
    }

    #[test]
    fn panic_handler_extract_panic_message_string() {
        // Simulate a panic payload with String
        let payload: Box<dyn std::any::Any + Send> = Box::new("test string panic".to_string());
        let msg = ExceptionHandlers::extract_panic_message(&*payload);
        assert_eq!(msg, "test string panic");
    }

    #[test]
    fn panic_handler_extract_panic_message_unknown() {
        // Simulate a panic payload with unknown type
        let payload: Box<dyn std::any::Any + Send> = Box::new(42i32);
        let msg = ExceptionHandlers::extract_panic_message(&*payload);
        assert_eq!(msg, "unknown panic");
    }

    #[test]
    fn panic_handler_merge_prefers_other() {
        let mut handlers1 = ExceptionHandlers::new()
            .panic_handler(|_ctx, _msg| Response::with_status(StatusCode::BAD_REQUEST));

        let handlers2 = ExceptionHandlers::new()
            .panic_handler(|_ctx, _msg| Response::with_status(StatusCode::SERVICE_UNAVAILABLE));

        handlers1.merge(handlers2);

        let response = handlers1.handle_panic(None, "test");
        // Should use handlers2's panic handler (503) after merge
        assert_eq!(response.status().as_u16(), 503);
    }

    #[test]
    fn panic_handler_merge_keeps_existing_if_other_empty() {
        let mut handlers1 = ExceptionHandlers::new()
            .panic_handler(|_ctx, _msg| Response::with_status(StatusCode::BAD_REQUEST));

        let handlers2 = ExceptionHandlers::new(); // No panic handler

        handlers1.merge(handlers2);

        let response = handlers1.handle_panic(None, "test");
        // Should keep handlers1's panic handler (400)
        assert_eq!(response.status().as_u16(), 400);
    }

    #[test]
    fn panic_handler_default_panic_response() {
        let response = ExceptionHandlers::default_panic_response();
        assert_eq!(response.status().as_u16(), 500);
    }

    #[test]
    fn app_debug_includes_exception_handlers() {
        let app = App::builder()
            .exception_handler::<TestError, _>(|_ctx, _err| {
                Response::with_status(StatusCode::BAD_REQUEST)
            })
            .get("/", test_handler)
            .build();

        let debug = format!("{:?}", app);
        assert!(debug.contains("exception_handlers"));
    }

    #[test]
    fn app_builder_debug_includes_exception_handlers() {
        let builder = App::builder().exception_handler::<TestError, _>(|_ctx, _err| {
            Response::with_status(StatusCode::BAD_REQUEST)
        });

        let debug = format!("{:?}", builder);
        assert!(debug.contains("exception_handlers"));
    }

    // =========================================================================
    // Lifecycle Hooks Tests
    // =========================================================================

    // --- Startup Hooks: Registration ---

    #[test]
    fn app_builder_startup_hook_registration() {
        let builder = App::builder().on_startup(|| Ok(())).on_startup(|| Ok(()));

        assert_eq!(builder.startup_hook_count(), 2);
    }

    #[test]
    fn app_builder_shutdown_hook_registration() {
        let builder = App::builder().on_shutdown(|| {}).on_shutdown(|| {});

        assert_eq!(builder.shutdown_hook_count(), 2);
    }

    #[test]
    fn app_builder_mixed_hooks() {
        let builder = App::builder()
            .on_startup(|| Ok(()))
            .on_shutdown(|| {})
            .on_startup(|| Ok(()))
            .on_shutdown(|| {});

        assert_eq!(builder.startup_hook_count(), 2);
        assert_eq!(builder.shutdown_hook_count(), 2);
    }

    #[test]
    fn app_pending_hooks_count() {
        let app = App::builder()
            .on_startup(|| Ok(()))
            .on_startup(|| Ok(()))
            .on_shutdown(|| {})
            .get("/", test_handler)
            .build();

        assert_eq!(app.pending_startup_hooks(), 2);
        assert_eq!(app.pending_shutdown_hooks(), 1);
    }

    // --- Startup Hooks: Execution Order (FIFO) ---

    #[test]
    fn startup_hooks_run_in_fifo_order() {
        let order = Arc::new(parking_lot::Mutex::new(Vec::new()));

        let order1 = Arc::clone(&order);
        let order2 = Arc::clone(&order);
        let order3 = Arc::clone(&order);

        let app = App::builder()
            .on_startup(move || {
                order1.lock().push(1);
                Ok(())
            })
            .on_startup(move || {
                order2.lock().push(2);
                Ok(())
            })
            .on_startup(move || {
                order3.lock().push(3);
                Ok(())
            })
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());

        // FIFO: 1, 2, 3
        assert_eq!(*order.lock(), vec![1, 2, 3]);

        // Hooks consumed
        assert_eq!(app.pending_startup_hooks(), 0);
    }

    // --- Shutdown Hooks: Execution Order (LIFO) ---

    #[test]
    fn shutdown_hooks_run_in_lifo_order() {
        let order = Arc::new(parking_lot::Mutex::new(Vec::new()));

        let order1 = Arc::clone(&order);
        let order2 = Arc::clone(&order);
        let order3 = Arc::clone(&order);

        let app = App::builder()
            .on_shutdown(move || {
                order1.lock().push(1);
            })
            .on_shutdown(move || {
                order2.lock().push(2);
            })
            .on_shutdown(move || {
                order3.lock().push(3);
            })
            .get("/", test_handler)
            .build();

        futures_executor::block_on(app.run_shutdown_hooks());

        // LIFO: 3, 2, 1
        assert_eq!(*order.lock(), vec![3, 2, 1]);

        // Hooks consumed
        assert_eq!(app.pending_shutdown_hooks(), 0);
    }

    // --- Startup Hooks: Success Outcome ---

    #[test]
    fn startup_hooks_success_outcome() {
        let app = App::builder()
            .on_startup(|| Ok(()))
            .on_startup(|| Ok(()))
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(matches!(outcome, StartupOutcome::Success));
        assert!(outcome.can_proceed());
    }

    // --- Startup Hooks: Fatal Error Aborts ---

    #[test]
    fn startup_hooks_fatal_error_aborts() {
        let app = App::builder()
            .on_startup(|| Ok(()))
            .on_startup(|| Err(StartupHookError::new("database connection failed")))
            .on_startup(|| Ok(())) // Should not run
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(!outcome.can_proceed());

        if let StartupOutcome::Aborted(err) = outcome {
            assert!(err.message.contains("database connection failed"));
            assert!(err.abort);
        } else {
            panic!("Expected Aborted outcome");
        }
    }

    // --- Startup Hooks: Non-Fatal Error Continues ---

    #[test]
    fn startup_hooks_non_fatal_error_continues() {
        let app = App::builder()
            .on_startup(|| Ok(()))
            .on_startup(|| Err(StartupHookError::non_fatal("optional feature unavailable")))
            .on_startup(|| Ok(())) // Should still run
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());

        if let StartupOutcome::PartialSuccess { warnings } = outcome {
            assert_eq!(warnings, 1);
        } else {
            panic!("Expected PartialSuccess outcome");
        }
    }

    // --- Startup Hook Error Types ---

    #[test]
    fn startup_hook_error_builder() {
        let err = StartupHookError::new("test error")
            .with_hook_name("database_init")
            .with_abort(false);

        assert_eq!(err.hook_name.as_deref(), Some("database_init"));
        assert_eq!(err.message, "test error");
        assert!(!err.abort);
    }

    #[test]
    fn startup_hook_error_display() {
        let err = StartupHookError::new("connection failed").with_hook_name("redis_init");

        let display = format!("{}", err);
        assert!(display.contains("redis_init"));
        assert!(display.contains("connection failed"));
    }

    #[test]
    fn startup_hook_error_non_fatal() {
        let err = StartupHookError::non_fatal("optional feature");
        assert!(!err.abort);
    }

    // --- Transfer Shutdown Hooks to Controller ---

    #[test]
    fn transfer_shutdown_hooks_to_controller() {
        let order = Arc::new(parking_lot::Mutex::new(Vec::new()));

        let order1 = Arc::clone(&order);
        let order2 = Arc::clone(&order);

        let app = App::builder()
            .on_shutdown(move || {
                order1.lock().push(1);
            })
            .on_shutdown(move || {
                order2.lock().push(2);
            })
            .get("/", test_handler)
            .build();

        let controller = ShutdownController::new();
        app.transfer_shutdown_hooks(&controller);

        // App hooks consumed
        assert_eq!(app.pending_shutdown_hooks(), 0);

        // Controller has the hooks
        assert_eq!(controller.hook_count(), 2);

        // Run via controller (LIFO order)
        while let Some(hook) = controller.pop_hook() {
            hook.run();
        }

        // LIFO order via controller
        assert_eq!(*order.lock(), vec![2, 1]);
    }

    // --- Debug Format Includes Hooks ---

    #[test]
    fn app_debug_includes_hooks() {
        let app = App::builder()
            .on_startup(|| Ok(()))
            .on_shutdown(|| {})
            .get("/", test_handler)
            .build();

        let debug = format!("{:?}", app);
        assert!(debug.contains("startup_hooks"));
        assert!(debug.contains("shutdown_hooks"));
    }

    #[test]
    fn app_builder_debug_includes_hooks() {
        let builder = App::builder().on_startup(|| Ok(())).on_shutdown(|| {});

        let debug = format!("{:?}", builder);
        assert!(debug.contains("startup_hooks"));
        assert!(debug.contains("shutdown_hooks"));
    }

    // --- Startup Outcome Accessors ---

    #[test]
    fn startup_outcome_success() {
        let outcome = StartupOutcome::Success;
        assert!(outcome.can_proceed());
        assert!(outcome.into_error().is_none());
    }

    #[test]
    fn startup_outcome_partial_success() {
        let outcome = StartupOutcome::PartialSuccess { warnings: 2 };
        assert!(outcome.can_proceed());
        assert!(outcome.into_error().is_none());
    }

    #[test]
    fn startup_outcome_aborted() {
        let err = StartupHookError::new("fatal");
        let outcome = StartupOutcome::Aborted(err);
        assert!(!outcome.can_proceed());

        let err = outcome.into_error();
        assert!(err.is_some());
        assert_eq!(err.unwrap().message, "fatal");
    }

    // --- Multiple Non-Fatal Errors ---

    #[test]
    fn startup_hooks_multiple_non_fatal_errors() {
        let app = App::builder()
            .on_startup(|| Err(StartupHookError::non_fatal("warning 1")))
            .on_startup(|| Ok(()))
            .on_startup(|| Err(StartupHookError::non_fatal("warning 2")))
            .on_startup(|| Err(StartupHookError::non_fatal("warning 3")))
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());

        if let StartupOutcome::PartialSuccess { warnings } = outcome {
            assert_eq!(warnings, 3);
        } else {
            panic!("Expected PartialSuccess");
        }
    }

    // --- Empty Hooks ---

    #[test]
    fn empty_startup_hooks() {
        let app = App::builder().get("/", test_handler).build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(matches!(outcome, StartupOutcome::Success));
    }

    #[test]
    fn empty_shutdown_hooks() {
        let app = App::builder().get("/", test_handler).build();

        // Should not panic with empty hooks
        futures_executor::block_on(app.run_shutdown_hooks());
    }

    // --- Hooks Can Only Run Once ---

    #[test]
    fn startup_hooks_consumed_after_run() {
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let app = App::builder()
            .on_startup(move || {
                counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .get("/", test_handler)
            .build();

        // First run
        futures_executor::block_on(app.run_startup_hooks());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Second run - no hooks left
        futures_executor::block_on(app.run_startup_hooks());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn shutdown_hooks_consumed_after_run() {
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let app = App::builder()
            .on_shutdown(move || {
                counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .get("/", test_handler)
            .build();

        // First run
        futures_executor::block_on(app.run_shutdown_hooks());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Second run - no hooks left
        futures_executor::block_on(app.run_shutdown_hooks());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
