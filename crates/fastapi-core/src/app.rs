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
use std::marker::PhantomData;
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
use crate::routing::{RouteLookup, RouteTable, format_allow_header, method_order};
use crate::shutdown::ShutdownController;
use serde::Deserialize;

// ============================================================================
// Type-Safe State Registry (Compile-Time State Tracking)
// ============================================================================

/// Marker trait for the type-level state registry.
///
/// The state registry is a type-level set represented as nested tuples:
/// - `()` represents the empty set
/// - `(T, S)` represents the set containing T plus all types in S
///
/// This enables compile-time verification that state types are registered
/// before they are used by handlers.
pub trait StateRegistry: Send + Sync + 'static {}

impl StateRegistry for () {}
impl<T: Send + Sync + 'static, S: StateRegistry> StateRegistry for (T, S) {}

/// Marker trait indicating that type T is present in state registry S.
///
/// This trait is automatically implemented for any type T that appears
/// in the nested tuple structure of S.
///
/// # Example
///
/// ```ignore
/// // (DbPool, (Config, ())) contains both DbPool and Config
/// fn requires_db<S: HasState<DbPool>>() {}
/// fn requires_config<S: HasState<Config>>() {}
///
/// // Both work with (DbPool, (Config, ()))
/// type MyState = (DbPool, (Config, ()));
/// requires_db::<MyState>();   // compiles
/// requires_config::<MyState>(); // compiles
/// ```
pub trait HasState<T>: StateRegistry {}

// T is in (T, S) - direct match (at head of tuple)
impl<T: Send + Sync + 'static, S: StateRegistry> HasState<T> for (T, S) {}

// Note: A recursive impl like "T is in (U, S) if T is in S" would conflict
// with the direct impl when T == U. Without negative bounds or specialization,
// we can only support checking for types at specific positions in the tuple.
// For multiple state types, users can define impls manually for their specific
// tuple structure, or use a different state organization.

/// Trait for types that require specific state to be registered.
///
/// This is implemented by extractors like `State<T>` to declare their
/// state dependencies at the type level.
///
/// # Example
///
/// ```ignore
/// // State<DbPool> requires DbPool to be in the registry
/// impl<S: HasState<DbPool>> RequiresState<S> for State<DbPool> {}
/// ```
pub trait RequiresState<S: StateRegistry> {}

// All types that don't need state trivially satisfy RequiresState
impl<S: StateRegistry> RequiresState<S> for () {}

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

// ============================================================================
// Lifespan Context Manager
// ============================================================================

/// Error during lifespan startup.
///
/// This error is returned when the lifespan function fails during the startup phase.
/// It will abort the application startup, preventing the server from accepting connections.
#[derive(Debug)]
pub struct LifespanError {
    /// Description of what went wrong.
    pub message: String,
    /// Optional underlying error source.
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl LifespanError {
    /// Creates a new lifespan error with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Creates a lifespan error wrapping another error.
    pub fn with_source<E: std::error::Error + Send + Sync + 'static>(
        message: impl Into<String>,
        source: E,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl std::fmt::Display for LifespanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lifespan error: {}", self.message)?;
        if let Some(ref source) = self.source {
            write!(f, " (caused by: {})", source)?;
        }
        Ok(())
    }
}

impl std::error::Error for LifespanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<LifespanError> for StartupHookError {
    fn from(err: LifespanError) -> Self {
        StartupHookError::new(err.to_string())
    }
}

/// Scope returned by a lifespan function containing state and cleanup logic.
///
/// The lifespan pattern allows sharing state between startup and shutdown phases,
/// which is particularly useful for resources like database connections that need
/// coordinated initialization and cleanup.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::app::{App, LifespanScope, LifespanError};
///
/// struct DatabasePool { /* ... */ }
///
/// impl DatabasePool {
///     async fn connect(url: &str) -> Result<Self, Error> { /* ... */ }
///     async fn close(&self) { /* ... */ }
/// }
///
/// let app = App::builder()
///     .lifespan(|| async {
///         // Startup: connect to database
///         let pool = DatabasePool::connect("postgres://localhost/mydb")
///             .await
///             .map_err(|e| LifespanError::with_source("failed to connect to database", e))?;
///
///         // Clone for the cleanup closure
///         let pool_for_cleanup = pool.clone();
///
///         // Return state + cleanup
///         Ok(LifespanScope::new(pool)
///             .on_shutdown(async move {
///                 pool_for_cleanup.close().await;
///             }))
///     })
///     .build();
/// ```
pub struct LifespanScope<T: Send + Sync + 'static> {
    /// State produced by the lifespan function.
    ///
    /// This state is automatically added to the application's state container
    /// and can be accessed by handlers via the `State<T>` extractor.
    pub state: T,

    /// Optional cleanup future to run during shutdown.
    cleanup: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl<T: Send + Sync + 'static> LifespanScope<T> {
    /// Creates a new lifespan scope with the given state.
    ///
    /// The state will be added to the application's state container after
    /// successful startup, accessible via the `State<T>` extractor.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let scope = LifespanScope::new(MyState { value: 42 });
    /// ```
    pub fn new(state: T) -> Self {
        Self {
            state,
            cleanup: None,
        }
    }

    /// Sets the cleanup future to run during application shutdown.
    ///
    /// The cleanup runs in reverse order relative to other lifespan/shutdown hooks,
    /// after all in-flight requests have completed or been cancelled.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let scope = LifespanScope::new(pool.clone())
    ///     .on_shutdown(async move {
    ///         pool.close().await;
    ///         println!("Database pool closed");
    ///     });
    /// ```
    #[must_use]
    pub fn on_shutdown<F>(mut self, cleanup: F) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.cleanup = Some(Box::pin(cleanup));
        self
    }

    /// Takes the cleanup future, leaving `None` in its place.
    ///
    /// This is used internally to transfer the cleanup to the shutdown system.
    pub fn take_cleanup(&mut self) -> Option<Pin<Box<dyn Future<Output = ()> + Send>>> {
        self.cleanup.take()
    }
}

impl<T: Send + Sync + std::fmt::Debug + 'static> std::fmt::Debug for LifespanScope<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifespanScope")
            .field("state", &self.state)
            .field("has_cleanup", &self.cleanup.is_some())
            .finish()
    }
}

/// Boxed lifespan function type.
///
/// The lifespan function runs during startup and returns state (as Any) along with
/// an optional cleanup future for shutdown. The state is then inserted into the container.
pub type BoxLifespanFn = Box<
    dyn FnOnce() -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            (
                                Box<dyn std::any::Any + Send + Sync>,
                                Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
                            ),
                            LifespanError,
                        >,
                    > + Send,
            >,
        > + Send,
>;

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

    /// Inserts a boxed Any value into the state container.
    ///
    /// This is used by the lifespan system to insert type-erased state.
    /// The TypeId is obtained from the actual type inside the box.
    pub fn insert_any(&mut self, value: Box<dyn Any + Send + Sync>) {
        let type_id = (*value).type_id();
        self.state.insert(type_id, Arc::from(value));
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
    /// Root path prefix for apps behind a reverse proxy.
    ///
    /// When the application is served behind a reverse proxy at a sub-path,
    /// this should be set to that sub-path. For example, if the app is
    /// proxied at `/api/v1`, set `root_path = "/api/v1"`.
    ///
    /// This affects:
    /// - URL generation via `url_for()`
    /// - OpenAPI servers list (if `root_path_in_servers` is true)
    pub root_path: String,
    /// Whether to include the root_path in OpenAPI servers list.
    ///
    /// When true and `root_path` is set, a server entry with the root_path
    /// will be added to the OpenAPI specification's servers array.
    pub root_path_in_servers: bool,
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
    root_path: Option<String>,
    root_path_in_servers: Option<bool>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: String::from("fastapi_rust"),
            version: String::from("0.1.0"),
            debug: false,
            max_body_size: 1024 * 1024, // 1MB
            request_timeout_ms: 30_000, // 30 seconds
            root_path: String::new(),
            root_path_in_servers: true,
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
    const ENV_ROOT_PATH: &'static str = "ROOT_PATH";
    const ENV_ROOT_PATH_IN_SERVERS: &'static str = "ROOT_PATH_IN_SERVERS";

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

    /// Sets the root path for apps behind a reverse proxy.
    ///
    /// When the application is served behind a reverse proxy at a sub-path,
    /// set this to that sub-path. For example, if the app is proxied at
    /// `/api/v1`, set `root_path("/api/v1")`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = AppConfig::new().root_path("/api/v1");
    /// ```
    #[must_use]
    pub fn root_path(mut self, path: impl Into<String>) -> Self {
        let mut path = path.into();
        // Normalize: remove trailing slashes
        while path.ends_with('/') && path.len() > 1 {
            path.pop();
        }
        self.root_path = path;
        self
    }

    /// Sets whether to include root_path in OpenAPI servers list.
    ///
    /// When true and `root_path` is set, a server entry with the root_path
    /// will be added to the OpenAPI specification's servers array.
    #[must_use]
    pub fn root_path_in_servers(mut self, include: bool) -> Self {
        self.root_path_in_servers = include;
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
    /// - `FASTAPI_ROOT_PATH` (path prefix for reverse proxy)
    /// - `FASTAPI_ROOT_PATH_IN_SERVERS` (true/false/1/0/yes/no/on/off)
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

    /// Returns the root_path as an OpenAPI server entry if configured.
    ///
    /// Returns `Some((url, description))` if:
    /// - `root_path` is non-empty
    /// - `root_path_in_servers` is true
    ///
    /// Returns `None` otherwise.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = AppConfig::new().root_path("/api/v1");
    /// if let Some((url, description)) = config.openapi_server() {
    ///     builder = builder.server(url, description);
    /// }
    /// ```
    #[must_use]
    pub fn openapi_server(&self) -> Option<(String, Option<String>)> {
        if !self.root_path.is_empty() && self.root_path_in_servers {
            Some((
                self.root_path.clone(),
                Some("Application root path".to_string()),
            ))
        } else {
            None
        }
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
        if let Some(root_path) = file.root_path {
            self.root_path = root_path;
        }
        if let Some(root_path_in_servers) = file.root_path_in_servers {
            self.root_path_in_servers = root_path_in_servers;
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
        let root_path_key = env_key(prefix, Self::ENV_ROOT_PATH);
        let root_path_in_servers_key = env_key(prefix, Self::ENV_ROOT_PATH_IN_SERVERS);

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
        if let Some(value) = fetch(&root_path_key)? {
            self.root_path = value;
        }
        if let Some(value) = fetch(&root_path_in_servers_key)? {
            self.root_path_in_servers = parse_bool(&root_path_in_servers_key, &value)?;
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
    value
        .parse::<usize>()
        .map_err(|_| ConfigError::InvalidEnvVar {
            key: key.to_string(),
            value: value.to_string(),
            expected: "usize".to_string(),
        })
}

fn parse_u64(key: &str, value: &str) -> Result<u64, ConfigError> {
    value
        .parse::<u64>()
        .map_err(|_| ConfigError::InvalidEnvVar {
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
///
/// # Type-Safe State
///
/// The `AppBuilder` uses a type-state pattern to track registered state types
/// at compile time. The generic parameter `S` represents the type-level set of
/// registered state types.
///
/// When you call `.with_state::<T>(value)`, the builder's type changes from
/// `AppBuilder<S>` to `AppBuilder<(T, S)>`, recording that `T` is now available.
///
/// Handlers that use `State<T>` extractors can optionally be constrained to
/// require `S: HasState<T>`, ensuring the state is registered at compile time.
///
/// ```ignore
/// // Type changes: AppBuilder<()> -> AppBuilder<(DbPool, ())> -> AppBuilder<(Config, (DbPool, ()))>
/// let app = App::builder()
///     .with_state(DbPool::new())  // Now has DbPool
///     .with_state(Config::default())  // Now has DbPool + Config
///     .build();
/// ```
pub struct AppBuilder<S: StateRegistry = ()> {
    config: AppConfig,
    routes: Vec<RouteEntry>,
    middleware: Vec<Arc<dyn Middleware>>,
    state: StateContainer,
    dependency_overrides: Arc<DependencyOverrides>,
    exception_handlers: ExceptionHandlers,
    startup_hooks: Vec<StartupHook>,
    shutdown_hooks: Vec<Box<dyn FnOnce() + Send>>,
    async_shutdown_hooks: Vec<Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>>,
    /// Optional lifespan function for async startup/shutdown context management.
    lifespan: Option<BoxLifespanFn>,
    /// Mounted sub-applications at specific path prefixes.
    mounted_apps: Vec<MountedApp>,
    _state_marker: PhantomData<S>,
}

impl Default for AppBuilder<()> {
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
            lifespan: None,
            mounted_apps: Vec::new(),
            _state_marker: PhantomData,
        }
    }
}

impl AppBuilder<()> {
    /// Creates a new application builder with no registered state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<S: StateRegistry> AppBuilder<S> {
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

    /// Mounts a sub-application at a path prefix.
    ///
    /// Mounted applications are completely independent from the parent:
    /// - They have their own middleware stack (parent middleware does NOT apply)
    /// - They have their own state and configuration
    /// - Their OpenAPI schemas are NOT merged with the parent
    /// - Request paths have the prefix stripped before being passed to the sub-app
    ///
    /// This differs from [`include_router`](Self::include_router), which merges
    /// routes into the parent app and applies parent middleware.
    ///
    /// # Use Cases
    ///
    /// - Mount admin panels at `/admin`
    /// - Mount Swagger UI at `/docs`
    /// - Mount static file servers
    /// - Integrate legacy or third-party apps
    ///
    /// # Path Stripping Behavior
    ///
    /// When a request arrives at `/admin/users`, and an app is mounted at `/admin`,
    /// the sub-app receives the request with path `/users`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::app::App;
    ///
    /// // Create an admin sub-application
    /// let admin_app = App::builder()
    ///     .get("/users", admin_list_users)
    ///     .get("/settings", admin_settings)
    ///     .middleware(AdminAuthMiddleware::new())
    ///     .build();
    ///
    /// // Mount it at /admin
    /// let main_app = App::builder()
    ///     .get("/", home_page)
    ///     .mount("/admin", admin_app)
    ///     .build();
    ///
    /// // Now:
    /// // - GET /           -> home_page
    /// // - GET /admin/users -> admin_list_users (with AdminAuthMiddleware)
    /// // - GET /admin/settings -> admin_settings (with AdminAuthMiddleware)
    /// ```
    #[must_use]
    pub fn mount(mut self, prefix: impl Into<String>, app: App) -> Self {
        self.mounted_apps.push(MountedApp::new(prefix, app));
        self
    }

    /// Returns the number of mounted sub-applications.
    #[must_use]
    pub fn mounted_app_count(&self) -> usize {
        self.mounted_apps.len()
    }

    /// Adds shared state to the application (legacy method).
    ///
    /// State can be accessed by handlers through the `State<T>` extractor.
    ///
    /// **Note:** This method is deprecated in favor of [`with_state`](Self::with_state),
    /// which provides compile-time verification that state types are registered.
    #[must_use]
    #[deprecated(
        since = "0.2.0",
        note = "Use `with_state` for compile-time state type verification"
    )]
    pub fn state<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.state.insert(value);
        self
    }

    /// Adds typed state to the application with compile-time registration.
    ///
    /// This method registers state using a type-state pattern, which enables
    /// compile-time verification that state types are properly registered before
    /// they are used by handlers.
    ///
    /// The return type changes from `AppBuilder<S>` to `AppBuilder<(T, S)>`,
    /// recording that type `T` is now available in the state registry.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::app::App;
    ///
    /// struct DbPool { /* ... */ }
    /// struct Config { api_key: String }
    ///
    /// // Type evolves: () -> (DbPool, ()) -> (Config, (DbPool, ()))
    /// let app = App::builder()
    ///     .with_state(DbPool::new())    // Now has DbPool
    ///     .with_state(Config::default()) // Now has DbPool + Config
    ///     .build();
    /// ```
    ///
    /// # Compile-Time Safety
    ///
    /// When used with the `RequiresState` trait, handlers can declare their
    /// state dependencies and the compiler will verify they are met:
    ///
    /// ```ignore
    /// // This handler requires DbPool to be registered
    /// fn handler_requiring_db<S: HasState<DbPool>>(app: AppBuilder<S>) { /* ... */ }
    /// ```
    #[must_use]
    pub fn with_state<T: Send + Sync + 'static>(mut self, value: T) -> AppBuilder<(T, S)> {
        self.state.insert(value);
        AppBuilder {
            config: self.config,
            routes: self.routes,
            middleware: self.middleware,
            state: self.state,
            dependency_overrides: self.dependency_overrides,
            exception_handlers: self.exception_handlers,
            startup_hooks: self.startup_hooks,
            shutdown_hooks: self.shutdown_hooks,
            async_shutdown_hooks: self.async_shutdown_hooks,
            lifespan: self.lifespan,
            mounted_apps: self.mounted_apps,
            _state_marker: PhantomData,
        }
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

    /// Registers a lifespan context manager for async startup/shutdown.
    ///
    /// The lifespan pattern is preferred over separate `on_startup`/`on_shutdown` hooks
    /// because it allows sharing state between the startup and shutdown phases. This is
    /// especially useful for resources like database connections, HTTP clients, or
    /// background task managers.
    ///
    /// The lifespan function runs during application startup. It should:
    /// 1. Initialize resources (connect to database, start background tasks, etc.)
    /// 2. Return a `LifespanScope` containing:
    ///    - State to be added to the application (accessible via `State<T>` extractor)
    ///    - An optional cleanup closure to run during shutdown
    ///
    /// If the lifespan function returns an error, application startup is aborted.
    ///
    /// **Note:** When a lifespan is provided, it runs *before* any `on_startup` hooks.
    /// The lifespan cleanup runs *after* all `on_shutdown` hooks during shutdown.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::app::{App, LifespanScope, LifespanError};
    ///
    /// #[derive(Clone)]
    /// struct DatabasePool { /* ... */ }
    ///
    /// impl DatabasePool {
    ///     async fn connect(url: &str) -> Result<Self, Error> { /* ... */ }
    ///     async fn close(&self) { /* ... */ }
    /// }
    ///
    /// let app = App::builder()
    ///     .lifespan(|| async {
    ///         // Startup: connect to database
    ///         println!("Connecting to database...");
    ///         let pool = DatabasePool::connect("postgres://localhost/mydb")
    ///             .await
    ///             .map_err(|e| LifespanError::with_source("database connection failed", e))?;
    ///
    ///         // Clone for use in cleanup
    ///         let pool_for_cleanup = pool.clone();
    ///
    ///         // Return state and cleanup
    ///         Ok(LifespanScope::new(pool)
    ///             .on_shutdown(async move {
    ///                 println!("Closing database connections...");
    ///                 pool_for_cleanup.close().await;
    ///             }))
    ///     })
    ///     .get("/users", get_users)  // Handler can use State<DatabasePool>
    ///     .build();
    /// ```
    ///
    /// # Multiple State Types
    ///
    /// To provide multiple state types from a single lifespan, use a tuple or
    /// define a struct containing all your state:
    ///
    /// ```ignore
    /// #[derive(Clone)]
    /// struct AppState {
    ///     db: DatabasePool,
    ///     cache: RedisClient,
    ///     config: AppConfig,
    /// }
    ///
    /// let app = App::builder()
    ///     .lifespan(|| async {
    ///         let db = DatabasePool::connect("...").await?;
    ///         let cache = RedisClient::connect("...").await?;
    ///         let config = load_config().await?;
    ///
    ///         let state = AppState { db, cache, config };
    ///         let state_for_cleanup = state.clone();
    ///
    ///         Ok(LifespanScope::new(state)
    ///             .on_shutdown(async move {
    ///                 state_for_cleanup.db.close().await;
    ///                 state_for_cleanup.cache.close().await;
    ///             }))
    ///     })
    ///     .build();
    /// ```
    #[must_use]
    pub fn lifespan<F, Fut, T>(mut self, lifespan_fn: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<LifespanScope<T>, LifespanError>> + Send + 'static,
        T: Send + Sync + 'static,
    {
        self.lifespan = Some(Box::new(move || {
            Box::pin(async move {
                // Run the lifespan function
                let mut scope = lifespan_fn().await?;

                // Extract cleanup first (before moving state)
                let cleanup = scope.take_cleanup();

                // Extract the state as a boxed Any
                let state: Box<dyn std::any::Any + Send + Sync> = Box::new(scope.state);

                // Return both the state and the cleanup future
                Ok((state, cleanup))
            })
        }));
        self
    }

    /// Returns true if a lifespan function has been registered.
    #[must_use]
    pub fn has_lifespan(&self) -> bool {
        self.lifespan.is_some()
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
            state: Arc::new(parking_lot::RwLock::new(self.state)),
            dependency_overrides: Arc::clone(&self.dependency_overrides),
            exception_handlers: Arc::new(self.exception_handlers),
            startup_hooks: parking_lot::Mutex::new(self.startup_hooks),
            shutdown_hooks: parking_lot::Mutex::new(self.shutdown_hooks),
            async_shutdown_hooks: parking_lot::Mutex::new(self.async_shutdown_hooks),
            lifespan: parking_lot::Mutex::new(self.lifespan),
            lifespan_cleanup: parking_lot::Mutex::new(None),
            mounted_apps: self.mounted_apps,
        }
    }
}

impl<S: StateRegistry> std::fmt::Debug for AppBuilder<S> {
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
            .field("mounted_apps", &self.mounted_apps.len())
            .finish()
    }
}

/// A mounted sub-application with its path prefix.
///
/// Mounted applications are completely independent from the parent app:
/// - They have their own middleware stack
/// - They have their own state and configuration
/// - Their OpenAPI schemas are NOT merged with the parent
/// - Request paths have the prefix stripped before being passed to the sub-app
///
/// This differs from `include_router`, which merges routes into the parent app.
#[derive(Debug)]
pub struct MountedApp {
    /// Path prefix at which this app is mounted (e.g., "/admin", "/docs").
    prefix: String,
    /// The mounted sub-application.
    app: Arc<App>,
}

impl MountedApp {
    /// Create a new mounted app at the given prefix.
    ///
    /// The prefix should start with '/' and not end with '/'.
    #[must_use]
    pub fn new(prefix: impl Into<String>, app: App) -> Self {
        let mut prefix = prefix.into();
        // Normalize prefix: ensure starts with '/', doesn't end with '/'
        if !prefix.starts_with('/') {
            prefix = format!("/{}", prefix);
        }
        while prefix.ends_with('/') && prefix.len() > 1 {
            prefix.pop();
        }
        Self {
            prefix,
            app: Arc::new(app),
        }
    }

    /// Returns the mount prefix.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Returns a reference to the mounted app.
    #[must_use]
    pub fn app(&self) -> &App {
        &self.app
    }

    /// Check if a request path matches this mount prefix.
    ///
    /// Returns the remaining path after the prefix is stripped, or None if no match.
    #[must_use]
    pub fn match_prefix<'a>(&self, path: &'a str) -> Option<&'a str> {
        if path == self.prefix {
            // Exact match - sub-app should receive "/"
            Some("/")
        } else if path.starts_with(&self.prefix) {
            let remaining = &path[self.prefix.len()..];
            // Must be followed by '/' or end of string
            if remaining.starts_with('/') {
                Some(remaining)
            } else {
                None
            }
        } else {
            None
        }
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
    /// State container with interior mutability to support lifespan-injected state.
    state: Arc<parking_lot::RwLock<StateContainer>>,
    dependency_overrides: Arc<DependencyOverrides>,
    exception_handlers: Arc<ExceptionHandlers>,
    startup_hooks: parking_lot::Mutex<Vec<StartupHook>>,
    shutdown_hooks: parking_lot::Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    async_shutdown_hooks: parking_lot::Mutex<
        Vec<Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>>,
    >,
    /// Pending lifespan function to run during startup.
    lifespan: parking_lot::Mutex<Option<BoxLifespanFn>>,
    /// Cleanup future from lifespan function (runs during shutdown).
    lifespan_cleanup: parking_lot::Mutex<Option<Pin<Box<dyn Future<Output = ()> + Send>>>>,
    /// Mounted sub-applications.
    mounted_apps: Vec<MountedApp>,
}

impl App {
    /// Creates a new application builder.
    #[must_use]
    pub fn builder() -> AppBuilder<()> {
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

    /// Returns the shared state container (protected by RwLock for lifespan mutation).
    #[must_use]
    pub fn state(&self) -> &Arc<parking_lot::RwLock<StateContainer>> {
        &self.state
    }

    /// Gets a reference to shared state of type T.
    pub fn get_state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.state.read().get::<T>()
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

    /// Returns the mounted sub-applications.
    #[must_use]
    pub fn mounted_apps(&self) -> &[MountedApp] {
        &self.mounted_apps
    }

    /// Handles an incoming request.
    ///
    /// This first checks mounted sub-applications (in registration order),
    /// then matches against registered routes, runs middleware, and returns
    /// the response. Path parameters are extracted and stored in request
    /// extensions for use by the `Path` extractor.
    ///
    /// # Mounted App Handling
    ///
    /// When a request path matches a mounted app's prefix, the request is
    /// forwarded to that app with the prefix stripped from the path. The
    /// mounted app's middleware runs instead of the parent's middleware.
    ///
    /// # Special Parameter Injection
    ///
    /// - **ResponseMutations**: Headers and cookies set by handlers are automatically
    ///   applied to the final response.
    /// - **BackgroundTasks**: Tasks are stored in request extensions via
    ///   `BackgroundTasksInner`. The HTTP server should retrieve and execute these
    ///   after sending the response using `take_background_tasks()`.
    pub async fn handle(&self, ctx: &RequestContext, req: &mut Request) -> Response {
        // First, check mounted sub-applications
        for mounted in &self.mounted_apps {
            if let Some(stripped_path) = mounted.match_prefix(req.path()) {
                // Clone the request with the stripped path for the sub-app
                let original_path = req.path().to_string();
                req.set_path(stripped_path.to_string());

                // Forward to the mounted app (uses its own middleware stack)
                // Use Box::pin for the recursive call to avoid infinite future size
                let response = Box::pin(mounted.app.handle(ctx, req)).await;

                // Restore original path (in case of error handling or logging)
                req.set_path(original_path);

                return response;
            }
        }

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
                let response = if let Some(mutations) =
                    req.get_extension::<crate::extract::ResponseMutations>()
                {
                    mutations.clone().apply(response)
                } else {
                    response
                };

                // Handle HEAD requests: strip body but preserve Content-Length
                // HEAD should return same headers as GET but with no body
                if req.method() == Method::Head {
                    strip_body_for_head(response)
                } else {
                    response
                }
            }
            RouteLookup::MethodNotAllowed { mut allowed } => {
                // Auto-handle OPTIONS requests: return 204 No Content with Allow header
                // listing all methods available for this path (including OPTIONS)
                if req.method() == Method::Options {
                    // Add OPTIONS to allowed methods if not already present
                    if !allowed.contains(&Method::Options) {
                        allowed.push(Method::Options);
                        // Re-sort to maintain consistent ordering
                        allowed.sort_by_key(|m| method_order(*m));
                    }
                    return Response::with_status(StatusCode::NO_CONTENT)
                        .header("Allow", format_allow_header(&allowed).into_bytes());
                }
                Response::with_status(StatusCode::METHOD_NOT_ALLOWED)
                    .header("Allow", format_allow_header(&allowed).into_bytes())
            }
            RouteLookup::NotFound => Response::with_status(StatusCode::NOT_FOUND),
            RouteLookup::Redirect { target } => {
                // 308 Permanent Redirect preserves the request method
                Response::with_status(StatusCode::PERMANENT_REDIRECT)
                    .header("Location", target.into_bytes())
            }
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
        let mut warnings = 0;

        // Run lifespan function first (if registered)
        let lifespan_fn = self.lifespan.lock().take();
        if let Some(lifespan) = lifespan_fn {
            // Run the lifespan function (returns state + cleanup)
            let result = lifespan().await;

            match result {
                Ok((state, cleanup)) => {
                    // Insert the state into the container
                    self.state.write().insert_any(state);
                    // Store cleanup future for shutdown
                    *self.lifespan_cleanup.lock() = cleanup;
                }
                Err(e) => {
                    // Convert LifespanError to StartupHookError and abort
                    return StartupOutcome::Aborted(e.into());
                }
            }
        }

        // Run regular startup hooks
        let hooks: Vec<StartupHook> = std::mem::take(&mut *self.startup_hooks.lock());

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

        // Run lifespan cleanup last (mirrors startup order)
        let lifespan_cleanup = self.lifespan_cleanup.lock().take();
        if let Some(cleanup) = lifespan_cleanup {
            cleanup.await;
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

    // =========================================================================
    // Testing Support
    // =========================================================================

    /// Creates a [`TestClient`] for this application.
    ///
    /// The test client provides in-process testing without network overhead,
    /// simulating HTTP requests against the full application stack including
    /// middleware, routing, and handlers.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::{App, Request, Response, RequestContext, Method};
    ///
    /// async fn hello(ctx: &RequestContext, req: &mut Request) -> Response {
    ///     Response::ok().body_text("Hello!")
    /// }
    ///
    /// let app = App::builder()
    ///     .route("/hello", Method::Get, hello)
    ///     .build();
    ///
    /// // Create test client from app
    /// let client = app.test_client();
    ///
    /// // Make requests
    /// let response = client.get("/hello").send();
    /// assert_eq!(response.status().as_u16(), 200);
    /// assert_eq!(response.text(), "Hello!");
    /// ```
    #[must_use]
    pub fn test_client(self: Arc<Self>) -> crate::testing::TestClient<Arc<App>> {
        crate::testing::TestClient::new(self)
    }

    /// Creates a [`TestClient`] with a specific seed for deterministic testing.
    ///
    /// Using the same seed produces reproducible behavior for tests involving
    /// concurrent operations or random elements.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let app = Arc::new(App::builder().build());
    /// let client = app.test_client_with_seed(42);
    /// // Tests will be deterministic with this seed
    /// ```
    #[must_use]
    pub fn test_client_with_seed(
        self: Arc<Self>,
        seed: u64,
    ) -> crate::testing::TestClient<Arc<App>> {
        crate::testing::TestClient::with_seed(self, seed)
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

impl Handler for Arc<App> {
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
        Box::pin(async move {
            let _ = ctx.checkpoint();
            handler(ctx, req).await
        })
    }
}

/// Strip the body from a response for HEAD requests.
///
/// Per HTTP spec (RFC 7231 Section 4.3.2), HEAD responses must have the
/// same headers as the corresponding GET response, including Content-Length
/// if applicable, but with an empty message body.
///
/// This function:
/// - Preserves Content-Length header if already present
/// - Adds Content-Length for known-length bodies (Bytes) if not present
/// - Does not add Content-Length for streaming bodies (length unknown)
/// - Strips the body, returning an empty response
fn strip_body_for_head(response: Response) -> Response {
    use crate::response::ResponseBody;

    // Check if Content-Length already exists
    let has_content_length = response
        .headers()
        .iter()
        .any(|(name, _): &(String, Vec<u8>)| name.eq_ignore_ascii_case("content-length"));

    // Decompose the response
    let (status, headers, body) = response.into_parts();

    // Determine if we should add Content-Length
    // Only add if not present and body has known length (Bytes, not Stream)
    let content_length_to_add: Option<usize> = if has_content_length {
        None
    } else {
        match &body {
            ResponseBody::Bytes(b) => Some(b.len()),
            ResponseBody::Empty => Some(0),
            ResponseBody::Stream(_) => None, // Unknown length, can't add
        }
    };

    // Build new response with empty body but preserved headers
    let mut new_response = Response::with_status(status)
        .body(ResponseBody::Empty)
        .rebuild_with_headers(headers);

    if let Some(len) = content_length_to_add {
        new_response = new_response.header("content-length", len.to_string().as_bytes().to_vec());
    }

    new_response
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::response::ResponseBody;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

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
        let mut env = HashMap::new();
        env.insert("FASTAPI_NAME".to_string(), "Env API".to_string());
        env.insert("FASTAPI_VERSION".to_string(), "9.9.9".to_string());
        env.insert("FASTAPI_DEBUG".to_string(), "true".to_string());
        env.insert("FASTAPI_MAX_BODY_SIZE".to_string(), "4096".to_string());
        env.insert(
            "FASTAPI_REQUEST_TIMEOUT_MS".to_string(),
            "15000".to_string(),
        );

        let mut config = AppConfig::default();
        config
            .apply_env_with(AppConfig::DEFAULT_ENV_PREFIX, |key| {
                Ok(env.get(key).cloned())
            })
            .expect("env config");
        config.validate().expect("env config");

        assert_eq!(config.name, "Env API");
        assert_eq!(config.version, "9.9.9");
        assert!(config.debug);
        assert_eq!(config.max_body_size, 4096);
        assert_eq!(config.request_timeout_ms, 15_000);
    }

    #[test]
    fn app_config_from_env_invalid_value() {
        let mut env = HashMap::new();
        env.insert(
            "FASTAPI_MAX_BODY_SIZE".to_string(),
            "not-a-number".to_string(),
        );

        let mut config = AppConfig::default();
        let err = config
            .apply_env_with(AppConfig::DEFAULT_ENV_PREFIX, |key| {
                Ok(env.get(key).cloned())
            })
            .expect_err("invalid env should error");
        match err {
            ConfigError::InvalidEnvVar { key, .. } => {
                assert_eq!(key, "FASTAPI_MAX_BODY_SIZE");
            }
            _ => panic!("expected invalid env var error"),
        }
    }

    #[test]
    fn app_config_validation_rejects_empty_name() {
        let config = AppConfig {
            name: String::new(),
            ..Default::default()
        };
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

        let mut env = HashMap::new();
        env.insert("FASTAPI_NAME".to_string(), "Env API".to_string());
        env.insert("FASTAPI_DEBUG".to_string(), "1".to_string());

        let mut config = AppConfig::from_file(&path).expect("file config");
        config
            .apply_env_with(AppConfig::DEFAULT_ENV_PREFIX, |key| {
                Ok(env.get(key).cloned())
            })
            .expect("env+file config");
        config.validate().expect("env+file config");

        assert_eq!(config.name, "Env API");
        assert!(config.debug);
        assert_eq!(config.version, "1.0.0");
        assert_eq!(config.max_body_size, 1024);
    }

    #[test]
    fn app_config_default_root_path() {
        let config = AppConfig::default();
        assert!(config.root_path.is_empty());
        assert!(config.root_path_in_servers);
    }

    #[test]
    fn app_config_builder_root_path() {
        let config = AppConfig::new()
            .root_path("/api/v1")
            .root_path_in_servers(false);

        assert_eq!(config.root_path, "/api/v1");
        assert!(!config.root_path_in_servers);
    }

    #[test]
    fn app_config_from_env_root_path() {
        let mut env = HashMap::new();
        env.insert("FASTAPI_ROOT_PATH".to_string(), "/proxy".to_string());
        env.insert(
            "FASTAPI_ROOT_PATH_IN_SERVERS".to_string(),
            "false".to_string(),
        );

        let mut config = AppConfig::default();
        config
            .apply_env_with(AppConfig::DEFAULT_ENV_PREFIX, |key| {
                Ok(env.get(key).cloned())
            })
            .expect("env config");

        assert_eq!(config.root_path, "/proxy");
        assert!(!config.root_path_in_servers);
    }

    #[test]
    fn app_config_from_env_root_path_in_servers_truthy() {
        for truthy in &["true", "1", "yes", "on"] {
            let mut env = HashMap::new();
            env.insert(
                "FASTAPI_ROOT_PATH_IN_SERVERS".to_string(),
                (*truthy).to_string(),
            );

            let mut config = AppConfig::new().root_path_in_servers(false);
            config
                .apply_env_with(AppConfig::DEFAULT_ENV_PREFIX, |key| {
                    Ok(env.get(key).cloned())
                })
                .expect("env config");

            assert!(
                config.root_path_in_servers,
                "expected true for value: {}",
                truthy
            );
        }
    }

    #[test]
    fn app_config_from_env_root_path_in_servers_falsy() {
        for falsy in &["false", "0", "no", "off"] {
            let mut env = HashMap::new();
            env.insert(
                "FASTAPI_ROOT_PATH_IN_SERVERS".to_string(),
                (*falsy).to_string(),
            );

            let mut config = AppConfig::default();
            config
                .apply_env_with(AppConfig::DEFAULT_ENV_PREFIX, |key| {
                    Ok(env.get(key).cloned())
                })
                .expect("env config");

            assert!(
                !config.root_path_in_servers,
                "expected false for value: {}",
                falsy
            );
        }
    }

    #[test]
    fn app_config_from_file_root_path() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("fastapi_config_root_{stamp}.json"));

        let json = r#"{
  "name": "Proxied API",
  "root_path": "/api/v2",
  "root_path_in_servers": false
}"#;
        std::fs::write(&path, json).expect("write temp config");

        let config = AppConfig::from_file(&path).expect("file config");
        assert_eq!(config.name, "Proxied API");
        assert_eq!(config.root_path, "/api/v2");
        assert!(!config.root_path_in_servers);
    }

    #[test]
    fn app_config_openapi_server_returns_some_when_configured() {
        let config = AppConfig::new()
            .root_path("/api/v1")
            .root_path_in_servers(true);

        let server = config.openapi_server();
        assert!(server.is_some());

        let (url, description) = server.unwrap();
        assert_eq!(url, "/api/v1");
        assert!(description.is_some());
    }

    #[test]
    fn app_config_openapi_server_returns_none_when_disabled() {
        let config = AppConfig::new()
            .root_path("/api/v1")
            .root_path_in_servers(false);

        assert!(config.openapi_server().is_none());
    }

    #[test]
    fn app_config_openapi_server_returns_none_when_empty_root_path() {
        let config = AppConfig::new().root_path_in_servers(true);

        assert!(config.openapi_server().is_none());
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
            .with_state(DbPool {
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

    // ========================================================================
    // HEAD Method Tests
    // ========================================================================

    #[test]
    fn head_request_returns_empty_body() {
        // GET handler returns a body
        let app = App::builder().get("/", test_handler).build();

        let ctx = test_context();
        let mut req = Request::new(Method::Head, "/");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
        // Body should be empty for HEAD
        assert!(response.body_ref().is_empty());
    }

    #[test]
    fn head_request_preserves_content_type() {
        // Handler that sets Content-Type
        fn json_handler(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
            std::future::ready(
                Response::ok()
                    .header("content-type", b"application/json".to_vec())
                    .body(ResponseBody::Bytes(br#"{"status":"ok"}"#.to_vec())),
            )
        }

        let app = App::builder().get("/api", json_handler).build();

        let ctx = test_context();
        let mut req = Request::new(Method::Head, "/api");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.body_ref().is_empty());

        // Content-Type should be preserved
        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"));
        assert!(content_type.is_some());
        assert_eq!(content_type.unwrap().1, b"application/json");
    }

    #[test]
    fn head_request_adds_content_length_for_known_body() {
        let app = App::builder().get("/", test_handler).build();

        let ctx = test_context();
        let mut req = Request::new(Method::Head, "/");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.body_ref().is_empty());

        // Content-Length should be set to original body length ("Hello, World!" = 13 bytes)
        let content_length = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        assert!(content_length.is_some());
        assert_eq!(content_length.unwrap().1, b"13");
    }

    #[test]
    fn head_request_preserves_existing_content_length() {
        // Handler that explicitly sets Content-Length
        fn explicit_length_handler(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> std::future::Ready<Response> {
            std::future::ready(
                Response::ok()
                    .header("content-length", b"999".to_vec()) // Explicit, different from actual
                    .body(ResponseBody::Bytes(b"short".to_vec())),
            )
        }

        let app = App::builder().get("/", explicit_length_handler).build();

        let ctx = test_context();
        let mut req = Request::new(Method::Head, "/");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.body_ref().is_empty());

        // Should preserve the explicit Content-Length, not replace it
        let content_length = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        assert!(content_length.is_some());
        assert_eq!(content_length.unwrap().1, b"999");
    }

    #[test]
    fn head_request_for_empty_body_adds_zero_content_length() {
        fn empty_handler(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> std::future::Ready<Response> {
            std::future::ready(Response::ok().body(ResponseBody::Empty))
        }

        let app = App::builder().get("/", empty_handler).build();

        let ctx = test_context();
        let mut req = Request::new(Method::Head, "/");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.body_ref().is_empty());

        // Content-Length should be 0 for empty body
        let content_length = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        assert!(content_length.is_some());
        assert_eq!(content_length.unwrap().1, b"0");
    }

    #[test]
    fn head_falls_through_to_get_route() {
        // Only register GET, HEAD should still work
        let app = App::builder()
            .get("/resource", test_handler)
            .post("/resource", test_handler) // Different route for POST
            .build();

        let ctx = test_context();

        // HEAD to GET route should work
        let mut head_req = Request::new(Method::Head, "/resource");
        let response = futures_executor::block_on(app.handle(&ctx, &mut head_req));
        assert_eq!(response.status().as_u16(), 200);
        assert!(response.body_ref().is_empty());
    }

    #[test]
    fn head_to_unknown_path_returns_404() {
        let app = App::builder().get("/", test_handler).build();

        let ctx = test_context();
        let mut req = Request::new(Method::Head, "/unknown");

        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 404);
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

    // =========================================================================
    // Async Lifecycle Hooks Tests
    // =========================================================================

    #[test]
    fn async_startup_hook_runs() {
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let app = App::builder()
            .on_startup_async(move || {
                let counter = Arc::clone(&counter_clone);
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }
            })
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn async_startup_hook_error_aborts() {
        let app = App::builder()
            .on_startup_async(|| async { Err(StartupHookError::new("async connection failed")) })
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(!outcome.can_proceed());

        if let StartupOutcome::Aborted(err) = outcome {
            assert!(err.message.contains("async connection failed"));
        } else {
            panic!("Expected Aborted outcome");
        }
    }

    #[test]
    fn async_shutdown_hook_runs() {
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let app = App::builder()
            .on_shutdown_async(move || {
                let counter = Arc::clone(&counter_clone);
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            })
            .get("/", test_handler)
            .build();

        futures_executor::block_on(app.run_shutdown_hooks());
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn mixed_sync_and_async_startup_hooks() {
        let order = Arc::new(parking_lot::Mutex::new(Vec::new()));

        let order1 = Arc::clone(&order);
        let order2 = Arc::clone(&order);
        let order3 = Arc::clone(&order);

        let app = App::builder()
            .on_startup(move || {
                order1.lock().push(1);
                Ok(())
            })
            .on_startup_async(move || {
                let order = Arc::clone(&order2);
                async move {
                    order.lock().push(2);
                    Ok(())
                }
            })
            .on_startup(move || {
                order3.lock().push(3);
                Ok(())
            })
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());

        // FIFO order: 1, 2, 3
        assert_eq!(*order.lock(), vec![1, 2, 3]);
    }

    #[test]
    fn mixed_sync_and_async_shutdown_hooks() {
        let order = Arc::new(parking_lot::Mutex::new(Vec::new()));

        let order1 = Arc::clone(&order);
        let order2 = Arc::clone(&order);
        let order3 = Arc::clone(&order);

        let app = App::builder()
            .on_shutdown(move || {
                order1.lock().push(1);
            })
            .on_shutdown_async(move || {
                let order = Arc::clone(&order2);
                async move {
                    order.lock().push(2);
                }
            })
            .on_shutdown(move || {
                order3.lock().push(3);
            })
            .get("/", test_handler)
            .build();

        futures_executor::block_on(app.run_shutdown_hooks());

        // Async hooks run first (LIFO within async), then sync hooks (LIFO within sync)
        // Async: [2], Sync LIFO: [3, 1] => [2, 3, 1]
        assert_eq!(*order.lock(), vec![2, 3, 1]);
    }

    // =========================================================================
    // State Accessible in Handlers Tests
    // =========================================================================

    #[test]
    fn state_accessible_via_app_get_state() {
        #[derive(Debug, Clone)]
        struct DatabasePool {
            connection_count: usize,
        }

        #[derive(Debug, Clone)]
        struct CacheClient {
            max_entries: usize,
        }

        let app = App::builder()
            .with_state(DatabasePool {
                connection_count: 10,
            })
            .with_state(CacheClient { max_entries: 1000 })
            .get("/", test_handler)
            .build();

        // Multiple state types accessible
        let db = app.get_state::<DatabasePool>();
        assert!(db.is_some());
        assert_eq!(db.unwrap().connection_count, 10);

        let cache = app.get_state::<CacheClient>();
        assert!(cache.is_some());
        assert_eq!(cache.unwrap().max_entries, 1000);

        // Non-existent state returns None
        let missing = app.get_state::<String>();
        assert!(missing.is_none());
    }

    #[test]
    fn state_container_replace_on_duplicate_type() {
        struct Counter(u32);

        let mut container = StateContainer::new();
        container.insert(Counter(1));
        assert_eq!(container.get::<Counter>().unwrap().0, 1);

        // Replace with new value
        container.insert(Counter(42));
        assert_eq!(container.get::<Counter>().unwrap().0, 42);

        // Still only one entry
        assert_eq!(container.len(), 1);
    }

    #[test]
    fn state_container_empty_checks() {
        let container = StateContainer::new();
        assert!(container.is_empty());
        assert_eq!(container.len(), 0);

        let mut container = StateContainer::new();
        container.insert(42i32);
        assert!(!container.is_empty());
        assert_eq!(container.len(), 1);
    }

    // =========================================================================
    // Configuration Validation Tests
    // =========================================================================

    #[test]
    fn app_config_validation_rejects_empty_version() {
        let config = AppConfig {
            version: String::new(),
            ..Default::default()
        };
        let err = config.validate().expect_err("empty version invalid");
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn app_config_validation_rejects_zero_body_size() {
        let config = AppConfig {
            max_body_size: 0,
            ..Default::default()
        };
        let err = config.validate().expect_err("zero body size invalid");
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn app_config_validation_rejects_zero_timeout() {
        let config = AppConfig {
            request_timeout_ms: 0,
            ..Default::default()
        };
        let err = config.validate().expect_err("zero timeout invalid");
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn app_config_debug_bool_parsing() {
        // Test various boolean string formats
        assert!(parse_bool("test", "true").unwrap());
        assert!(parse_bool("test", "TRUE").unwrap());
        assert!(parse_bool("test", "1").unwrap());
        assert!(parse_bool("test", "yes").unwrap());
        assert!(parse_bool("test", "YES").unwrap());
        assert!(parse_bool("test", "on").unwrap());
        assert!(parse_bool("test", "ON").unwrap());

        assert!(!parse_bool("test", "false").unwrap());
        assert!(!parse_bool("test", "FALSE").unwrap());
        assert!(!parse_bool("test", "0").unwrap());
        assert!(!parse_bool("test", "no").unwrap());
        assert!(!parse_bool("test", "NO").unwrap());
        assert!(!parse_bool("test", "off").unwrap());
        assert!(!parse_bool("test", "OFF").unwrap());

        // Invalid values
        assert!(parse_bool("test", "maybe").is_err());
        assert!(parse_bool("test", "2").is_err());
    }

    #[test]
    fn app_config_unsupported_format() {
        let err = AppConfig::from_file("/tmp/config.yaml");
        assert!(matches!(err, Err(ConfigError::UnsupportedFormat { .. })));
    }

    // =========================================================================
    // Full Lifecycle Integration Tests
    // =========================================================================

    #[test]
    fn full_lifecycle_startup_serve_shutdown() {
        let lifecycle_log = Arc::new(parking_lot::Mutex::new(Vec::new()));

        let log1 = Arc::clone(&lifecycle_log);
        let log2 = Arc::clone(&lifecycle_log);
        let log3 = Arc::clone(&lifecycle_log);
        let log4 = Arc::clone(&lifecycle_log);

        let app = App::builder()
            .on_startup(move || {
                log1.lock().push("startup_1");
                Ok(())
            })
            .on_startup(move || {
                log2.lock().push("startup_2");
                Ok(())
            })
            .on_shutdown(move || {
                log3.lock().push("shutdown_1");
            })
            .on_shutdown(move || {
                log4.lock().push("shutdown_2");
            })
            .get("/", test_handler)
            .build();

        // Phase 1: Startup
        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());

        // Phase 2: Serve (simulated - just verify app is functional)
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);

        // Phase 3: Shutdown
        futures_executor::block_on(app.run_shutdown_hooks());

        // Verify lifecycle order
        let log = lifecycle_log.lock();
        assert_eq!(
            *log,
            vec!["startup_1", "startup_2", "shutdown_2", "shutdown_1"]
        );
    }

    #[test]
    fn lifecycle_startup_failure_prevents_serving() {
        let serve_attempted = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let app = App::builder()
            .on_startup(|| Err(StartupHookError::new("database unavailable")))
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());

        // Startup failed - should not proceed to serving
        if outcome.can_proceed() {
            serve_attempted.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        assert!(!serve_attempted.load(std::sync::atomic::Ordering::SeqCst));
        assert!(matches!(outcome, StartupOutcome::Aborted(_)));
    }

    #[test]
    fn lifecycle_with_state_initialization() {
        #[derive(Debug)]
        struct AppState {
            initialized: std::sync::atomic::AtomicBool,
        }

        let state = Arc::new(AppState {
            initialized: std::sync::atomic::AtomicBool::new(false),
        });
        let state_for_hook = Arc::clone(&state);

        let app = App::builder()
            .on_startup(move || {
                state_for_hook
                    .initialized
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .get("/", test_handler)
            .build();

        // Before startup
        assert!(!state.initialized.load(std::sync::atomic::Ordering::SeqCst));

        // Run startup
        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());

        // After startup - state initialized
        assert!(state.initialized.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn lifecycle_shutdown_runs_even_after_failed_startup() {
        let shutdown_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown_ran);

        let app = App::builder()
            .on_startup(|| Err(StartupHookError::new("startup failed")))
            .on_shutdown(move || {
                shutdown_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            })
            .get("/", test_handler)
            .build();

        // Startup fails
        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(!outcome.can_proceed());

        // But shutdown hooks should still be available to run for cleanup
        futures_executor::block_on(app.run_shutdown_hooks());
        assert!(shutdown_ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn multiple_lifecycle_phases_with_async_hooks() {
        let log = Arc::new(parking_lot::Mutex::new(Vec::<&str>::new()));

        let log1 = Arc::clone(&log);
        let log2 = Arc::clone(&log);
        let log3 = Arc::clone(&log);
        let log4 = Arc::clone(&log);

        let app = App::builder()
            .on_startup(move || {
                log1.lock().push("sync_startup");
                Ok(())
            })
            .on_startup_async(move || {
                let log = Arc::clone(&log2);
                async move {
                    log.lock().push("async_startup");
                    Ok(())
                }
            })
            .on_shutdown(move || {
                log3.lock().push("sync_shutdown");
            })
            .on_shutdown_async(move || {
                let log = Arc::clone(&log4);
                async move {
                    log.lock().push("async_shutdown");
                }
            })
            .get("/", test_handler)
            .build();

        // Run full lifecycle
        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());

        futures_executor::block_on(app.run_shutdown_hooks());

        // Verify order: startup FIFO, shutdown LIFO
        let events = log.lock();
        assert_eq!(
            *events,
            vec![
                "sync_startup",
                "async_startup",
                "async_shutdown",
                "sync_shutdown"
            ]
        );
    }

    // =========================================================================
    // Lifespan Tests
    // =========================================================================

    #[test]
    fn lifespan_scope_creation() {
        let scope = LifespanScope::new(42i32);
        assert_eq!(scope.state, 42);
    }

    #[test]
    fn lifespan_scope_with_cleanup() {
        let cleanup_called = Arc::new(Mutex::new(false));
        let cleanup_called_clone = Arc::clone(&cleanup_called);

        let mut scope = LifespanScope::new("state").on_shutdown(async move {
            *cleanup_called_clone.lock().unwrap() = true;
        });

        // Cleanup should not be called yet
        assert!(!*cleanup_called.lock().unwrap());

        // Take the cleanup
        let cleanup = scope.take_cleanup();
        assert!(cleanup.is_some());

        // Run the cleanup
        futures_executor::block_on(cleanup.unwrap());
        assert!(*cleanup_called.lock().unwrap());
    }

    #[test]
    fn lifespan_error_display() {
        let err = LifespanError::new("connection failed");
        assert!(err.to_string().contains("connection failed"));
        assert!(err.source.is_none());

        let io_err = std::io::Error::other("disk full");
        let err_with_source = LifespanError::with_source("backup failed", io_err);
        assert!(err_with_source.to_string().contains("backup failed"));
        assert!(err_with_source.source.is_some());
    }

    #[test]
    fn lifespan_error_into_startup_hook_error() {
        let err = LifespanError::new("startup failed");
        let hook_err: StartupHookError = err.into();
        assert!(hook_err.abort);
        assert!(hook_err.message.contains("startup failed"));
    }

    /// Simulated database pool for testing.
    struct TestDbPool {
        connection_count: i32,
    }

    #[test]
    fn lifespan_injects_state() {
        let app = App::builder()
            .lifespan(|| async {
                let pool = TestDbPool {
                    connection_count: 10,
                };
                Ok(LifespanScope::new(pool))
            })
            .get("/", test_handler)
            .build();

        // Run startup hooks (which runs lifespan)
        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());

        // State should now be available
        let pool = app.get_state::<TestDbPool>();
        assert!(pool.is_some());
        assert_eq!(pool.unwrap().connection_count, 10);
    }

    #[test]
    fn lifespan_runs_cleanup_on_shutdown() {
        let cleanup_log = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let log_clone = Arc::clone(&cleanup_log);

        let app = App::builder()
            .lifespan(move || {
                let log = Arc::clone(&log_clone);
                async move {
                    let pool = TestDbPool {
                        connection_count: 5,
                    };
                    Ok(LifespanScope::new(pool).on_shutdown(async move {
                        log.lock().unwrap().push("cleanup");
                    }))
                }
            })
            .get("/", test_handler)
            .build();

        // Run startup
        let outcome = futures_executor::block_on(app.run_startup_hooks());
        assert!(outcome.can_proceed());

        // Cleanup not called yet
        assert!(cleanup_log.lock().unwrap().is_empty());

        // Run shutdown
        futures_executor::block_on(app.run_shutdown_hooks());

        // Cleanup should have been called
        assert_eq!(*cleanup_log.lock().unwrap(), vec!["cleanup"]);
    }

    #[test]
    fn lifespan_error_aborts_startup() {
        let app = App::builder()
            .lifespan(|| async {
                Err::<LifespanScope<()>, _>(LifespanError::new("database connection failed"))
            })
            .get("/", test_handler)
            .build();

        let outcome = futures_executor::block_on(app.run_startup_hooks());

        match outcome {
            StartupOutcome::Aborted(err) => {
                assert!(err.message.contains("database connection failed"));
                assert!(err.abort);
            }
            _ => panic!("expected Aborted outcome"),
        }
    }

    #[test]
    fn lifespan_runs_before_other_startup_hooks() {
        let log = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let log1 = Arc::clone(&log);
        let log2 = Arc::clone(&log);

        let app = App::builder()
            .on_startup(move || {
                log1.lock().unwrap().push("regular_hook");
                Ok(())
            })
            .lifespan(move || {
                let log = Arc::clone(&log2);
                async move {
                    log.lock().unwrap().push("lifespan");
                    Ok(LifespanScope::new(()))
                }
            })
            .get("/", test_handler)
            .build();

        futures_executor::block_on(app.run_startup_hooks());

        // Lifespan should run before regular hooks
        let events = log.lock().unwrap();
        assert_eq!(*events, vec!["lifespan", "regular_hook"]);
    }

    #[test]
    fn lifespan_cleanup_runs_after_other_shutdown_hooks() {
        let log = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let log1 = Arc::clone(&log);
        let log2 = Arc::clone(&log);

        let app = App::builder()
            .on_shutdown(move || {
                log1.lock().unwrap().push("regular_hook");
            })
            .lifespan(move || {
                let log = Arc::clone(&log2);
                async move {
                    Ok(LifespanScope::new(()).on_shutdown(async move {
                        log.lock().unwrap().push("lifespan_cleanup");
                    }))
                }
            })
            .get("/", test_handler)
            .build();

        futures_executor::block_on(app.run_startup_hooks());
        futures_executor::block_on(app.run_shutdown_hooks());

        // Lifespan cleanup should run after regular hooks
        let events = log.lock().unwrap();
        assert_eq!(*events, vec!["regular_hook", "lifespan_cleanup"]);
    }

    // =========================================================================
    // Sub-Application Mounting Tests
    // =========================================================================

    #[test]
    fn mounted_app_prefix_matching() {
        let mounted = MountedApp::new("/admin", App::builder().build());

        // Exact match returns "/"
        assert_eq!(mounted.match_prefix("/admin"), Some("/"));

        // Prefix match with remaining path
        assert_eq!(mounted.match_prefix("/admin/users"), Some("/users"));
        assert_eq!(mounted.match_prefix("/admin/users/123"), Some("/users/123"));

        // No match - different prefix
        assert_eq!(mounted.match_prefix("/api"), None);
        assert_eq!(mounted.match_prefix("/"), None);

        // No match - partial prefix without slash boundary
        assert_eq!(mounted.match_prefix("/administrator"), None);
    }

    #[test]
    fn mounted_app_prefix_normalization() {
        // Prefix without leading slash gets normalized
        let mounted = MountedApp::new("admin", App::builder().build());
        assert_eq!(mounted.prefix(), "/admin");

        // Trailing slash gets removed
        let mounted = MountedApp::new("/admin/", App::builder().build());
        assert_eq!(mounted.prefix(), "/admin");

        // Multiple trailing slashes get removed
        let mounted = MountedApp::new("/admin///", App::builder().build());
        assert_eq!(mounted.prefix(), "/admin");
    }

    #[test]
    fn app_builder_mount_adds_mounted_app() {
        let sub_app = App::builder().get("/", test_handler).build();

        let app = App::builder()
            .get("/", test_handler)
            .mount("/admin", sub_app)
            .build();

        assert_eq!(app.mounted_apps().len(), 1);
        assert_eq!(app.mounted_apps()[0].prefix(), "/admin");
    }

    #[test]
    fn app_builder_multiple_mounts() {
        let admin_app = App::builder().get("/", test_handler).build();
        let api_app = App::builder().get("/", test_handler).build();
        let docs_app = App::builder().get("/", test_handler).build();

        let app = App::builder()
            .get("/", test_handler)
            .mount("/admin", admin_app)
            .mount("/api", api_app)
            .mount("/docs", docs_app)
            .build();

        assert_eq!(app.mounted_apps().len(), 3);
        assert_eq!(app.mounted_apps()[0].prefix(), "/admin");
        assert_eq!(app.mounted_apps()[1].prefix(), "/api");
        assert_eq!(app.mounted_apps()[2].prefix(), "/docs");
    }

    fn admin_handler(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
        std::future::ready(Response::ok().body(ResponseBody::Bytes(b"Admin Panel".to_vec())))
    }

    #[test]
    fn app_routes_to_mounted_app() {
        let admin_app = App::builder()
            .get("/", admin_handler)
            .get("/users", admin_handler)
            .build();

        let app = App::builder()
            .get("/", test_handler)
            .mount("/admin", admin_app)
            .build();

        let ctx = test_context();

        // Request to parent app
        let mut req = Request::new(Method::Get, "/");
        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
        if let ResponseBody::Bytes(body) = response.body_ref() {
            assert_eq!(body.as_slice(), b"Hello, World!");
        }

        // Request to mounted app (exact prefix)
        let mut req = Request::new(Method::Get, "/admin");
        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
        if let ResponseBody::Bytes(body) = response.body_ref() {
            assert_eq!(body.as_slice(), b"Admin Panel");
        }

        // Request to mounted app (with path)
        let mut req = Request::new(Method::Get, "/admin/users");
        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);
        if let ResponseBody::Bytes(body) = response.body_ref() {
            assert_eq!(body.as_slice(), b"Admin Panel");
        }
    }

    #[test]
    fn mounted_app_404_for_unknown_routes() {
        let admin_app = App::builder().get("/users", admin_handler).build();

        let app = App::builder()
            .get("/", test_handler)
            .mount("/admin", admin_app)
            .build();

        let ctx = test_context();

        // Request to mounted app for unknown route
        let mut req = Request::new(Method::Get, "/admin/unknown");
        let response = futures_executor::block_on(app.handle(&ctx, &mut req));
        assert_eq!(response.status().as_u16(), 404);
    }
}
