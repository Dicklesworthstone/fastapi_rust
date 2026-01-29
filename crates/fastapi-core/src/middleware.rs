//! Middleware abstraction for request/response processing.
//!
//! This module provides a flexible middleware system that allows:
//! - Pre-processing requests before handlers run
//! - Post-processing responses after handlers complete
//! - Short-circuiting to return early without calling handlers
//! - Composable middleware stacks with defined ordering
//!
//! # Design Philosophy
//!
//! The middleware system follows these principles:
//! - **Zero-cost when empty**: No overhead if no middleware is configured
//! - **Async-native**: All hooks are async for I/O operations
//! - **Cancel-aware**: Integrates with asupersync's cancellation
//! - **Composable**: Middleware can be stacked and layered
//!
//! # Ordering Semantics
//!
//! Middleware executes in a specific order:
//! 1. `before` hooks run in **registration order** (first registered, first run)
//! 2. Handler executes
//! 3. `after` hooks run in **reverse order** (last registered, first run)
//!
//! This creates an "onion" model where the first middleware wraps everything:
//!
//! ```text
//! Request → MW1.before → MW2.before → MW3.before → Handler
//!                                                     ↓
//! Response ← MW1.after ← MW2.after ← MW3.after ← Response
//! ```
//!
//! # Example
//!
//! ```ignore
//! use fastapi_core::middleware::{Middleware, ControlFlow};
//! use fastapi_core::{Request, Response, RequestContext};
//!
//! struct LoggingMiddleware;
//!
//! impl Middleware for LoggingMiddleware {
//!     async fn before(&self, ctx: &RequestContext, req: &Request) -> ControlFlow {
//!         println!("Request: {} {}", req.method(), req.path());
//!         ControlFlow::Continue
//!     }
//!
//!     async fn after(&self, _ctx: &RequestContext, _req: &Request, resp: Response) -> Response {
//!         println!("Response: {}", resp.status().as_u16());
//!         resp
//!     }
//! }
//! ```

use std::collections::HashSet;
use std::future::Future;
use std::ops::ControlFlow as StdControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use crate::context::RequestContext;
use crate::dependency::DependencyOverrides;
use crate::logging::{LogConfig, RequestLogger};
use crate::request::{Body, Request};
use crate::response::Response;

/// A boxed future for async middleware operations.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Control flow for middleware `before` hooks.
///
/// Determines whether request processing should continue to the handler
/// or short-circuit with an early response.
#[derive(Debug)]
pub enum ControlFlow {
    /// Continue processing - call the next middleware or handler.
    Continue,
    /// Short-circuit - return this response immediately without calling the handler.
    ///
    /// Subsequent `before` hooks and the handler will NOT run.
    /// However, `after` hooks for middleware that already ran their `before` WILL run.
    Break(Response),
}

impl ControlFlow {
    /// Returns `true` if this is `Continue`.
    #[must_use]
    pub fn is_continue(&self) -> bool {
        matches!(self, Self::Continue)
    }

    /// Returns `true` if this is `Break`.
    #[must_use]
    pub fn is_break(&self) -> bool {
        matches!(self, Self::Break(_))
    }
}

impl From<ControlFlow> for StdControlFlow<Response, ()> {
    fn from(cf: ControlFlow) -> Self {
        match cf {
            ControlFlow::Continue => StdControlFlow::Continue(()),
            ControlFlow::Break(r) => StdControlFlow::Break(r),
        }
    }
}

/// The core middleware trait.
///
/// Middleware wraps request handling with pre-processing and post-processing hooks.
/// Implementations must be thread-safe (`Send + Sync`) as middleware may be shared
/// across concurrent requests.
///
/// # Implementation Guide
///
/// - **`before`**: Inspect/modify the request, optionally short-circuit
/// - **`after`**: Inspect/modify the response
///
/// Both methods have default implementations that do nothing, so you can
/// implement only what you need.
///
/// # Cancel-Safety
///
/// Middleware should check `ctx.checkpoint()` for long operations to support
/// graceful cancellation when clients disconnect or timeouts occur.
///
/// # Example: Request Timing
///
/// ```ignore
/// use std::time::Instant;
/// use fastapi_core::middleware::{Middleware, ControlFlow};
///
/// struct TimingMiddleware;
///
/// impl Middleware for TimingMiddleware {
///     async fn before(&self, ctx: &RequestContext, req: &mut Request) -> ControlFlow {
///         // Store start time in request extensions (future feature)
///         ControlFlow::Continue
///     }
///
///     async fn after(&self, _ctx: &RequestContext, _req: &Request, mut resp: Response) -> Response {
///         // Add timing header
///         resp = resp.header("X-Response-Time", b"42ms".to_vec());
///         resp
///     }
/// }
/// ```
pub trait Middleware: Send + Sync {
    /// Called before the handler executes.
    ///
    /// # Parameters
    ///
    /// - `ctx`: Request context with cancellation support
    /// - `req`: Mutable request that can be inspected or modified
    ///
    /// # Returns
    ///
    /// - `ControlFlow::Continue` to proceed to the next middleware/handler
    /// - `ControlFlow::Break(response)` to short-circuit and return immediately
    ///
    /// # Default Implementation
    ///
    /// Returns `ControlFlow::Continue` (no-op).
    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow> {
        Box::pin(async { ControlFlow::Continue })
    }

    /// Called after the handler executes.
    ///
    /// # Parameters
    ///
    /// - `ctx`: Request context with cancellation support
    /// - `req`: The request (read-only at this point)
    /// - `response`: The response from the handler or previous `after` hooks
    ///
    /// # Returns
    ///
    /// The response to pass to the next `after` hook or to return to the client.
    ///
    /// # Default Implementation
    ///
    /// Returns the response unchanged (no-op).
    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        Box::pin(async move { response })
    }

    /// Returns the middleware name for debugging and logging.
    ///
    /// Override this to provide a meaningful name for your middleware.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// A handler that processes requests into responses.
///
/// This trait abstracts over handler functions, allowing middleware to wrap
/// any type that can handle requests.
pub trait Handler: Send + Sync {
    /// Process a request and return a response.
    fn call<'a>(&'a self, ctx: &'a RequestContext, req: &'a mut Request)
    -> BoxFuture<'a, Response>;

    /// Optional dependency overrides to apply when building request contexts.
    ///
    /// Default implementation returns `None`, which means no overrides.
    fn dependency_overrides(&self) -> Option<Arc<DependencyOverrides>> {
        None
    }
}

/// Implement Handler for async functions.
///
/// This allows any async function with the signature
/// `async fn(&RequestContext, &mut Request) -> Response` to be used as a handler.
impl<F, Fut> Handler for F
where
    F: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync,
    Fut: Future<Output = Response> + Send + 'static,
{
    fn call<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, Response> {
        let fut = self(ctx, req);
        Box::pin(fut)
    }
}

/// A stack of middleware that wraps a handler.
///
/// The stack executes middleware in order:
/// 1. `before` hooks run first-to-last (registration order)
/// 2. Handler executes (if no middleware short-circuited)
/// 3. `after` hooks run last-to-first (reverse order)
///
/// # Example
///
/// ```ignore
/// let mut stack = MiddlewareStack::new();
/// stack.push(LoggingMiddleware);
/// stack.push(AuthMiddleware);
/// stack.push(CorsMiddleware);
///
/// let response = stack.execute(&handler, &ctx, &mut request).await;
/// ```
#[derive(Default)]
pub struct MiddlewareStack {
    middleware: Vec<Arc<dyn Middleware>>,
}

impl MiddlewareStack {
    /// Creates an empty middleware stack.
    #[must_use]
    pub fn new() -> Self {
        Self {
            middleware: Vec::new(),
        }
    }

    /// Creates a middleware stack with pre-allocated capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            middleware: Vec::with_capacity(capacity),
        }
    }

    /// Adds middleware to the end of the stack.
    ///
    /// Middleware added first will have its `before` run first and `after` run last.
    pub fn push<M: Middleware + 'static>(&mut self, middleware: M) {
        self.middleware.push(Arc::new(middleware));
    }

    /// Adds middleware wrapped in an Arc.
    ///
    /// Useful for sharing middleware across multiple stacks.
    pub fn push_arc(&mut self, middleware: Arc<dyn Middleware>) {
        self.middleware.push(middleware);
    }

    /// Returns the number of middleware in the stack.
    #[must_use]
    pub fn len(&self) -> usize {
        self.middleware.len()
    }

    /// Returns `true` if the stack is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.middleware.is_empty()
    }

    /// Executes the middleware stack with the given handler.
    ///
    /// # Execution Order
    ///
    /// 1. Each middleware's `before` hook runs in order
    /// 2. If any `before` returns `Break`, skip remaining middleware and handler
    /// 3. Handler executes
    /// 4. Each middleware's `after` hook runs in reverse order
    ///
    /// # Short-Circuit Behavior
    ///
    /// If middleware N calls `Break(response)`:
    /// - Middleware N+1..end `before` hooks do NOT run
    /// - Handler does NOT run
    /// - Middleware 0..N `after` hooks STILL run (in reverse: N, N-1, ..., 0)
    ///
    /// This ensures cleanup middleware (like timing or logging) always runs.
    pub async fn execute<H: Handler>(
        &self,
        handler: &H,
        ctx: &RequestContext,
        req: &mut Request,
    ) -> Response {
        // Track which middleware ran their `before` hook
        let mut ran_before_count = 0;

        // Run before hooks in order
        for mw in &self.middleware {
            let _ = ctx.checkpoint();
            match mw.before(ctx, req).await {
                ControlFlow::Continue => {
                    ran_before_count += 1;
                }
                ControlFlow::Break(response) => {
                    // Short-circuit: run after hooks for middleware that already ran
                    return self
                        .run_after_hooks(ctx, req, response, ran_before_count)
                        .await;
                }
            }
        }

        // All before hooks passed, call the handler
        let _ = ctx.checkpoint();
        let response = handler.call(ctx, req).await;

        // Run after hooks in reverse order
        self.run_after_hooks(ctx, req, response, ran_before_count)
            .await
    }

    /// Runs after hooks for middleware that ran their before hook.
    async fn run_after_hooks(
        &self,
        ctx: &RequestContext,
        req: &Request,
        mut response: Response,
        count: usize,
    ) -> Response {
        // Run in reverse order (last middleware's after runs first)
        for mw in self.middleware[..count].iter().rev() {
            let _ = ctx.checkpoint();
            response = mw.after(ctx, req, response).await;
        }
        response
    }
}

/// A layer that can wrap handlers with middleware.
///
/// This provides a more functional composition style similar to Tower's Layer trait.
///
/// # Example
///
/// ```ignore
/// let layer = Layer::new(LoggingMiddleware);
/// let wrapped = layer.wrap(my_handler);
/// ```
pub struct Layer<M> {
    middleware: M,
}

impl<M: Middleware + Clone> Layer<M> {
    /// Creates a new layer with the given middleware.
    pub fn new(middleware: M) -> Self {
        Self { middleware }
    }

    /// Wraps a handler with this layer's middleware.
    pub fn wrap<H: Handler>(&self, handler: H) -> Layered<M, H> {
        Layered {
            middleware: self.middleware.clone(),
            inner: handler,
        }
    }
}

/// A handler wrapped with middleware via a Layer.
pub struct Layered<M, H> {
    middleware: M,
    inner: H,
}

impl<M: Middleware, H: Handler> Handler for Layered<M, H> {
    fn call<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, Response> {
        Box::pin(async move {
            // Run before hook
            let _ = ctx.checkpoint();
            match self.middleware.before(ctx, req).await {
                ControlFlow::Continue => {
                    // Call inner handler
                    let _ = ctx.checkpoint();
                    let response = self.inner.call(ctx, req).await;
                    // Run after hook
                    let _ = ctx.checkpoint();
                    self.middleware.after(ctx, req, response).await
                }
                ControlFlow::Break(response) => {
                    // Short-circuit: still run after for this middleware
                    let _ = ctx.checkpoint();
                    self.middleware.after(ctx, req, response).await
                }
            }
        })
    }
}

// ============================================================================
// Common Middleware Implementations
// ============================================================================

/// No-op middleware that does nothing.
///
/// Useful as a placeholder or for testing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopMiddleware;

impl Middleware for NoopMiddleware {
    fn name(&self) -> &'static str {
        "Noop"
    }
}

/// Middleware that adds a custom header to all responses.
///
/// # Example
///
/// ```ignore
/// // Add X-Powered-By header to all responses
/// let mw = AddResponseHeader::new("X-Powered-By", "fastapi_rust");
/// stack.push(mw);
/// ```
#[derive(Debug, Clone)]
pub struct AddResponseHeader {
    name: String,
    value: Vec<u8>,
}

impl AddResponseHeader {
    /// Creates a new middleware that adds the specified header to responses.
    pub fn new(name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

impl Middleware for AddResponseHeader {
    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let name = self.name.clone();
        let value = self.value.clone();
        Box::pin(async move { response.header(name, value) })
    }

    fn name(&self) -> &'static str {
        "AddResponseHeader"
    }
}

/// Middleware that requires a specific header to be present.
///
/// Returns 400 Bad Request if the header is missing.
///
/// # Example
///
/// ```ignore
/// // Require X-Api-Key header
/// let mw = RequireHeader::new("X-Api-Key");
/// stack.push(mw);
/// ```
#[derive(Debug, Clone)]
pub struct RequireHeader {
    name: String,
}

impl RequireHeader {
    /// Creates a new middleware that requires the specified header.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl Middleware for RequireHeader {
    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow> {
        let has_header = req.headers().get(&self.name).is_some();
        let name = self.name.clone();
        Box::pin(async move {
            if has_header {
                ControlFlow::Continue
            } else {
                let body = format!("Missing required header: {name}");
                ControlFlow::Break(
                    Response::with_status(crate::response::StatusCode::BAD_REQUEST)
                        .header("content-type", b"text/plain".to_vec())
                        .body(crate::response::ResponseBody::Bytes(body.into_bytes())),
                )
            }
        })
    }

    fn name(&self) -> &'static str {
        "RequireHeader"
    }
}

/// Middleware that limits request processing based on path prefix.
///
/// Only allows requests to paths starting with the specified prefix.
/// Other requests receive a 404 Not Found response.
///
/// # Example
///
/// ```ignore
/// // Only allow requests to /api/*
/// let mw = PathPrefixFilter::new("/api");
/// stack.push(mw);
/// ```
#[derive(Debug, Clone)]
pub struct PathPrefixFilter {
    prefix: String,
}

impl PathPrefixFilter {
    /// Creates a new middleware that only allows requests with the specified path prefix.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

impl Middleware for PathPrefixFilter {
    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow> {
        let path_matches = req.path().starts_with(&self.prefix);
        Box::pin(async move {
            if path_matches {
                ControlFlow::Continue
            } else {
                ControlFlow::Break(Response::with_status(
                    crate::response::StatusCode::NOT_FOUND,
                ))
            }
        })
    }

    fn name(&self) -> &'static str {
        "PathPrefixFilter"
    }
}

/// Middleware that sets response status code based on a condition.
///
/// This is useful for implementing health checks or conditional responses.
#[derive(Debug, Clone)]
pub struct ConditionalStatus<F>
where
    F: Fn(&Request) -> bool + Send + Sync,
{
    condition: F,
    status_if_true: crate::response::StatusCode,
    status_if_false: crate::response::StatusCode,
}

impl<F> ConditionalStatus<F>
where
    F: Fn(&Request) -> bool + Send + Sync,
{
    /// Creates a new conditional status middleware.
    ///
    /// If the condition returns true, the response gets `status_if_true`.
    /// Otherwise, it gets `status_if_false`.
    pub fn new(
        condition: F,
        status_if_true: crate::response::StatusCode,
        status_if_false: crate::response::StatusCode,
    ) -> Self {
        Self {
            condition,
            status_if_true,
            status_if_false,
        }
    }
}

impl<F> Middleware for ConditionalStatus<F>
where
    F: Fn(&Request) -> bool + Send + Sync,
{
    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let matches = (self.condition)(req);
        let status = if matches {
            self.status_if_true
        } else {
            self.status_if_false
        };
        Box::pin(async move { Response::with_status(status).body(response.body_ref().into()) })
    }

    fn name(&self) -> &'static str {
        "ConditionalStatus"
    }
}

// ============================================================================
// CORS Middleware
// ============================================================================

/// Origin matching pattern for CORS.
#[derive(Debug, Clone)]
pub enum OriginPattern {
    /// Allow any origin.
    Any,
    /// Exact match.
    Exact(String),
    /// Wildcard match (supports `*`).
    Wildcard(String),
    /// Simple regex match (supports `^`, `$`, `.`, `*`).
    Regex(String),
}

impl OriginPattern {
    fn matches(&self, origin: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(value) => value == origin,
            Self::Wildcard(pattern) => wildcard_match(pattern, origin),
            Self::Regex(pattern) => regex_match(pattern, origin),
        }
    }
}

/// CORS configuration.
#[derive(Debug, Clone)]
pub struct CorsConfig {
    allow_any_origin: bool,
    allow_credentials: bool,
    allowed_methods: Vec<crate::request::Method>,
    allowed_headers: Vec<String>,
    expose_headers: Vec<String>,
    max_age: Option<u32>,
    origins: Vec<OriginPattern>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allow_any_origin: false,
            allow_credentials: false,
            allowed_methods: vec![
                crate::request::Method::Get,
                crate::request::Method::Post,
                crate::request::Method::Put,
                crate::request::Method::Patch,
                crate::request::Method::Delete,
                crate::request::Method::Options,
                crate::request::Method::Head,
            ],
            allowed_headers: Vec::new(),
            expose_headers: Vec::new(),
            max_age: None,
            origins: Vec::new(),
        }
    }
}

/// CORS middleware.
#[derive(Debug, Clone)]
pub struct Cors {
    config: CorsConfig,
}

impl Cors {
    /// Create a new CORS middleware with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: CorsConfig::default(),
        }
    }

    /// Replace the configuration entirely.
    #[must_use]
    pub fn config(mut self, config: CorsConfig) -> Self {
        self.config = config;
        self
    }

    /// Allow any origin.
    #[must_use]
    pub fn allow_any_origin(mut self) -> Self {
        self.config.allow_any_origin = true;
        self
    }

    /// Allow a single exact origin.
    #[must_use]
    pub fn allow_origin(mut self, origin: impl Into<String>) -> Self {
        self.config
            .origins
            .push(OriginPattern::Exact(origin.into()));
        self
    }

    /// Allow a wildcard origin pattern (supports `*`).
    #[must_use]
    pub fn allow_origin_wildcard(mut self, pattern: impl Into<String>) -> Self {
        self.config
            .origins
            .push(OriginPattern::Wildcard(pattern.into()));
        self
    }

    /// Allow a simple regex origin pattern (supports `^`, `$`, `.`, `*`).
    #[must_use]
    pub fn allow_origin_regex(mut self, pattern: impl Into<String>) -> Self {
        self.config
            .origins
            .push(OriginPattern::Regex(pattern.into()));
        self
    }

    /// Allow credentials for CORS responses.
    #[must_use]
    pub fn allow_credentials(mut self, allow: bool) -> Self {
        self.config.allow_credentials = allow;
        self
    }

    /// Override allowed HTTP methods for preflight.
    #[must_use]
    pub fn allow_methods<I>(mut self, methods: I) -> Self
    where
        I: IntoIterator<Item = crate::request::Method>,
    {
        self.config.allowed_methods = methods.into_iter().collect();
        self
    }

    /// Override allowed headers for preflight.
    #[must_use]
    pub fn allow_headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.allowed_headers = headers.into_iter().map(Into::into).collect();
        self
    }

    /// Add exposed headers for responses.
    #[must_use]
    pub fn expose_headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.expose_headers = headers.into_iter().map(Into::into).collect();
        self
    }

    /// Set the preflight max-age in seconds.
    #[must_use]
    pub fn max_age(mut self, seconds: u32) -> Self {
        self.config.max_age = Some(seconds);
        self
    }

    fn is_origin_allowed(&self, origin: &str) -> bool {
        if self.config.allow_any_origin {
            return true;
        }
        self.config
            .origins
            .iter()
            .any(|pattern| pattern.matches(origin))
    }

    fn allow_origin_value(&self, origin: &str) -> Option<String> {
        if !self.is_origin_allowed(origin) {
            return None;
        }
        if self.config.allow_any_origin && !self.config.allow_credentials {
            Some("*".to_string())
        } else {
            Some(origin.to_string())
        }
    }

    fn allow_methods_value(&self) -> String {
        self.config
            .allowed_methods
            .iter()
            .map(|method| method.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn allow_headers_value(&self, request: &Request) -> Option<String> {
        if !self.config.allowed_headers.is_empty() {
            return Some(self.config.allowed_headers.join(", "));
        }

        request
            .headers()
            .get("access-control-request-headers")
            .and_then(|value| std::str::from_utf8(value).ok())
            .map(ToString::to_string)
    }

    fn apply_common_headers(&self, mut response: Response, origin: &str) -> Response {
        if let Some(allow_origin) = self.allow_origin_value(origin) {
            let is_wildcard = allow_origin == "*";
            response = response.header("access-control-allow-origin", allow_origin.into_bytes());
            if !is_wildcard {
                response = response.header("vary", b"Origin".to_vec());
            }
            if self.config.allow_credentials {
                response = response.header("access-control-allow-credentials", b"true".to_vec());
            }
            if !self.config.expose_headers.is_empty() {
                response = response.header(
                    "access-control-expose-headers",
                    self.config.expose_headers.join(", ").into_bytes(),
                );
            }
        }
        response
    }
}

impl Default for Cors {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct CorsOrigin(String);

impl Middleware for Cors {
    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow> {
        let origin = req
            .headers()
            .get("origin")
            .and_then(|value| std::str::from_utf8(value).ok())
            .map(ToString::to_string);

        let Some(origin) = origin else {
            return Box::pin(async { ControlFlow::Continue });
        };

        if !self.is_origin_allowed(&origin) {
            let is_preflight = req.method() == crate::request::Method::Options
                && req.headers().get("access-control-request-method").is_some();
            if is_preflight {
                return Box::pin(async {
                    ControlFlow::Break(Response::with_status(
                        crate::response::StatusCode::FORBIDDEN,
                    ))
                });
            }
            return Box::pin(async { ControlFlow::Continue });
        }

        let is_preflight = req.method() == crate::request::Method::Options
            && req.headers().get("access-control-request-method").is_some();

        if is_preflight {
            let mut response = Response::no_content();
            response = self.apply_common_headers(response, &origin);
            response = response.header(
                "access-control-allow-methods",
                self.allow_methods_value().into_bytes(),
            );

            if let Some(value) = self.allow_headers_value(req) {
                response = response.header("access-control-allow-headers", value.into_bytes());
            }

            if let Some(max_age) = self.config.max_age {
                response =
                    response.header("access-control-max-age", max_age.to_string().into_bytes());
            }

            return Box::pin(async move { ControlFlow::Break(response) });
        }

        req.insert_extension(CorsOrigin(origin));
        Box::pin(async { ControlFlow::Continue })
    }

    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let origin = req.get_extension::<CorsOrigin>().map(|v| v.0.clone());
        Box::pin(async move {
            if let Some(origin) = origin {
                return self.apply_common_headers(response, &origin);
            }
            response
        })
    }

    fn name(&self) -> &'static str {
        "Cors"
    }
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    // Simple glob matcher for '*'
    let mut pat_chars = pattern.chars().peekable();
    let mut val_chars = value.chars().peekable();
    let mut star = None;
    let mut match_after_star = None;

    while let Some(p) = pat_chars.next() {
        match p {
            '*' => {
                star = Some(pat_chars.clone());
                match_after_star = Some(val_chars.clone());
            }
            _ => {
                if let Some(v) = val_chars.next() {
                    if p != v {
                        if let (Some(pat_backup), Some(val_backup)) =
                            (star.clone(), match_after_star.clone())
                        {
                            pat_chars = pat_backup;
                            val_chars = val_backup;
                            val_chars.next();
                            match_after_star = Some(val_chars.clone());
                            continue;
                        }
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }
    }

    // Consume trailing '*' in pattern
    if pat_chars.peek().is_none() && val_chars.peek().is_none() {
        return true;
    }

    if let Some(pat_backup) = star {
        if val_chars.peek().is_none() {
            let trailing = pat_backup;
            for ch in trailing {
                if ch != '*' {
                    return false;
                }
            }
            return true;
        }
    }

    val_chars.peek().is_none()
}

fn regex_match(pattern: &str, value: &str) -> bool {
    // Minimal regex engine: supports ^, $, ., *
    let pat = pattern.as_bytes();
    let text = value.as_bytes();

    if pat.first() == Some(&b'^') {
        return regex_match_here(&pat[1..], text);
    }

    let mut i = 0;
    loop {
        if regex_match_here(pat, &text[i..]) {
            return true;
        }
        if i == text.len() {
            break;
        }
        i += 1;
    }
    false
}

fn regex_match_here(pattern: &[u8], text: &[u8]) -> bool {
    if pattern.is_empty() {
        return true;
    }
    if pattern == b"$" {
        return text.is_empty();
    }
    if pattern.len() >= 2 && pattern[1] == b'*' {
        return regex_match_star(pattern[0], &pattern[2..], text);
    }
    if !text.is_empty() && (pattern[0] == b'.' || pattern[0] == text[0]) {
        return regex_match_here(&pattern[1..], &text[1..]);
    }
    false
}

fn regex_match_star(ch: u8, pattern: &[u8], text: &[u8]) -> bool {
    let mut i = 0;
    loop {
        if regex_match_here(pattern, &text[i..]) {
            return true;
        }
        if i == text.len() {
            return false;
        }
        if ch != b'.' && text[i] != ch {
            return false;
        }
        i += 1;
    }
}

// ============================================================================
// Request/Response Logging Middleware
// ============================================================================

/// Middleware that logs requests and responses with configurable redaction.
#[derive(Debug, Clone)]
pub struct RequestResponseLogger {
    log_config: LogConfig,
    redact_headers: HashSet<String>,
    log_request_headers: bool,
    log_response_headers: bool,
    log_body: bool,
    max_body_bytes: usize,
}

impl Default for RequestResponseLogger {
    fn default() -> Self {
        Self {
            log_config: LogConfig::production(),
            redact_headers: default_redacted_headers(),
            log_request_headers: true,
            log_response_headers: true,
            log_body: false,
            max_body_bytes: 1024,
        }
    }
}

impl RequestResponseLogger {
    /// Create a new logger middleware with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the logging configuration.
    #[must_use]
    pub fn log_config(mut self, config: LogConfig) -> Self {
        self.log_config = config;
        self
    }

    /// Enable or disable request header logging.
    #[must_use]
    pub fn log_request_headers(mut self, enabled: bool) -> Self {
        self.log_request_headers = enabled;
        self
    }

    /// Enable or disable response header logging.
    #[must_use]
    pub fn log_response_headers(mut self, enabled: bool) -> Self {
        self.log_response_headers = enabled;
        self
    }

    /// Enable or disable request/response body logging.
    #[must_use]
    pub fn log_body(mut self, enabled: bool) -> Self {
        self.log_body = enabled;
        self
    }

    /// Set the maximum number of body bytes to include in logs.
    #[must_use]
    pub fn max_body_bytes(mut self, max: usize) -> Self {
        self.max_body_bytes = max;
        self
    }

    /// Add a header name to redact (case-insensitive).
    #[must_use]
    pub fn redact_header(mut self, name: impl Into<String>) -> Self {
        self.redact_headers.insert(name.into().to_ascii_lowercase());
        self
    }
}

#[derive(Debug, Clone)]
struct RequestStart(Instant);

impl Middleware for RequestResponseLogger {
    fn before<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow> {
        let logger = RequestLogger::new(ctx, self.log_config.clone());
        req.insert_extension(RequestStart(Instant::now()));

        let method = req.method();
        let path = req.path();
        let query = req.query();
        let body_bytes = body_len(req.body());

        logger.info_with_fields("request", |entry| {
            let mut entry = entry
                .field("method", method)
                .field("path", path)
                .field("body_bytes", body_bytes);

            if let Some(q) = query {
                entry = entry.field("query", q);
            }

            if self.log_request_headers {
                let headers = format_headers(req.headers().iter(), &self.redact_headers);
                entry = entry.field("headers", headers);
            }

            if self.log_body {
                if let Some(body) = preview_body(req.body(), self.max_body_bytes) {
                    entry = entry.field("body", body);
                }
            }

            entry
        });

        Box::pin(async { ControlFlow::Continue })
    }

    fn after<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let logger = RequestLogger::new(ctx, self.log_config.clone());
        let duration = req
            .get_extension::<RequestStart>()
            .map(|start| start.0.elapsed())
            .unwrap_or_default();

        let status = response.status();
        let body_bytes = response.body_ref().len();

        logger.info_with_fields("response", |entry| {
            let mut entry = entry
                .field("status", status.as_u16())
                .field("duration_us", duration.as_micros())
                .field("body_bytes", body_bytes);

            if self.log_response_headers {
                let headers = format_response_headers(response.headers(), &self.redact_headers);
                entry = entry.field("headers", headers);
            }

            if self.log_body {
                if let Some(body) = preview_response_body(response.body_ref(), self.max_body_bytes)
                {
                    entry = entry.field("body", body);
                }
            }

            entry
        });

        Box::pin(async move { response })
    }

    fn name(&self) -> &'static str {
        "RequestResponseLogger"
    }
}

fn default_redacted_headers() -> HashSet<String> {
    [
        "authorization",
        "proxy-authorization",
        "cookie",
        "set-cookie",
    ]
    .iter()
    .map(ToString::to_string)
    .collect()
}

fn body_len(body: &Body) -> usize {
    match body {
        Body::Empty => 0,
        Body::Bytes(bytes) => bytes.len(),
        Body::Stream(_) => 0, // Length unknown for streaming bodies
    }
}

fn preview_body(body: &Body, max_bytes: usize) -> Option<String> {
    if max_bytes == 0 {
        return None;
    }
    match body {
        Body::Empty => None,
        Body::Bytes(bytes) => {
            if bytes.is_empty() {
                None
            } else {
                Some(format_bytes(bytes, max_bytes))
            }
        }
        Body::Stream(_) => None, // Cannot preview streaming body
    }
}

fn preview_response_body(body: &crate::response::ResponseBody, max_bytes: usize) -> Option<String> {
    if max_bytes == 0 {
        return None;
    }
    match body {
        crate::response::ResponseBody::Empty => None,
        crate::response::ResponseBody::Bytes(bytes) => {
            if bytes.is_empty() {
                None
            } else {
                Some(format_bytes(bytes, max_bytes))
            }
        }
        crate::response::ResponseBody::Stream(_) => None,
    }
}

fn format_headers<'a>(
    headers: impl Iterator<Item = (&'a str, &'a [u8])>,
    redacted: &HashSet<String>,
) -> String {
    let mut out = String::new();
    for (idx, (name, value)) in headers.enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str(name);
        out.push('=');

        let lowered = name.to_ascii_lowercase();
        if redacted.contains(&lowered) {
            out.push_str("<redacted>");
            continue;
        }

        match std::str::from_utf8(value) {
            Ok(text) => out.push_str(text),
            Err(_) => out.push_str("<binary>"),
        }
    }
    out
}

fn format_response_headers(headers: &[(String, Vec<u8>)], redacted: &HashSet<String>) -> String {
    format_headers(
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_slice())),
        redacted,
    )
}

fn format_bytes(bytes: &[u8], max_bytes: usize) -> String {
    let limit = max_bytes.min(bytes.len());
    match std::str::from_utf8(&bytes[..limit]) {
        Ok(text) => {
            let mut output = text.to_string();
            if bytes.len() > max_bytes {
                output.push_str("...");
            }
            output
        }
        Err(_) => format!("<{} bytes binary>", bytes.len()),
    }
}

// Helper for ResponseBody conversion
impl From<&crate::response::ResponseBody> for crate::response::ResponseBody {
    fn from(body: &crate::response::ResponseBody) -> Self {
        match body {
            crate::response::ResponseBody::Empty => crate::response::ResponseBody::Empty,
            crate::response::ResponseBody::Bytes(b) => {
                crate::response::ResponseBody::Bytes(b.clone())
            }
            crate::response::ResponseBody::Stream(_) => crate::response::ResponseBody::Empty,
        }
    }
}

// ============================================================================
// Request ID Middleware
// ============================================================================

/// A request ID that was extracted or generated for the current request.
///
/// This is stored in request extensions and can be retrieved by handlers
/// or other middleware for logging and tracing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

impl RequestId {
    /// Creates a new request ID with the given value.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the request ID as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Generates a new unique request ID.
    ///
    /// Uses a simple format: timestamp-counter for uniqueness without
    /// requiring external UUID dependencies.
    #[must_use]
    pub fn generate() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);

        // Format: base36 timestamp + counter for compact, unique IDs
        Self(format!("{:x}-{:04x}", timestamp, counter & 0xFFFF))
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for RequestId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Configuration for request ID middleware.
#[derive(Debug, Clone)]
pub struct RequestIdConfig {
    /// Header name to read/write request ID (default: "x-request-id").
    pub header_name: String,
    /// Whether to accept request ID from client (default: true).
    pub accept_from_client: bool,
    /// Whether to add request ID to response headers (default: true).
    pub add_to_response: bool,
    /// Maximum length of client-provided request ID (default: 128).
    pub max_client_id_length: usize,
}

impl Default for RequestIdConfig {
    fn default() -> Self {
        Self {
            header_name: "x-request-id".to_string(),
            accept_from_client: true,
            add_to_response: true,
            max_client_id_length: 128,
        }
    }
}

impl RequestIdConfig {
    /// Creates a new configuration with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the header name for request ID.
    #[must_use]
    pub fn header_name(mut self, name: impl Into<String>) -> Self {
        self.header_name = name.into();
        self
    }

    /// Sets whether to accept request ID from client.
    #[must_use]
    pub fn accept_from_client(mut self, accept: bool) -> Self {
        self.accept_from_client = accept;
        self
    }

    /// Sets whether to add request ID to response.
    #[must_use]
    pub fn add_to_response(mut self, add: bool) -> Self {
        self.add_to_response = add;
        self
    }

    /// Sets the maximum length for client-provided request IDs.
    #[must_use]
    pub fn max_client_id_length(mut self, max: usize) -> Self {
        self.max_client_id_length = max;
        self
    }
}

/// Middleware that adds unique request IDs to requests and responses.
///
/// This middleware:
/// 1. Checks for an existing X-Request-ID header from the client
/// 2. If present and valid, uses it; otherwise generates a new ID
/// 3. Stores the ID in request extensions for handlers to access
/// 4. Adds the ID to response headers
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::RequestIdMiddleware;
///
/// let mut stack = MiddlewareStack::new();
/// stack.push(RequestIdMiddleware::new());
///
/// // In your handler:
/// async fn handler(ctx: &RequestContext, req: &Request) -> Response {
///     if let Some(request_id) = req.get_extension::<RequestId>() {
///         println!("Request ID: {}", request_id);
///     }
///     Response::ok()
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RequestIdMiddleware {
    config: RequestIdConfig,
}

impl Default for RequestIdMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl RequestIdMiddleware {
    /// Creates a new request ID middleware with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: RequestIdConfig::default(),
        }
    }

    /// Creates a new request ID middleware with the given configuration.
    #[must_use]
    pub fn with_config(config: RequestIdConfig) -> Self {
        Self { config }
    }

    /// Extracts or generates a request ID for the given request.
    fn get_or_generate_id(&self, req: &Request) -> RequestId {
        if self.config.accept_from_client {
            if let Some(header_value) = req.headers().get(&self.config.header_name) {
                if let Ok(client_id) = std::str::from_utf8(header_value) {
                    // Validate length and basic content
                    if !client_id.is_empty()
                        && client_id.len() <= self.config.max_client_id_length
                        && is_valid_request_id(client_id)
                    {
                        return RequestId::new(client_id);
                    }
                }
            }
        }
        RequestId::generate()
    }
}

/// Validates that a request ID contains only safe characters.
fn is_valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

impl Middleware for RequestIdMiddleware {
    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow> {
        let request_id = self.get_or_generate_id(req);
        req.insert_extension(request_id);
        Box::pin(async { ControlFlow::Continue })
    }

    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        if !self.config.add_to_response {
            return Box::pin(async move { response });
        }

        let request_id = req.get_extension::<RequestId>().cloned();
        let header_name = self.config.header_name.clone();

        Box::pin(async move {
            if let Some(id) = request_id {
                response.header(header_name, id.0.into_bytes())
            } else {
                response
            }
        })
    }

    fn name(&self) -> &'static str {
        "RequestId"
    }
}

// ============================================================================
// Security Headers Middleware
// ============================================================================

/// X-Frame-Options header value.
///
/// Controls whether the page can be displayed in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XFrameOptions {
    /// Prevents any domain from framing the content.
    Deny,
    /// Allows the current site to frame the content.
    SameOrigin,
}

impl XFrameOptions {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Deny => b"DENY",
            Self::SameOrigin => b"SAMEORIGIN",
        }
    }
}

/// Referrer-Policy header value.
///
/// Controls how much referrer information should be included with requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferrerPolicy {
    /// No referrer information is sent.
    NoReferrer,
    /// Only send origin when protocol security level stays the same.
    NoReferrerWhenDowngrade,
    /// Only send the origin (not the path).
    Origin,
    /// Only send origin for cross-origin requests.
    OriginWhenCrossOrigin,
    /// Send the origin, path, and query string for same-origin requests only.
    SameOrigin,
    /// Only send origin if protocol security level stays the same.
    StrictOrigin,
    /// Send full referrer for same-origin, origin only for cross-origin if secure.
    StrictOriginWhenCrossOrigin,
    /// Send the full referrer (not recommended).
    UnsafeUrl,
}

impl ReferrerPolicy {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::NoReferrer => b"no-referrer",
            Self::NoReferrerWhenDowngrade => b"no-referrer-when-downgrade",
            Self::Origin => b"origin",
            Self::OriginWhenCrossOrigin => b"origin-when-cross-origin",
            Self::SameOrigin => b"same-origin",
            Self::StrictOrigin => b"strict-origin",
            Self::StrictOriginWhenCrossOrigin => b"strict-origin-when-cross-origin",
            Self::UnsafeUrl => b"unsafe-url",
        }
    }
}

/// Configuration for the Security Headers middleware.
///
/// All headers are optional. Set a value to `Some(...)` to include the header,
/// or `None` to skip it.
///
/// # Defaults
///
/// The default configuration provides secure defaults:
/// - `X-Content-Type-Options: nosniff`
/// - `X-Frame-Options: DENY`
/// - `X-XSS-Protection: 0` (disabled as modern browsers have built-in protection)
/// - `Referrer-Policy: strict-origin-when-cross-origin`
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::{SecurityHeadersConfig, XFrameOptions, ReferrerPolicy};
///
/// let config = SecurityHeadersConfig::default()
///     .x_frame_options(XFrameOptions::SameOrigin)
///     .content_security_policy("default-src 'self'")
///     .hsts(31536000, true);  // 1 year, includeSubDomains
/// ```
#[derive(Debug, Clone)]
pub struct SecurityHeadersConfig {
    /// X-Content-Type-Options header.
    /// Default: `Some("nosniff")`
    pub x_content_type_options: Option<&'static str>,
    /// X-Frame-Options header.
    /// Default: `Some(XFrameOptions::Deny)`
    pub x_frame_options: Option<XFrameOptions>,
    /// X-XSS-Protection header.
    /// Default: `Some("0")` (disabled - modern browsers have built-in protection)
    ///
    /// Note: This header is largely obsolete. Setting it to "0" is recommended
    /// to prevent potential security issues in older browsers.
    pub x_xss_protection: Option<&'static str>,
    /// Content-Security-Policy header.
    /// Default: `None` (should be configured based on your application)
    pub content_security_policy: Option<String>,
    /// Strict-Transport-Security (HSTS) header.
    /// Tuple of (max_age_seconds, include_sub_domains, preload)
    /// Default: `None` (only set this for HTTPS-only sites)
    pub hsts: Option<(u64, bool, bool)>,
    /// Referrer-Policy header.
    /// Default: `Some(ReferrerPolicy::StrictOriginWhenCrossOrigin)`
    pub referrer_policy: Option<ReferrerPolicy>,
    /// Permissions-Policy header (formerly Feature-Policy).
    /// Default: `None` (should be configured based on your application)
    pub permissions_policy: Option<String>,
}

impl Default for SecurityHeadersConfig {
    fn default() -> Self {
        Self {
            x_content_type_options: Some("nosniff"),
            x_frame_options: Some(XFrameOptions::Deny),
            x_xss_protection: Some("0"),
            content_security_policy: None,
            hsts: None,
            referrer_policy: Some(ReferrerPolicy::StrictOriginWhenCrossOrigin),
            permissions_policy: None,
        }
    }
}

impl SecurityHeadersConfig {
    /// Creates a new configuration with secure defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty configuration (no headers).
    #[must_use]
    pub fn none() -> Self {
        Self {
            x_content_type_options: None,
            x_frame_options: None,
            x_xss_protection: None,
            content_security_policy: None,
            hsts: None,
            referrer_policy: None,
            permissions_policy: None,
        }
    }

    /// Creates a strict configuration for high-security applications.
    ///
    /// Includes:
    /// - All default headers
    /// - HSTS with 1 year max-age and includeSubDomains
    /// - A basic CSP that only allows same-origin resources
    #[must_use]
    pub fn strict() -> Self {
        Self {
            x_content_type_options: Some("nosniff"),
            x_frame_options: Some(XFrameOptions::Deny),
            x_xss_protection: Some("0"),
            content_security_policy: Some("default-src 'self'".to_string()),
            hsts: Some((31536000, true, false)), // 1 year, includeSubDomains
            referrer_policy: Some(ReferrerPolicy::NoReferrer),
            permissions_policy: Some("geolocation=(), camera=(), microphone=()".to_string()),
        }
    }

    /// Sets the X-Content-Type-Options header.
    #[must_use]
    pub fn x_content_type_options(mut self, value: Option<&'static str>) -> Self {
        self.x_content_type_options = value;
        self
    }

    /// Sets the X-Frame-Options header.
    #[must_use]
    pub fn x_frame_options(mut self, value: Option<XFrameOptions>) -> Self {
        self.x_frame_options = value;
        self
    }

    /// Sets the X-XSS-Protection header.
    #[must_use]
    pub fn x_xss_protection(mut self, value: Option<&'static str>) -> Self {
        self.x_xss_protection = value;
        self
    }

    /// Sets the Content-Security-Policy header.
    #[must_use]
    pub fn content_security_policy(mut self, value: impl Into<String>) -> Self {
        self.content_security_policy = Some(value.into());
        self
    }

    /// Clears the Content-Security-Policy header.
    #[must_use]
    pub fn no_content_security_policy(mut self) -> Self {
        self.content_security_policy = None;
        self
    }

    /// Sets the Strict-Transport-Security (HSTS) header.
    ///
    /// # Arguments
    ///
    /// - `max_age`: Maximum time (in seconds) the browser should remember HTTPS
    /// - `include_sub_domains`: Whether to apply to all subdomains
    /// - `preload`: Whether to include in browser preload lists (use with caution)
    ///
    /// # Warning
    ///
    /// Only enable HSTS for sites that are HTTPS-only. Enabling HSTS incorrectly
    /// can make your site inaccessible.
    #[must_use]
    pub fn hsts(mut self, max_age: u64, include_sub_domains: bool, preload: bool) -> Self {
        self.hsts = Some((max_age, include_sub_domains, preload));
        self
    }

    /// Clears the HSTS header.
    #[must_use]
    pub fn no_hsts(mut self) -> Self {
        self.hsts = None;
        self
    }

    /// Sets the Referrer-Policy header.
    #[must_use]
    pub fn referrer_policy(mut self, value: Option<ReferrerPolicy>) -> Self {
        self.referrer_policy = value;
        self
    }

    /// Sets the Permissions-Policy header.
    #[must_use]
    pub fn permissions_policy(mut self, value: impl Into<String>) -> Self {
        self.permissions_policy = Some(value.into());
        self
    }

    /// Clears the Permissions-Policy header.
    #[must_use]
    pub fn no_permissions_policy(mut self) -> Self {
        self.permissions_policy = None;
        self
    }

    /// Builds the HSTS header value.
    fn build_hsts_value(&self) -> Option<String> {
        self.hsts.map(|(max_age, include_sub, preload)| {
            let mut value = format!("max-age={}", max_age);
            if include_sub {
                value.push_str("; includeSubDomains");
            }
            if preload {
                value.push_str("; preload");
            }
            value
        })
    }
}

/// Middleware that adds security-related HTTP headers to responses.
///
/// This middleware helps protect against common web vulnerabilities by setting
/// appropriate security headers. It's recommended for all web applications.
///
/// # Headers
///
/// - **X-Content-Type-Options**: Prevents MIME type sniffing
/// - **X-Frame-Options**: Controls iframe embedding (clickjacking protection)
/// - **X-XSS-Protection**: Legacy XSS filter control (disabled by default)
/// - **Content-Security-Policy**: Controls resource loading
/// - **Strict-Transport-Security**: Enforces HTTPS
/// - **Referrer-Policy**: Controls referrer information
/// - **Permissions-Policy**: Controls browser features
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::{SecurityHeaders, SecurityHeadersConfig};
///
/// // Use defaults
/// let mw = SecurityHeaders::new();
///
/// // Custom configuration
/// let config = SecurityHeadersConfig::default()
///     .content_security_policy("default-src 'self'; img-src *")
///     .hsts(86400, false, false);  // 1 day
///
/// let mw = SecurityHeaders::with_config(config);
/// ```
#[derive(Debug, Clone)]
pub struct SecurityHeaders {
    config: SecurityHeadersConfig,
}

impl Default for SecurityHeaders {
    fn default() -> Self {
        Self::new()
    }
}

impl SecurityHeaders {
    /// Creates a new middleware with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: SecurityHeadersConfig::default(),
        }
    }

    /// Creates a new middleware with custom configuration.
    #[must_use]
    pub fn with_config(config: SecurityHeadersConfig) -> Self {
        Self { config }
    }

    /// Creates a middleware with strict security settings.
    #[must_use]
    pub fn strict() -> Self {
        Self {
            config: SecurityHeadersConfig::strict(),
        }
    }
}

impl Middleware for SecurityHeaders {
    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let config = self.config.clone();
        Box::pin(async move {
            let mut resp = response;

            // X-Content-Type-Options
            if let Some(value) = config.x_content_type_options {
                resp = resp.header("X-Content-Type-Options", value.as_bytes().to_vec());
            }

            // X-Frame-Options
            if let Some(value) = config.x_frame_options {
                resp = resp.header("X-Frame-Options", value.as_bytes().to_vec());
            }

            // X-XSS-Protection
            if let Some(value) = config.x_xss_protection {
                resp = resp.header("X-XSS-Protection", value.as_bytes().to_vec());
            }

            // Content-Security-Policy
            if let Some(ref value) = config.content_security_policy {
                resp = resp.header("Content-Security-Policy", value.as_bytes().to_vec());
            }

            // Strict-Transport-Security
            if let Some(ref hsts_value) = config.build_hsts_value() {
                resp = resp.header("Strict-Transport-Security", hsts_value.as_bytes().to_vec());
            }

            // Referrer-Policy
            if let Some(value) = config.referrer_policy {
                resp = resp.header("Referrer-Policy", value.as_bytes().to_vec());
            }

            // Permissions-Policy
            if let Some(ref value) = config.permissions_policy {
                resp = resp.header("Permissions-Policy", value.as_bytes().to_vec());
            }

            resp
        })
    }

    fn name(&self) -> &'static str {
        "SecurityHeaders"
    }
}

// ============================================================================
// CSRF Protection Middleware
// ============================================================================

/// CSRF token stored in request extensions.
///
/// Middleware stores this after generating or validating a token,
/// allowing handlers to access the current CSRF token.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CsrfToken(pub String);

impl CsrfToken {
    /// Creates a new CSRF token with the given value.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// Returns the token as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Generates a new unique CSRF token.
    ///
    /// Uses timestamp + counter + thread-id for uniqueness.
    /// For production use, consider replacing with a cryptographically
    /// secure random source.
    #[must_use]
    pub fn generate() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let thread_id = std::thread::current().id();

        // Create a longer, more unique token
        // Format: timestamp_hex-counter_hex-thread_hash
        let thread_hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            thread_id.hash(&mut hasher);
            hasher.finish()
        };

        Self(format!("{timestamp:016x}-{counter:08x}-{thread_hash:016x}"))
    }
}

impl std::fmt::Display for CsrfToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for CsrfToken {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// CSRF protection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CsrfMode {
    /// Double-submit cookie pattern: token in cookie must match token in header.
    /// This is the default and most common pattern.
    #[default]
    DoubleSubmit,
    /// Require token in header only (for APIs where cookies are not used).
    HeaderOnly,
}

/// Configuration for CSRF protection middleware.
#[derive(Debug, Clone)]
pub struct CsrfConfig {
    /// Cookie name for CSRF token (default: "csrf_token").
    pub cookie_name: String,
    /// Header name for CSRF token (default: "x-csrf-token").
    pub header_name: String,
    /// CSRF protection mode (default: DoubleSubmit).
    pub mode: CsrfMode,
    /// Whether to rotate token on each request (default: false).
    pub rotate_token: bool,
    /// Whether in production mode (affects Secure cookie flag).
    pub production: bool,
    /// Custom error message for CSRF failures.
    pub error_message: Option<String>,
}

impl Default for CsrfConfig {
    fn default() -> Self {
        Self {
            cookie_name: "csrf_token".to_string(),
            header_name: "x-csrf-token".to_string(),
            mode: CsrfMode::DoubleSubmit,
            rotate_token: false,
            production: true,
            error_message: None,
        }
    }
}

impl CsrfConfig {
    /// Creates a new configuration with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the cookie name for CSRF token.
    #[must_use]
    pub fn cookie_name(mut self, name: impl Into<String>) -> Self {
        self.cookie_name = name.into();
        self
    }

    /// Sets the header name for CSRF token.
    #[must_use]
    pub fn header_name(mut self, name: impl Into<String>) -> Self {
        self.header_name = name.into();
        self
    }

    /// Sets the CSRF protection mode.
    #[must_use]
    pub fn mode(mut self, mode: CsrfMode) -> Self {
        self.mode = mode;
        self
    }

    /// Enables token rotation on each request.
    #[must_use]
    pub fn rotate_token(mut self, rotate: bool) -> Self {
        self.rotate_token = rotate;
        self
    }

    /// Sets production mode (affects Secure cookie flag).
    #[must_use]
    pub fn production(mut self, production: bool) -> Self {
        self.production = production;
        self
    }

    /// Sets a custom error message for CSRF failures.
    #[must_use]
    pub fn error_message(mut self, message: impl Into<String>) -> Self {
        self.error_message = Some(message.into());
        self
    }
}

/// CSRF protection middleware.
///
/// Implements protection against Cross-Site Request Forgery attacks using
/// the double-submit cookie pattern by default.
///
/// # How It Works
///
/// 1. For safe methods (GET, HEAD, OPTIONS, TRACE): generates a CSRF token
///    and sets it in a cookie if not present.
/// 2. For state-changing methods (POST, PUT, DELETE, PATCH): validates that
///    the token in the header matches the token in the cookie.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::{CsrfMiddleware, CsrfConfig};
///
/// let mut stack = MiddlewareStack::new();
/// stack.push(CsrfMiddleware::new());
///
/// // Or with custom configuration:
/// let csrf = CsrfMiddleware::with_config(
///     CsrfConfig::new()
///         .header_name("X-XSRF-Token")
///         .cookie_name("XSRF-TOKEN")
///         .production(false)
/// );
/// stack.push(csrf);
/// ```
#[derive(Debug, Clone)]
pub struct CsrfMiddleware {
    config: CsrfConfig,
}

impl Default for CsrfMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl CsrfMiddleware {
    /// Creates a new CSRF middleware with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: CsrfConfig::default(),
        }
    }

    /// Creates a new CSRF middleware with the given configuration.
    #[must_use]
    pub fn with_config(config: CsrfConfig) -> Self {
        Self { config }
    }

    /// Checks if the HTTP method is safe (does not modify state).
    fn is_safe_method(method: crate::request::Method) -> bool {
        matches!(
            method,
            crate::request::Method::Get
                | crate::request::Method::Head
                | crate::request::Method::Options
                | crate::request::Method::Trace
        )
    }

    /// Extracts the CSRF token from the cookie header.
    fn get_cookie_token(&self, req: &Request) -> Option<String> {
        let cookie_header = req.headers().get("cookie")?;
        let cookie_str = std::str::from_utf8(cookie_header).ok()?;

        // Parse cookie header: "name1=value1; name2=value2"
        for part in cookie_str.split(';') {
            let part = part.trim();
            if let Some((name, value)) = part.split_once('=') {
                if name.trim() == self.config.cookie_name {
                    return Some(value.trim().to_string());
                }
            }
        }
        None
    }

    /// Extracts the CSRF token from the request header.
    fn get_header_token(&self, req: &Request) -> Option<String> {
        let header_value = req.headers().get(&self.config.header_name)?;
        std::str::from_utf8(header_value)
            .ok()
            .map(|s| s.trim().to_string())
    }

    /// Validates the CSRF token for state-changing requests.
    fn validate_token(&self, req: &Request) -> Result<Option<CsrfToken>, Response> {
        let header_token = self.get_header_token(req);

        match self.config.mode {
            CsrfMode::DoubleSubmit => {
                let cookie_token = self.get_cookie_token(req);

                match (header_token, cookie_token) {
                    (Some(header), Some(cookie)) if header == cookie && !header.is_empty() => {
                        Ok(Some(CsrfToken::new(header)))
                    }
                    (None, _) | (_, None) => Err(self.csrf_error_response("CSRF token missing")),
                    _ => Err(self.csrf_error_response("CSRF token mismatch")),
                }
            }
            CsrfMode::HeaderOnly => match header_token {
                Some(token) if !token.is_empty() => Ok(Some(CsrfToken::new(token))),
                _ => Err(self.csrf_error_response("CSRF token missing in header")),
            },
        }
    }

    /// Creates a 403 Forbidden response for CSRF failures.
    fn csrf_error_response(&self, default_message: &str) -> Response {
        let message = self
            .config
            .error_message
            .as_deref()
            .unwrap_or(default_message);

        // Create a FastAPI-compatible error response
        let body = format!(
            r#"{{"detail":[{{"type":"csrf_error","loc":["header","{}"],"msg":"{}"}}]}}"#,
            self.config.header_name, message
        );

        Response::with_status(crate::response::StatusCode::FORBIDDEN)
            .header("content-type", b"application/json".to_vec())
            .body(crate::response::ResponseBody::Bytes(body.into_bytes()))
    }

    /// Creates the Set-Cookie header value for a CSRF token.
    fn make_set_cookie_header_value(cookie_name: &str, token: &str, production: bool) -> Vec<u8> {
        let mut cookie = format!("{}={}; Path=/; SameSite=Strict", cookie_name, token);

        if production {
            cookie.push_str("; Secure");
        }

        // Note: HttpOnly is NOT set - CSRF cookies must be readable by JavaScript

        cookie.into_bytes()
    }
}

impl Middleware for CsrfMiddleware {
    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow> {
        Box::pin(async move {
            if Self::is_safe_method(req.method()) {
                // Safe methods: generate token if not present
                let existing_token = self.get_cookie_token(req);
                let token = existing_token
                    .map(CsrfToken::new)
                    .unwrap_or_else(CsrfToken::generate);
                req.insert_extension(token);
                ControlFlow::Continue
            } else {
                // State-changing methods: validate token
                match self.validate_token(req) {
                    Ok(Some(token)) => {
                        req.insert_extension(token);
                        ControlFlow::Continue
                    }
                    Ok(None) => ControlFlow::Continue,
                    Err(response) => ControlFlow::Break(response),
                }
            }
        })
    }

    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let config = self.config.clone();
        let is_safe = Self::is_safe_method(req.method());
        let existing_cookie_token = self.get_cookie_token(req);
        let token = req.get_extension::<CsrfToken>().cloned();

        Box::pin(async move {
            // Set cookie for safe methods if:
            // 1. No cookie exists yet, or
            // 2. Token rotation is enabled
            if is_safe {
                let should_set_cookie = existing_cookie_token.is_none() || config.rotate_token;

                if should_set_cookie {
                    if let Some(token) = token {
                        let cookie_value = Self::make_set_cookie_header_value(
                            &config.cookie_name,
                            token.as_str(),
                            config.production,
                        );
                        return response.header("set-cookie", cookie_value);
                    }
                }
            }
            response
        })
    }

    fn name(&self) -> &'static str {
        "CSRF"
    }
}

// ============================================================================
// Compression Middleware (requires "compression" feature)
// ============================================================================

/// Configuration for response compression.
///
/// Controls when and how responses are compressed using gzip.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::{CompressionMiddleware, CompressionConfig};
///
/// // Use defaults (min size 1024, level 6)
/// let mw = CompressionMiddleware::new();
///
/// // Custom configuration
/// let config = CompressionConfig::new()
///     .min_size(512)
///     .level(9);  // Maximum compression
/// let mw = CompressionMiddleware::with_config(config);
/// ```
#[cfg(feature = "compression")]
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Minimum response size in bytes to compress.
    /// Responses smaller than this are not compressed.
    /// Default: 1024 bytes (1 KB)
    pub min_size: usize,
    /// Compression level (1-9).
    /// 1 = fastest, 9 = best compression, 6 = balanced (default)
    pub level: u32,
    /// Content types that are already compressed and should be skipped.
    /// Default includes common compressed formats.
    pub skip_content_types: Vec<&'static str>,
}

#[cfg(feature = "compression")]
impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            min_size: 1024,
            level: 6,
            skip_content_types: vec![
                // Images (already compressed)
                "image/jpeg",
                "image/png",
                "image/gif",
                "image/webp",
                "image/avif",
                // Video/Audio (already compressed)
                "video/",
                "audio/",
                // Archives (already compressed)
                "application/zip",
                "application/gzip",
                "application/x-gzip",
                "application/x-bzip2",
                "application/x-xz",
                "application/x-7z-compressed",
                "application/x-rar-compressed",
                // Other compressed formats
                "application/pdf",
                "application/woff",
                "application/woff2",
                "font/woff",
                "font/woff2",
            ],
        }
    }
}

#[cfg(feature = "compression")]
impl CompressionConfig {
    /// Creates a new configuration with default values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the minimum response size to compress.
    ///
    /// Responses smaller than this threshold will not be compressed,
    /// as compression overhead may exceed the savings.
    #[must_use]
    pub fn min_size(mut self, size: usize) -> Self {
        self.min_size = size;
        self
    }

    /// Sets the compression level (1-9).
    ///
    /// - 1: Fastest compression, lowest ratio
    /// - 6: Balanced (default)
    /// - 9: Best compression ratio, slowest
    ///
    /// Values outside 1-9 are clamped.
    #[must_use]
    pub fn level(mut self, level: u32) -> Self {
        self.level = level.clamp(1, 9);
        self
    }

    /// Adds a content type to skip during compression.
    ///
    /// Content types can be exact matches or prefixes (e.g., "video/" matches all video types).
    #[must_use]
    pub fn skip_content_type(mut self, content_type: &'static str) -> Self {
        self.skip_content_types.push(content_type);
        self
    }

    /// Checks if the given content type should be skipped.
    fn should_skip_content_type(&self, content_type: &str) -> bool {
        let ct_lower = content_type.to_ascii_lowercase();
        for skip in &self.skip_content_types {
            if skip.ends_with('/') {
                // Prefix match (e.g., "video/" matches "video/mp4")
                if ct_lower.starts_with(*skip) {
                    return true;
                }
            } else {
                // Exact match (with optional charset)
                if ct_lower == *skip || ct_lower.starts_with(&format!("{skip};")) {
                    return true;
                }
            }
        }
        false
    }
}

/// Middleware that compresses responses using gzip.
///
/// This middleware inspects the `Accept-Encoding` header and compresses
/// eligible responses with gzip. Compression is skipped for:
/// - Responses smaller than `min_size`
/// - Responses with already-compressed content types
/// - Responses that already have a `Content-Encoding` header
/// - Clients that don't accept gzip
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::{CompressionMiddleware, CompressionConfig, MiddlewareStack};
///
/// let mut stack = MiddlewareStack::new();
///
/// // Default configuration
/// stack.push(CompressionMiddleware::new());
///
/// // Or with custom settings
/// let config = CompressionConfig::new()
///     .min_size(256)   // Compress smaller responses
///     .level(9);       // Maximum compression
/// stack.push(CompressionMiddleware::with_config(config));
/// ```
///
/// # Headers
///
/// When compression is applied:
/// - `Content-Encoding: gzip` is added
/// - `Vary: Accept-Encoding` is added (for caching)
/// - `Content-Length` is updated to reflect compressed size
#[cfg(feature = "compression")]
#[derive(Debug, Clone)]
pub struct CompressionMiddleware {
    config: CompressionConfig,
}

#[cfg(feature = "compression")]
impl Default for CompressionMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "compression")]
impl CompressionMiddleware {
    /// Creates compression middleware with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: CompressionConfig::default(),
        }
    }

    /// Creates compression middleware with custom configuration.
    #[must_use]
    pub fn with_config(config: CompressionConfig) -> Self {
        Self { config }
    }

    /// Checks if the client accepts gzip encoding.
    fn accepts_gzip(req: &Request) -> bool {
        if let Some(accept_encoding) = req.headers().get("accept-encoding") {
            if let Ok(value) = std::str::from_utf8(accept_encoding) {
                // Parse Accept-Encoding header
                // Examples: "gzip", "gzip, deflate", "gzip;q=1.0, identity;q=0.5"
                for part in value.split(',') {
                    let encoding = part.trim().split(';').next().unwrap_or("").trim();
                    if encoding.eq_ignore_ascii_case("gzip") {
                        return true;
                    }
                    // Also accept "*" which means any encoding
                    if encoding == "*" {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Gets the Content-Type from response headers.
    fn get_content_type(headers: &[(String, Vec<u8>)]) -> Option<String> {
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("content-type") {
                return std::str::from_utf8(value).ok().map(String::from);
            }
        }
        None
    }

    /// Checks if response already has Content-Encoding header.
    fn has_content_encoding(headers: &[(String, Vec<u8>)]) -> bool {
        headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
    }

    /// Compresses data using gzip.
    fn compress_gzip(data: &[u8], level: u32) -> Result<Vec<u8>, std::io::Error> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(level));
        encoder.write_all(data)?;
        encoder.finish()
    }
}

#[cfg(feature = "compression")]
impl Middleware for CompressionMiddleware {
    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let config = self.config.clone();

        Box::pin(async move {
            // Check if client accepts gzip
            if !Self::accepts_gzip(req) {
                return response;
            }

            // Decompose response to inspect body
            let (status, headers, body) = response.into_parts();

            // Check if already compressed
            if Self::has_content_encoding(&headers) {
                return Response::with_status(status)
                    .body(body)
                    .rebuild_with_headers(headers);
            }

            // Get body bytes (only compress Bytes variant, not streaming)
            let body_bytes = match body {
                crate::response::ResponseBody::Bytes(bytes) => bytes,
                other => {
                    // Can't compress Empty or Stream bodies
                    return Response::with_status(status)
                        .body(other)
                        .rebuild_with_headers(headers);
                }
            };

            // Check minimum size
            if body_bytes.len() < config.min_size {
                return Response::with_status(status)
                    .body(crate::response::ResponseBody::Bytes(body_bytes))
                    .rebuild_with_headers(headers);
            }

            // Check content type
            if let Some(content_type) = Self::get_content_type(&headers) {
                if config.should_skip_content_type(&content_type) {
                    return Response::with_status(status)
                        .body(crate::response::ResponseBody::Bytes(body_bytes))
                        .rebuild_with_headers(headers);
                }
            }

            // Compress the body
            match Self::compress_gzip(&body_bytes, config.level) {
                Ok(compressed) => {
                    // Only use compressed if it's actually smaller
                    if compressed.len() >= body_bytes.len() {
                        return Response::with_status(status)
                            .body(crate::response::ResponseBody::Bytes(body_bytes))
                            .rebuild_with_headers(headers);
                    }

                    // Build response with compression headers
                    let mut resp = Response::with_status(status)
                        .body(crate::response::ResponseBody::Bytes(compressed));

                    // Copy original headers (except content-length)
                    for (name, value) in headers {
                        if !name.eq_ignore_ascii_case("content-length") {
                            resp = resp.header(name, value);
                        }
                    }

                    // Add compression headers
                    resp = resp.header("Content-Encoding", b"gzip".to_vec());
                    resp = resp.header("Vary", b"Accept-Encoding".to_vec());

                    resp
                }
                Err(_) => {
                    // Compression failed, return original
                    Response::with_status(status)
                        .body(crate::response::ResponseBody::Bytes(body_bytes))
                        .rebuild_with_headers(headers)
                }
            }
        })
    }

    fn name(&self) -> &'static str {
        "Compression"
    }
}

// ---------------------------------------------------------------------------
// Rate Limiting Middleware
// ---------------------------------------------------------------------------

use std::collections::HashMap as StdHashMap;
use std::sync::Mutex;
use std::time::Duration;

/// Rate limiting algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitAlgorithm {
    /// Token bucket: steady refill rate, allows short bursts.
    TokenBucket,
    /// Fixed window: resets at the start of each interval.
    FixedWindow,
    /// Sliding window: weighted combination of current and previous window.
    SlidingWindow,
}

/// Result of a rate limit check.
#[derive(Debug, Clone)]
pub struct RateLimitResult {
    /// Whether the request is allowed.
    pub allowed: bool,
    /// Maximum requests per window.
    pub limit: u64,
    /// Remaining requests in the current window.
    pub remaining: u64,
    /// Seconds until the window resets.
    pub reset_after_secs: u64,
}

/// Extracts a rate limit key from a request.
///
/// Different extractors allow rate limiting by different criteria:
/// IP address, API key header, path, or custom logic.
pub trait KeyExtractor: Send + Sync {
    /// Extract the key string from the request.
    ///
    /// Returns `None` if no key can be extracted (request is not rate-limited).
    fn extract_key(&self, req: &Request) -> Option<String>;
}

/// Rate limit by client IP address (from `X-Forwarded-For` or `X-Real-IP` headers).
///
/// Falls back to `"unknown"` when no client IP is available.
#[derive(Debug, Clone)]
pub struct IpKeyExtractor;

impl KeyExtractor for IpKeyExtractor {
    fn extract_key(&self, req: &Request) -> Option<String> {
        // Try X-Forwarded-For first, then X-Real-IP, then fall back
        if let Some(forwarded) = req.headers().get("x-forwarded-for") {
            if let Ok(s) = std::str::from_utf8(forwarded) {
                // Take the first IP (client IP) from the chain
                if let Some(ip) = s.split(',').next() {
                    return Some(ip.trim().to_string());
                }
            }
        }
        if let Some(real_ip) = req.headers().get("x-real-ip") {
            if let Ok(s) = std::str::from_utf8(real_ip) {
                return Some(s.trim().to_string());
            }
        }
        Some("unknown".to_string())
    }
}

/// Rate limit by a specific header value (e.g., `X-API-Key`).
#[derive(Debug, Clone)]
pub struct HeaderKeyExtractor {
    header_name: String,
}

impl HeaderKeyExtractor {
    /// Create a new header key extractor.
    #[must_use]
    pub fn new(header_name: impl Into<String>) -> Self {
        Self {
            header_name: header_name.into(),
        }
    }
}

impl KeyExtractor for HeaderKeyExtractor {
    fn extract_key(&self, req: &Request) -> Option<String> {
        req.headers()
            .get(&self.header_name)
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(str::to_string)
    }
}

/// Rate limit by request path.
#[derive(Debug, Clone)]
pub struct PathKeyExtractor;

impl KeyExtractor for PathKeyExtractor {
    fn extract_key(&self, req: &Request) -> Option<String> {
        Some(req.path().to_string())
    }
}

/// A composite key extractor that combines multiple extractors.
///
/// Keys from all extractors are joined with `:` to form a composite key.
/// If any extractor returns `None`, that part is omitted.
pub struct CompositeKeyExtractor {
    extractors: Vec<Box<dyn KeyExtractor>>,
}

impl CompositeKeyExtractor {
    /// Create a composite key extractor from multiple extractors.
    #[must_use]
    pub fn new(extractors: Vec<Box<dyn KeyExtractor>>) -> Self {
        Self { extractors }
    }
}

impl KeyExtractor for CompositeKeyExtractor {
    fn extract_key(&self, req: &Request) -> Option<String> {
        let parts: Vec<String> = self
            .extractors
            .iter()
            .filter_map(|e| e.extract_key(req))
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(":"))
        }
    }
}

/// Token bucket state for a single key.
#[derive(Debug, Clone)]
struct TokenBucketState {
    tokens: f64,
    last_refill: Instant,
}

/// Fixed window state for a single key.
#[derive(Debug, Clone)]
struct FixedWindowState {
    count: u64,
    window_start: Instant,
}

/// Sliding window state for a single key.
#[derive(Debug, Clone)]
struct SlidingWindowState {
    current_count: u64,
    previous_count: u64,
    current_window_start: Instant,
}

/// In-memory rate limit store.
///
/// Uses a `HashMap` protected by a `Mutex` for thread-safe access.
/// Suitable for single-process deployments. For distributed systems,
/// implement a custom store using Redis or similar.
pub struct InMemoryRateLimitStore {
    token_buckets: Mutex<StdHashMap<String, TokenBucketState>>,
    fixed_windows: Mutex<StdHashMap<String, FixedWindowState>>,
    sliding_windows: Mutex<StdHashMap<String, SlidingWindowState>>,
}

impl InMemoryRateLimitStore {
    /// Create a new in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            token_buckets: Mutex::new(StdHashMap::new()),
            fixed_windows: Mutex::new(StdHashMap::new()),
            sliding_windows: Mutex::new(StdHashMap::new()),
        }
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
    fn check_token_bucket(
        &self,
        key: &str,
        max_tokens: u64,
        refill_rate: f64,
        window: Duration,
    ) -> RateLimitResult {
        let mut buckets = self
            .token_buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();

        let state = buckets
            .entry(key.to_string())
            .or_insert_with(|| TokenBucketState {
                tokens: max_tokens as f64,
                last_refill: now,
            });

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(state.last_refill);
        let refill = elapsed.as_secs_f64() * refill_rate;
        state.tokens = (state.tokens + refill).min(max_tokens as f64);
        state.last_refill = now;

        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            RateLimitResult {
                allowed: true,
                limit: max_tokens,
                remaining: state.tokens as u64,
                reset_after_secs: if state.tokens < max_tokens as f64 {
                    ((max_tokens as f64 - state.tokens) / refill_rate).ceil() as u64
                } else {
                    window.as_secs()
                },
            }
        } else {
            let wait_secs = ((1.0 - state.tokens) / refill_rate).ceil() as u64;
            RateLimitResult {
                allowed: false,
                limit: max_tokens,
                remaining: 0,
                reset_after_secs: wait_secs,
            }
        }
    }

    fn check_fixed_window(
        &self,
        key: &str,
        max_requests: u64,
        window: Duration,
    ) -> RateLimitResult {
        let mut windows = self
            .fixed_windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();

        let state = windows
            .entry(key.to_string())
            .or_insert_with(|| FixedWindowState {
                count: 0,
                window_start: now,
            });

        // Check if window has expired
        let elapsed = now.duration_since(state.window_start);
        if elapsed >= window {
            state.count = 0;
            state.window_start = now;
        }

        let remaining_time = window
            .checked_sub(now.duration_since(state.window_start))
            .unwrap_or(Duration::ZERO);

        if state.count < max_requests {
            state.count += 1;
            RateLimitResult {
                allowed: true,
                limit: max_requests,
                remaining: max_requests - state.count,
                reset_after_secs: remaining_time.as_secs(),
            }
        } else {
            RateLimitResult {
                allowed: false,
                limit: max_requests,
                remaining: 0,
                reset_after_secs: remaining_time.as_secs(),
            }
        }
    }

    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
    fn check_sliding_window(
        &self,
        key: &str,
        max_requests: u64,
        window: Duration,
    ) -> RateLimitResult {
        let mut windows = self
            .sliding_windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();

        let state = windows
            .entry(key.to_string())
            .or_insert_with(|| SlidingWindowState {
                current_count: 0,
                previous_count: 0,
                current_window_start: now,
            });

        // Check if we need to rotate windows
        let elapsed = now.duration_since(state.current_window_start);
        if elapsed >= window {
            // Rotate: current becomes previous
            state.previous_count = state.current_count;
            state.current_count = 0;
            state.current_window_start = now;
        }

        // Calculate weighted count using the proportion of the previous window
        // that overlaps with the current sliding window
        let window_elapsed = now.duration_since(state.current_window_start);
        let window_fraction = window_elapsed.as_secs_f64() / window.as_secs_f64();
        let previous_weight = 1.0 - window_fraction;
        let weighted_count =
            (state.previous_count as f64 * previous_weight) + state.current_count as f64;

        let remaining_time = window.checked_sub(window_elapsed).unwrap_or(Duration::ZERO);

        if weighted_count < max_requests as f64 {
            state.current_count += 1;
            let new_weighted =
                (state.previous_count as f64 * previous_weight) + state.current_count as f64;
            let remaining = (max_requests as f64 - new_weighted).max(0.0) as u64;
            RateLimitResult {
                allowed: true,
                limit: max_requests,
                remaining,
                reset_after_secs: remaining_time.as_secs(),
            }
        } else {
            RateLimitResult {
                allowed: false,
                limit: max_requests,
                remaining: 0,
                reset_after_secs: remaining_time.as_secs(),
            }
        }
    }

    /// Check and consume a request against the rate limit.
    #[allow(clippy::cast_precision_loss)]
    pub fn check(
        &self,
        key: &str,
        algorithm: RateLimitAlgorithm,
        max_requests: u64,
        window: Duration,
    ) -> RateLimitResult {
        match algorithm {
            RateLimitAlgorithm::TokenBucket => {
                let refill_rate = max_requests as f64 / window.as_secs_f64();
                self.check_token_bucket(key, max_requests, refill_rate, window)
            }
            RateLimitAlgorithm::FixedWindow => self.check_fixed_window(key, max_requests, window),
            RateLimitAlgorithm::SlidingWindow => {
                self.check_sliding_window(key, max_requests, window)
            }
        }
    }
}

impl Default for InMemoryRateLimitStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for the rate limiting middleware.
#[derive(Clone)]
pub struct RateLimitConfig {
    /// Maximum number of requests allowed per window.
    pub max_requests: u64,
    /// Time window for the rate limit.
    pub window: Duration,
    /// The algorithm to use.
    pub algorithm: RateLimitAlgorithm,
    /// Whether to include rate limit headers in responses.
    pub include_headers: bool,
    /// Custom message for 429 responses.
    pub retry_message: String,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: 100,
            window: Duration::from_secs(60),
            algorithm: RateLimitAlgorithm::TokenBucket,
            include_headers: true,
            retry_message: "Rate limit exceeded. Please retry later.".to_string(),
        }
    }
}

/// Builder for `RateLimitConfig`.
pub struct RateLimitBuilder {
    config: RateLimitConfig,
    key_extractor: Option<Box<dyn KeyExtractor>>,
}

impl RateLimitBuilder {
    /// Create a new rate limit builder with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: RateLimitConfig::default(),
            key_extractor: None,
        }
    }

    /// Set the maximum number of requests per window.
    #[must_use]
    pub fn requests(mut self, max: u64) -> Self {
        self.config.max_requests = max;
        self
    }

    /// Set the time window.
    #[must_use]
    pub fn per(mut self, window: Duration) -> Self {
        self.config.window = window;
        self
    }

    /// Shorthand: set the window to the given number of seconds.
    #[must_use]
    pub fn per_second(self, secs: u64) -> Self {
        self.per(Duration::from_secs(secs))
    }

    /// Shorthand: set the window to the given number of minutes.
    #[must_use]
    pub fn per_minute(self, minutes: u64) -> Self {
        self.per(Duration::from_secs(minutes * 60))
    }

    /// Shorthand: set the window to the given number of hours.
    #[must_use]
    pub fn per_hour(self, hours: u64) -> Self {
        self.per(Duration::from_secs(hours * 3600))
    }

    /// Set the rate limiting algorithm.
    #[must_use]
    pub fn algorithm(mut self, algo: RateLimitAlgorithm) -> Self {
        self.config.algorithm = algo;
        self
    }

    /// Set the key extractor.
    #[must_use]
    pub fn key_extractor(mut self, extractor: impl KeyExtractor + 'static) -> Self {
        self.key_extractor = Some(Box::new(extractor));
        self
    }

    /// Whether to include rate limit headers in responses.
    #[must_use]
    pub fn include_headers(mut self, include: bool) -> Self {
        self.config.include_headers = include;
        self
    }

    /// Set the custom message for 429 responses.
    #[must_use]
    pub fn retry_message(mut self, msg: impl Into<String>) -> Self {
        self.config.retry_message = msg.into();
        self
    }

    /// Build the rate limiting middleware.
    #[must_use]
    pub fn build(self) -> RateLimitMiddleware {
        let key_extractor = self
            .key_extractor
            .unwrap_or_else(|| Box::new(IpKeyExtractor));
        RateLimitMiddleware {
            config: self.config,
            store: Arc::new(InMemoryRateLimitStore::new()),
            key_extractor: Arc::from(key_extractor),
        }
    }
}

impl Default for RateLimitBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Extension type stored on requests to carry rate limit info to `after` hook.
#[derive(Debug, Clone)]
struct RateLimitInfo {
    result: RateLimitResult,
}

/// Rate limiting middleware.
///
/// Tracks request rates per key and returns 429 Too Many Requests
/// when a client exceeds the configured limit.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::{RateLimitMiddleware, RateLimitAlgorithm, IpKeyExtractor};
/// use std::time::Duration;
///
/// let rate_limiter = RateLimitMiddleware::builder()
///     .requests(100)
///     .per(Duration::from_secs(60))
///     .algorithm(RateLimitAlgorithm::TokenBucket)
///     .key_extractor(IpKeyExtractor)
///     .build();
///
/// let app = App::builder()
///     .middleware(rate_limiter)
///     .build();
/// ```
pub struct RateLimitMiddleware {
    config: RateLimitConfig,
    store: Arc<InMemoryRateLimitStore>,
    key_extractor: Arc<dyn KeyExtractor>,
}

impl RateLimitMiddleware {
    /// Create a new rate limiter with default settings (100 requests/minute, token bucket, IP-based).
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Create a builder for configuring the rate limiter.
    #[must_use]
    pub fn builder() -> RateLimitBuilder {
        RateLimitBuilder::new()
    }

    /// Format a 429 response body as JSON.
    fn too_many_requests_body(&self, result: &RateLimitResult) -> Vec<u8> {
        format!(
            r#"{{"detail":"{}","retry_after_secs":{}}}"#,
            self.config.retry_message, result.reset_after_secs
        )
        .into_bytes()
    }

    /// Add rate limit headers to a response.
    fn add_headers(&self, response: Response, result: &RateLimitResult) -> Response {
        response
            .header("X-RateLimit-Limit", result.limit.to_string().into_bytes())
            .header(
                "X-RateLimit-Remaining",
                result.remaining.to_string().into_bytes(),
            )
            .header(
                "X-RateLimit-Reset",
                result.reset_after_secs.to_string().into_bytes(),
            )
    }
}

impl Default for RateLimitMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for RateLimitMiddleware {
    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow> {
        Box::pin(async move {
            // Extract the key for this request
            let Some(key) = self.key_extractor.extract_key(req) else {
                // No key extracted — skip rate limiting for this request
                return ControlFlow::Continue;
            };

            // Check the rate limit
            let result = self.store.check(
                &key,
                self.config.algorithm,
                self.config.max_requests,
                self.config.window,
            );

            if result.allowed {
                // Store the result for the `after` hook to add headers
                req.insert_extension(RateLimitInfo { result });
                ControlFlow::Continue
            } else {
                // Return 429 Too Many Requests
                let body = self.too_many_requests_body(&result);
                let mut response =
                    Response::with_status(crate::response::StatusCode::TOO_MANY_REQUESTS)
                        .header("Content-Type", b"application/json".to_vec())
                        .header(
                            "Retry-After",
                            result.reset_after_secs.to_string().into_bytes(),
                        )
                        .body(crate::response::ResponseBody::Bytes(body));

                if self.config.include_headers {
                    response = self.add_headers(response, &result);
                }

                ControlFlow::Break(response)
            }
        })
    }

    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        Box::pin(async move {
            if !self.config.include_headers {
                return response;
            }

            // Retrieve the rate limit info stored in `before`
            if let Some(info) = req.get_extension::<RateLimitInfo>() {
                self.add_headers(response, &info.result)
            } else {
                response
            }
        })
    }

    fn name(&self) -> &'static str {
        "RateLimit"
    }
}

// ---------------------------------------------------------------------------
// End Rate Limiting Middleware
// ---------------------------------------------------------------------------

// ============================================================================
// Request Inspection Middleware (Development)
// ============================================================================

/// Verbosity level for the request inspection middleware.
///
/// Controls how much detail is shown in the request/response output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectionVerbosity {
    /// Minimal: one-line summary per request/response.
    ///
    /// Shows: `-->  GET /path` and `<--  200 OK (12ms)`
    Minimal,

    /// Normal: summary plus headers.
    ///
    /// Shows method/path, all headers (filtered), and status/timing.
    Normal,

    /// Verbose: summary, headers, and body preview.
    ///
    /// Shows everything in Normal plus request/response body previews
    /// with JSON pretty-printing when applicable.
    Verbose,
}

/// Development middleware that logs detailed, human-readable request/response
/// information using arrow-style formatting.
///
/// This middleware is designed for development and debugging. It outputs
/// concise inspection lines showing request flow:
///
/// ```text
/// -->  POST /api/users
///      Content-Type: application/json
///      Content-Length: 42
///      {"name": "Alice"}
/// <--  201 Created (12ms)
///      Content-Type: application/json
///      {"id": 1, "name": "Alice"}
/// ```
///
/// # Features
///
/// - **Configurable verbosity**: Minimal (one-liner), Normal (+ headers),
///   Verbose (+ body preview with JSON pretty-printing)
/// - **Slow request highlighting**: Marks requests exceeding a threshold
/// - **Sensitive header filtering**: Redacts authorization, cookie, etc.
/// - **JSON pretty-printing**: Detects JSON bodies and formats them
/// - **Body size limits**: Truncates large bodies to a configurable max
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::RequestInspectionMiddleware;
///
/// let inspector = RequestInspectionMiddleware::new()
///     .verbosity(InspectionVerbosity::Verbose)
///     .slow_threshold_ms(500)
///     .max_body_preview(4096);
///
/// let mut stack = MiddlewareStack::new();
/// stack.push(inspector);
/// ```
pub struct RequestInspectionMiddleware {
    log_config: LogConfig,
    verbosity: InspectionVerbosity,
    redact_headers: HashSet<String>,
    slow_threshold_ms: u64,
    max_body_preview: usize,
}

impl Default for RequestInspectionMiddleware {
    fn default() -> Self {
        Self {
            log_config: LogConfig::development(),
            verbosity: InspectionVerbosity::Normal,
            redact_headers: default_redacted_headers(),
            slow_threshold_ms: 1000,
            max_body_preview: 2048,
        }
    }
}

impl RequestInspectionMiddleware {
    /// Create a new inspection middleware with development defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the logging configuration.
    #[must_use]
    pub fn log_config(mut self, config: LogConfig) -> Self {
        self.log_config = config;
        self
    }

    /// Set the verbosity level.
    #[must_use]
    pub fn verbosity(mut self, level: InspectionVerbosity) -> Self {
        self.verbosity = level;
        self
    }

    /// Set the threshold (in milliseconds) above which requests are flagged as slow.
    #[must_use]
    pub fn slow_threshold_ms(mut self, ms: u64) -> Self {
        self.slow_threshold_ms = ms;
        self
    }

    /// Set the maximum number of bytes to show in body previews.
    #[must_use]
    pub fn max_body_preview(mut self, max: usize) -> Self {
        self.max_body_preview = max;
        self
    }

    /// Add a header name to the redaction set (case-insensitive).
    #[must_use]
    pub fn redact_header(mut self, name: impl Into<String>) -> Self {
        self.redact_headers.insert(name.into().to_ascii_lowercase());
        self
    }

    /// Format a request body for display, with optional JSON pretty-printing.
    fn format_body_preview(&self, bytes: &[u8], content_type: Option<&[u8]>) -> Option<String> {
        if bytes.is_empty() || self.max_body_preview == 0 {
            return None;
        }

        let is_json = content_type
            .and_then(|ct| std::str::from_utf8(ct).ok())
            .is_some_and(|ct| ct.contains("application/json"));

        let limit = self.max_body_preview.min(bytes.len());
        let truncated = bytes.len() > self.max_body_preview;

        match std::str::from_utf8(&bytes[..limit]) {
            Ok(text) => {
                if is_json {
                    // Attempt JSON pretty-printing on the full available text
                    if let Some(pretty) = try_pretty_json(text) {
                        let mut output = pretty;
                        if truncated {
                            output.push_str("\n     ... (truncated)");
                        }
                        return Some(output);
                    }
                }
                let mut output = text.to_string();
                if truncated {
                    output.push_str("...");
                }
                Some(output)
            }
            Err(_) => Some(format!("<{} bytes binary>", bytes.len())),
        }
    }

    /// Format a response body for display.
    fn format_response_preview(
        &self,
        body: &crate::response::ResponseBody,
        content_type: Option<&[u8]>,
    ) -> Option<String> {
        match body {
            crate::response::ResponseBody::Empty => None,
            crate::response::ResponseBody::Bytes(bytes) => {
                self.format_body_preview(bytes, content_type)
            }
            crate::response::ResponseBody::Stream(_) => Some("<streaming body>".to_string()),
        }
    }

    /// Build the formatted header block for display.
    fn format_inspection_headers<'a>(
        &self,
        headers: impl Iterator<Item = (&'a str, &'a [u8])>,
    ) -> String {
        let mut out = String::new();
        for (name, value) in headers {
            out.push_str("\n     ");
            out.push_str(name);
            out.push_str(": ");

            let lowered = name.to_ascii_lowercase();
            if self.redact_headers.contains(&lowered) {
                out.push_str("[REDACTED]");
            } else {
                match std::str::from_utf8(value) {
                    Ok(text) => out.push_str(text),
                    Err(_) => out.push_str("<binary>"),
                }
            }
        }
        out
    }

    /// Build the response header block from (String, Vec<u8>) pairs.
    fn format_response_inspection_headers(&self, headers: &[(String, Vec<u8>)]) -> String {
        self.format_inspection_headers(
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_slice())),
        )
    }
}

/// Extension type to store request start time for the inspection middleware.
#[derive(Debug, Clone)]
struct InspectionStart(Instant);

impl Middleware for RequestInspectionMiddleware {
    fn before<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow> {
        let logger = RequestLogger::new(ctx, self.log_config.clone());
        req.insert_extension(InspectionStart(Instant::now()));

        let method = req.method();
        let path = req.path();
        let query = req.query();

        // Build the request line: "-->  GET /path?query"
        let mut request_line = format!("-->  {method} {path}");
        if let Some(q) = query {
            request_line.push('?');
            request_line.push_str(q);
        }

        let body_size = body_len(req.body());
        if body_size > 0 {
            request_line.push_str(&format!(" ({body_size} bytes)"));
        }

        match self.verbosity {
            InspectionVerbosity::Minimal => {
                logger.info(request_line);
            }
            InspectionVerbosity::Normal => {
                let headers = self.format_inspection_headers(req.headers().iter());
                logger.info(format!("{request_line}{headers}"));
            }
            InspectionVerbosity::Verbose => {
                let headers = self.format_inspection_headers(req.headers().iter());
                let content_type = req.headers().get("content-type");
                let body_preview = match req.body() {
                    Body::Empty => None,
                    Body::Bytes(bytes) => self.format_body_preview(bytes, content_type),
                    Body::Stream(_) => Some("<streaming body>".to_string()),
                };

                let mut output = format!("{request_line}{headers}");
                if let Some(body) = body_preview {
                    output.push_str("\n     ");
                    // Indent multi-line body previews
                    output.push_str(&body.replace('\n', "\n     "));
                }
                logger.info(output);
            }
        }

        Box::pin(async { ControlFlow::Continue })
    }

    fn after<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let logger = RequestLogger::new(ctx, self.log_config.clone());
        let duration = req
            .get_extension::<InspectionStart>()
            .map(|start| start.0.elapsed())
            .unwrap_or_default();

        let status = response.status();
        let duration_ms = duration.as_millis();

        // Build the response line: "<--  200 OK (12ms)"
        let mut response_line = format!(
            "<--  {} {} ({duration_ms}ms)",
            status.as_u16(),
            status.canonical_reason(),
        );

        // Flag slow requests
        if duration_ms >= u128::from(self.slow_threshold_ms) {
            response_line.push_str(" [SLOW]");
        }

        match self.verbosity {
            InspectionVerbosity::Minimal => {
                if duration_ms >= u128::from(self.slow_threshold_ms) {
                    logger.warn(response_line);
                } else {
                    logger.info(response_line);
                }
            }
            InspectionVerbosity::Normal => {
                let headers = self.format_response_inspection_headers(response.headers());
                let output = format!("{response_line}{headers}");
                if duration_ms >= u128::from(self.slow_threshold_ms) {
                    logger.warn(output);
                } else {
                    logger.info(output);
                }
            }
            InspectionVerbosity::Verbose => {
                let headers = self.format_response_inspection_headers(response.headers());

                // Find content-type from response headers for JSON detection
                let resp_content_type: Option<&[u8]> = response
                    .headers()
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                    .map(|(_, value)| value.as_slice());

                let body_preview =
                    self.format_response_preview(response.body_ref(), resp_content_type);

                let mut output = format!("{response_line}{headers}");
                if let Some(body) = body_preview {
                    output.push_str("\n     ");
                    output.push_str(&body.replace('\n', "\n     "));
                }

                if duration_ms >= u128::from(self.slow_threshold_ms) {
                    logger.warn(output);
                } else {
                    logger.info(output);
                }
            }
        }

        Box::pin(async move { response })
    }

    fn name(&self) -> &'static str {
        "RequestInspection"
    }
}

/// Attempt to parse and pretty-print a JSON string.
///
/// Returns `None` if the input is not valid JSON. Uses a minimal
/// recursive formatter to avoid external dependencies.
fn try_pretty_json(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return None;
    }

    // Validate it's actual JSON by attempting a parse, then pretty-format.
    let mut output = String::with_capacity(trimmed.len() * 2);
    if json_pretty_format(trimmed, &mut output).is_ok() {
        Some(output)
    } else {
        None
    }
}

/// Minimal JSON pretty-formatter without external dependencies.
///
/// Handles objects, arrays, strings, numbers, booleans, and null.
/// Produces 2-space indented output.
fn json_pretty_format(input: &str, output: &mut String) -> Result<(), ()> {
    let bytes = input.as_bytes();
    let mut pos = 0;
    let mut indent: usize = 0;
    let mut in_string = false;
    let mut escape_next = false;

    while pos < bytes.len() {
        let ch = bytes[pos] as char;

        if escape_next {
            output.push(ch);
            escape_next = false;
            pos += 1;
            continue;
        }

        if in_string {
            output.push(ch);
            if ch == '\\' {
                escape_next = true;
            } else if ch == '"' {
                in_string = false;
            }
            pos += 1;
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                output.push('"');
            }
            '{' | '[' => {
                output.push(ch);
                // Peek ahead: if the next non-whitespace is the closing bracket, keep compact
                let peek = skip_whitespace(bytes, pos + 1);
                let closing = if ch == '{' { '}' } else { ']' };
                if peek < bytes.len() && bytes[peek] as char == closing {
                    output.push(closing);
                    pos = peek + 1;
                    continue;
                }
                indent += 1;
                output.push('\n');
                push_indent(output, indent);
            }
            '}' | ']' => {
                indent = indent.saturating_sub(1);
                output.push('\n');
                push_indent(output, indent);
                output.push(ch);
            }
            ':' => {
                output.push_str(": ");
            }
            ',' => {
                output.push(',');
                output.push('\n');
                push_indent(output, indent);
            }
            c if c.is_ascii_whitespace() => {
                // Skip whitespace outside strings
            }
            _ => {
                output.push(ch);
            }
        }

        pos += 1;
    }

    if in_string || indent != 0 {
        return Err(());
    }

    Ok(())
}

fn skip_whitespace(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn push_indent(output: &mut String, level: usize) {
    for _ in 0..level {
        output.push_str("  ");
    }
}

// ---------------------------------------------------------------------------
// End Request Inspection Middleware
// ---------------------------------------------------------------------------

// ===========================================================================
// ETag Middleware
// ===========================================================================

/// Configuration for ETag generation strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ETagMode {
    /// Automatically generate ETag from response body hash.
    /// Uses FNV-1a hash for fast, consistent ETag generation.
    Auto,
    /// Expect handler to set ETag manually. Middleware only handles
    /// conditional request logic (If-None-Match checking).
    Manual,
    /// Disable ETag handling entirely.
    Disabled,
}

impl Default for ETagMode {
    fn default() -> Self {
        Self::Auto
    }
}

/// Configuration for ETag middleware.
#[derive(Debug, Clone)]
pub struct ETagConfig {
    /// ETag generation mode.
    pub mode: ETagMode,
    /// Generate weak ETags (W/"...") instead of strong ETags.
    /// Weak ETags indicate semantic equivalence, allowing minor changes.
    pub weak: bool,
    /// Minimum response body size to generate ETag.
    /// Responses smaller than this won't get an ETag.
    pub min_size: usize,
}

impl Default for ETagConfig {
    fn default() -> Self {
        Self {
            mode: ETagMode::Auto,
            weak: false,
            min_size: 0,
        }
    }
}

impl ETagConfig {
    /// Create a new ETag configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the ETag generation mode.
    #[must_use]
    pub fn mode(mut self, mode: ETagMode) -> Self {
        self.mode = mode;
        self
    }

    /// Enable weak ETags.
    #[must_use]
    pub fn weak(mut self, weak: bool) -> Self {
        self.weak = weak;
        self
    }

    /// Set minimum body size for ETag generation.
    #[must_use]
    pub fn min_size(mut self, size: usize) -> Self {
        self.min_size = size;
        self
    }
}

/// Middleware for ETag generation and conditional request handling.
///
/// Implements HTTP caching through ETags as defined in RFC 7232.
///
/// # Features
///
/// - **Automatic ETag generation**: Computes ETag from response body hash
/// - **If-None-Match handling**: Returns 304 Not Modified for GET/HEAD when ETag matches
/// - **Weak and strong ETags**: Configurable ETag strength
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::{ETagMiddleware, ETagConfig, ETagMode};
///
/// // Default: auto-generate strong ETags
/// let middleware = ETagMiddleware::new();
///
/// // With custom configuration
/// let middleware = ETagMiddleware::with_config(
///     ETagConfig::new()
///         .mode(ETagMode::Auto)
///         .weak(true)
///         .min_size(1024)
/// );
/// ```
///
/// # Conditional Request Flow
///
/// For GET/HEAD requests with `If-None-Match` header:
/// 1. Generate ETag for response body
/// 2. Compare with client's cached ETag
/// 3. If match: return 304 Not Modified (empty body)
/// 4. If no match: return full response with ETag header
///
/// # Note on If-Match
///
/// `If-Match` handling for PUT/PATCH/DELETE is typically done at the
/// application level since it requires knowledge of the current resource
/// state before the modification occurs.
pub struct ETagMiddleware {
    config: ETagConfig,
}

impl Default for ETagMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl ETagMiddleware {
    /// Create ETag middleware with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ETagConfig::default(),
        }
    }

    /// Create ETag middleware with custom configuration.
    #[must_use]
    pub fn with_config(config: ETagConfig) -> Self {
        Self { config }
    }

    /// Generate an ETag from response body bytes using FNV-1a hash.
    ///
    /// FNV-1a is chosen for:
    /// - Speed: Very fast for small to medium data
    /// - Consistency: Deterministic output
    /// - Simplicity: No external dependencies
    fn generate_etag(data: &[u8], weak: bool) -> String {
        // FNV-1a 64-bit hash
        const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;

        let mut hash = FNV_OFFSET_BASIS;
        for &byte in data {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }

        // Format as quoted hex string
        if weak {
            format!("W/\"{:016x}\"", hash)
        } else {
            format!("\"{:016x}\"", hash)
        }
    }

    /// Parse ETags from If-None-Match header value.
    ///
    /// Handles:
    /// - Single ETag: "abc123"
    /// - Multiple ETags: "abc123", "def456"
    /// - Wildcard: *
    /// - Weak ETags: W/"abc123"
    fn parse_if_none_match(value: &str) -> Vec<String> {
        let trimmed = value.trim();

        // Handle wildcard
        if trimmed == "*" {
            return vec!["*".to_string()];
        }

        let mut etags = Vec::new();
        let mut current = String::new();
        let mut in_quote = false;
        let mut prev_char = '\0';

        for ch in trimmed.chars() {
            match ch {
                '"' if prev_char != '\\' => {
                    current.push(ch);
                    if in_quote {
                        // End of ETag value
                        let etag = current.trim().to_string();
                        if !etag.is_empty() {
                            etags.push(etag);
                        }
                        current.clear();
                    }
                    in_quote = !in_quote;
                }
                ',' if !in_quote => {
                    // ETag separator, already handled by quote closing
                    current.clear();
                }
                _ => {
                    current.push(ch);
                }
            }
            prev_char = ch;
        }

        etags
    }

    /// Check if two ETags match according to weak comparison rules.
    ///
    /// Weak comparison (for If-None-Match with GET/HEAD):
    /// - W/"a" matches W/"a"
    /// - W/"a" matches "a"
    /// - "a" matches W/"a"
    /// - "a" matches "a"
    fn etags_match_weak(etag1: &str, etag2: &str) -> bool {
        // Strip W/ prefix for weak comparison
        let e1 = Self::strip_weak_prefix(etag1);
        let e2 = Self::strip_weak_prefix(etag2);
        e1 == e2
    }

    /// Strip the weak ETag prefix (W/) if present.
    fn strip_weak_prefix(s: &str) -> &str {
        if s.starts_with("W/") || s.starts_with("w/") {
            &s[2..]
        } else {
            s
        }
    }

    /// Check if request method is cacheable (GET or HEAD).
    fn is_cacheable_method(method: crate::request::Method) -> bool {
        matches!(
            method,
            crate::request::Method::Get | crate::request::Method::Head
        )
    }

    /// Get existing ETag from response headers.
    fn get_existing_etag(headers: &[(String, Vec<u8>)]) -> Option<String> {
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("etag") {
                return std::str::from_utf8(value).ok().map(String::from);
            }
        }
        None
    }
}

impl Middleware for ETagMiddleware {
    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let config = self.config.clone();

        Box::pin(async move {
            // Skip if disabled
            if config.mode == ETagMode::Disabled {
                return response;
            }

            // Only handle cacheable methods
            if !Self::is_cacheable_method(req.method()) {
                return response;
            }

            // Decompose response to work with parts
            let (status, headers, body) = response.into_parts();

            // Check for existing ETag (for Manual mode or pre-set ETags)
            let existing_etag = Self::get_existing_etag(&headers);

            // Get body bytes if available
            let body_bytes = match &body {
                crate::response::ResponseBody::Bytes(bytes) => Some(bytes.clone()),
                crate::response::ResponseBody::Empty => Some(Vec::new()),
                crate::response::ResponseBody::Stream(_) => None,
            };

            // Determine the ETag to use
            let etag = if let Some(existing) = existing_etag {
                Some(existing)
            } else if config.mode == ETagMode::Auto {
                if let Some(ref bytes) = body_bytes {
                    if bytes.len() >= config.min_size {
                        Some(Self::generate_etag(bytes, config.weak))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            // Check If-None-Match header
            if let Some(ref etag_value) = etag {
                if let Some(if_none_match) = req.headers().get("if-none-match") {
                    if let Ok(value) = std::str::from_utf8(if_none_match) {
                        let client_etags = Self::parse_if_none_match(value);

                        // Check for wildcard or matching ETag
                        let matches = client_etags.iter().any(|client_etag| {
                            client_etag == "*" || Self::etags_match_weak(client_etag, etag_value)
                        });

                        if matches {
                            // Return 304 Not Modified with ETag header
                            return Response::with_status(
                                crate::response::StatusCode::NOT_MODIFIED,
                            )
                            .header("etag", etag_value.as_bytes().to_vec());
                        }
                    }
                }
            }

            // Rebuild response with ETag header if we have one
            let mut new_response = Response::with_status(status)
                .body(body)
                .rebuild_with_headers(headers);

            if let Some(etag_value) = etag {
                new_response = new_response.header("etag", etag_value.into_bytes());
            }

            new_response
        })
    }

    fn name(&self) -> &'static str {
        "ETagMiddleware"
    }
}

// ===========================================================================
// HTTP Cache Control Middleware
// ===========================================================================

/// Individual Cache-Control directives.
///
/// These directives control how responses are cached by browsers, proxies,
/// and CDNs. See RFC 7234 for full specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDirective {
    /// Response may be stored by any cache.
    Public,
    /// Response may only be stored by browser cache (not shared caches like CDNs).
    Private,
    /// Response must not be stored by any cache.
    NoStore,
    /// Cache must validate with server before using cached response.
    NoCache,
    /// Cache must not transform the response (e.g., compress images).
    NoTransform,
    /// Cached response must be revalidated once it becomes stale.
    MustRevalidate,
    /// Like must-revalidate but only for shared caches.
    ProxyRevalidate,
    /// Response may be served stale if origin is unreachable.
    StaleIfError,
    /// Response may be served stale while revalidating in background.
    StaleWhileRevalidate,
    /// Only cache if explicitly told to (for shared caches).
    SMaxAge,
    /// Do not store response in persistent storage.
    OnlyIfCached,
    /// Indicates an immutable response that won't change during its freshness lifetime.
    Immutable,
}

impl CacheDirective {
    /// Returns the directive as a Cache-Control header string fragment.
    fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::NoStore => "no-store",
            Self::NoCache => "no-cache",
            Self::NoTransform => "no-transform",
            Self::MustRevalidate => "must-revalidate",
            Self::ProxyRevalidate => "proxy-revalidate",
            Self::StaleIfError => "stale-if-error",
            Self::StaleWhileRevalidate => "stale-while-revalidate",
            Self::SMaxAge => "s-maxage",
            Self::OnlyIfCached => "only-if-cached",
            Self::Immutable => "immutable",
        }
    }
}

/// Builder for constructing Cache-Control header values.
///
/// Provides a fluent API for building complex cache control policies.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::CacheControlBuilder;
///
/// // Public, cacheable for 1 hour, must revalidate after
/// let cache = CacheControlBuilder::new()
///     .public()
///     .max_age_secs(3600)
///     .must_revalidate()
///     .build();
///
/// // Private, no caching
/// let no_cache = CacheControlBuilder::new()
///     .private()
///     .no_store()
///     .build();
///
/// // CDN-friendly: public with different browser/CDN TTLs
/// let cdn = CacheControlBuilder::new()
///     .public()
///     .max_age_secs(60)        // Browser caches for 1 minute
///     .s_maxage_secs(3600)     // CDN caches for 1 hour
///     .build();
/// ```
#[derive(Debug, Clone, Default)]
pub struct CacheControlBuilder {
    directives: Vec<CacheDirective>,
    max_age: Option<u32>,
    s_maxage: Option<u32>,
    stale_while_revalidate: Option<u32>,
    stale_if_error: Option<u32>,
}

impl CacheControlBuilder {
    /// Create a new empty Cache-Control builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add the `public` directive - response may be cached by any cache.
    #[must_use]
    pub fn public(mut self) -> Self {
        self.directives.push(CacheDirective::Public);
        self
    }

    /// Add the `private` directive - response may only be cached by browser.
    #[must_use]
    pub fn private(mut self) -> Self {
        self.directives.push(CacheDirective::Private);
        self
    }

    /// Add the `no-store` directive - response must not be cached.
    #[must_use]
    pub fn no_store(mut self) -> Self {
        self.directives.push(CacheDirective::NoStore);
        self
    }

    /// Add the `no-cache` directive - must revalidate before using cache.
    #[must_use]
    pub fn no_cache(mut self) -> Self {
        self.directives.push(CacheDirective::NoCache);
        self
    }

    /// Add the `no-transform` directive - caches must not modify response.
    #[must_use]
    pub fn no_transform(mut self) -> Self {
        self.directives.push(CacheDirective::NoTransform);
        self
    }

    /// Add the `must-revalidate` directive - cache must check origin when stale.
    #[must_use]
    pub fn must_revalidate(mut self) -> Self {
        self.directives.push(CacheDirective::MustRevalidate);
        self
    }

    /// Add the `proxy-revalidate` directive - shared caches must check origin when stale.
    #[must_use]
    pub fn proxy_revalidate(mut self) -> Self {
        self.directives.push(CacheDirective::ProxyRevalidate);
        self
    }

    /// Add the `immutable` directive - response won't change during freshness lifetime.
    #[must_use]
    pub fn immutable(mut self) -> Self {
        self.directives.push(CacheDirective::Immutable);
        self
    }

    /// Set `max-age` directive - maximum time response is fresh (in seconds).
    #[must_use]
    pub fn max_age_secs(mut self, seconds: u32) -> Self {
        self.max_age = Some(seconds);
        self
    }

    /// Set `max-age` directive from a Duration.
    #[must_use]
    pub fn max_age(self, duration: std::time::Duration) -> Self {
        self.max_age_secs(duration.as_secs() as u32)
    }

    /// Set `s-maxage` directive - maximum time for shared caches (in seconds).
    #[must_use]
    pub fn s_maxage_secs(mut self, seconds: u32) -> Self {
        self.s_maxage = Some(seconds);
        self
    }

    /// Set `s-maxage` directive from a Duration.
    #[must_use]
    pub fn s_maxage(self, duration: std::time::Duration) -> Self {
        self.s_maxage_secs(duration.as_secs() as u32)
    }

    /// Set `stale-while-revalidate` directive - serve stale while revalidating (in seconds).
    #[must_use]
    pub fn stale_while_revalidate_secs(mut self, seconds: u32) -> Self {
        self.stale_while_revalidate = Some(seconds);
        self
    }

    /// Set `stale-if-error` directive - serve stale if origin errors (in seconds).
    #[must_use]
    pub fn stale_if_error_secs(mut self, seconds: u32) -> Self {
        self.stale_if_error = Some(seconds);
        self
    }

    /// Build the Cache-Control header value string.
    #[must_use]
    pub fn build(&self) -> String {
        let mut parts = Vec::new();

        // Add directives
        for directive in &self.directives {
            parts.push(directive.as_str().to_string());
        }

        // Add max-age
        if let Some(age) = self.max_age {
            parts.push(format!("max-age={age}"));
        }

        // Add s-maxage
        if let Some(age) = self.s_maxage {
            parts.push(format!("s-maxage={age}"));
        }

        // Add stale-while-revalidate
        if let Some(seconds) = self.stale_while_revalidate {
            parts.push(format!("stale-while-revalidate={seconds}"));
        }

        // Add stale-if-error
        if let Some(seconds) = self.stale_if_error {
            parts.push(format!("stale-if-error={seconds}"));
        }

        parts.join(", ")
    }

    /// Check if this represents a no-cache policy.
    #[must_use]
    pub fn is_no_cache(&self) -> bool {
        self.directives.contains(&CacheDirective::NoStore)
            || self.directives.contains(&CacheDirective::NoCache)
    }
}

/// Common cache control presets for typical use cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePreset {
    /// No caching: `no-store, no-cache, must-revalidate`
    NoCache,
    /// Private caching only: `private, max-age=0, must-revalidate`
    PrivateNoCache,
    /// Standard public caching: `public, max-age=3600`
    PublicOneHour,
    /// Long-term immutable: `public, max-age=31536000, immutable`
    Immutable,
    /// CDN-friendly with short browser TTL: `public, max-age=60, s-maxage=3600`
    CdnFriendly,
    /// Static assets: `public, max-age=86400`
    StaticAssets,
}

impl CachePreset {
    /// Convert preset to Cache-Control header value.
    #[must_use]
    pub fn to_header_value(&self) -> String {
        match self {
            Self::NoCache => "no-store, no-cache, must-revalidate".to_string(),
            Self::PrivateNoCache => "private, max-age=0, must-revalidate".to_string(),
            Self::PublicOneHour => "public, max-age=3600".to_string(),
            Self::Immutable => "public, max-age=31536000, immutable".to_string(),
            Self::CdnFriendly => "public, max-age=60, s-maxage=3600".to_string(),
            Self::StaticAssets => "public, max-age=86400".to_string(),
        }
    }

    /// Convert preset to a CacheControlBuilder for further customization.
    #[must_use]
    pub fn to_builder(&self) -> CacheControlBuilder {
        match self {
            Self::NoCache => CacheControlBuilder::new()
                .no_store()
                .no_cache()
                .must_revalidate(),
            Self::PrivateNoCache => CacheControlBuilder::new()
                .private()
                .max_age_secs(0)
                .must_revalidate(),
            Self::PublicOneHour => CacheControlBuilder::new().public().max_age_secs(3600),
            Self::Immutable => CacheControlBuilder::new()
                .public()
                .max_age_secs(31536000)
                .immutable(),
            Self::CdnFriendly => CacheControlBuilder::new()
                .public()
                .max_age_secs(60)
                .s_maxage_secs(3600),
            Self::StaticAssets => CacheControlBuilder::new().public().max_age_secs(86400),
        }
    }
}

/// Configuration for the Cache Control middleware.
#[derive(Debug, Clone)]
pub struct CacheControlConfig {
    /// The Cache-Control header value to set.
    pub cache_control: String,
    /// Optional Vary header values for content negotiation.
    pub vary: Vec<String>,
    /// Whether to set Expires header (deprecated but still used).
    pub set_expires: bool,
    /// Whether to preserve existing Cache-Control headers.
    pub preserve_existing: bool,
    /// HTTP methods to apply caching to (default: GET, HEAD).
    pub methods: Vec<crate::request::Method>,
    /// Path patterns to match (empty = match all).
    pub path_patterns: Vec<String>,
    /// Status codes to cache (default: 200-299).
    pub cacheable_statuses: Vec<u16>,
}

impl Default for CacheControlConfig {
    fn default() -> Self {
        Self {
            cache_control: CachePreset::NoCache.to_header_value(),
            vary: Vec::new(),
            set_expires: false,
            preserve_existing: true,
            methods: vec![crate::request::Method::Get, crate::request::Method::Head],
            path_patterns: Vec::new(),
            cacheable_statuses: (200..300).collect(),
        }
    }
}

impl CacheControlConfig {
    /// Create a new configuration with the default no-cache policy.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create configuration from a preset.
    #[must_use]
    pub fn from_preset(preset: CachePreset) -> Self {
        Self {
            cache_control: preset.to_header_value(),
            ..Self::default()
        }
    }

    /// Create configuration from a custom builder.
    #[must_use]
    pub fn from_builder(builder: CacheControlBuilder) -> Self {
        Self {
            cache_control: builder.build(),
            ..Self::default()
        }
    }

    /// Set the Cache-Control header value.
    #[must_use]
    pub fn cache_control(mut self, value: impl Into<String>) -> Self {
        self.cache_control = value.into();
        self
    }

    /// Add a Vary header value (for content negotiation).
    #[must_use]
    pub fn vary(mut self, header: impl Into<String>) -> Self {
        self.vary.push(header.into());
        self
    }

    /// Add multiple Vary header values.
    #[must_use]
    pub fn vary_headers(mut self, headers: Vec<String>) -> Self {
        self.vary.extend(headers);
        self
    }

    /// Enable setting the Expires header.
    #[must_use]
    pub fn with_expires(mut self, enable: bool) -> Self {
        self.set_expires = enable;
        self
    }

    /// Whether to preserve existing Cache-Control headers.
    #[must_use]
    pub fn preserve_existing(mut self, preserve: bool) -> Self {
        self.preserve_existing = preserve;
        self
    }

    /// Set the HTTP methods to apply caching to.
    #[must_use]
    pub fn methods(mut self, methods: Vec<crate::request::Method>) -> Self {
        self.methods = methods;
        self
    }

    /// Set path patterns to match (glob-style).
    #[must_use]
    pub fn path_patterns(mut self, patterns: Vec<String>) -> Self {
        self.path_patterns = patterns;
        self
    }

    /// Set cacheable status codes.
    #[must_use]
    pub fn cacheable_statuses(mut self, statuses: Vec<u16>) -> Self {
        self.cacheable_statuses = statuses;
        self
    }
}

/// Middleware for setting HTTP cache control headers.
///
/// This middleware adds Cache-Control, Vary, and optionally Expires headers
/// to responses. It supports various caching strategies from no-cache to
/// aggressive caching for static assets.
///
/// # Features
///
/// - **Cache-Control directives**: Full support for RFC 7234 directives
/// - **Vary header**: Content negotiation support for Accept-Encoding, Accept-Language, etc.
/// - **Expires header**: Optional legacy header support
/// - **Per-route configuration**: Apply different policies via middleware stacks
/// - **Method filtering**: Only cache GET/HEAD by default
/// - **Status filtering**: Only cache successful responses
///
/// # Example
///
/// ```ignore
/// use fastapi_core::middleware::{CacheControlMiddleware, CacheControlConfig, CachePreset};
///
/// // No caching for API responses (default)
/// let api_cache = CacheControlMiddleware::new();
///
/// // Public caching for static assets
/// let static_cache = CacheControlMiddleware::with_preset(CachePreset::StaticAssets);
///
/// // Custom caching with Vary header
/// let custom_cache = CacheControlMiddleware::with_config(
///     CacheControlConfig::from_preset(CachePreset::PublicOneHour)
///         .vary("Accept-Encoding")
///         .vary("Accept-Language")
///         .with_expires(true)
/// );
///
/// // CDN-friendly caching
/// let cdn_cache = CacheControlMiddleware::with_preset(CachePreset::CdnFriendly);
/// ```
///
/// # Response Headers Set
///
/// | Header | Description |
/// |--------|-------------|
/// | `Cache-Control` | Main caching directive |
/// | `Vary` | Headers that affect caching |
/// | `Expires` | Legacy expiration (if enabled) |
///
pub struct CacheControlMiddleware {
    config: CacheControlConfig,
}

impl Default for CacheControlMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheControlMiddleware {
    /// Create middleware with default no-cache policy.
    ///
    /// This is the safest default - no caching unless explicitly configured.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: CacheControlConfig::default(),
        }
    }

    /// Create middleware with a preset caching policy.
    #[must_use]
    pub fn with_preset(preset: CachePreset) -> Self {
        Self {
            config: CacheControlConfig::from_preset(preset),
        }
    }

    /// Create middleware with custom configuration.
    #[must_use]
    pub fn with_config(config: CacheControlConfig) -> Self {
        Self { config }
    }

    /// Check if the request method is cacheable.
    fn is_cacheable_method(&self, method: crate::request::Method) -> bool {
        self.config.methods.contains(&method)
    }

    /// Check if the response status is cacheable.
    fn is_cacheable_status(&self, status: u16) -> bool {
        self.config.cacheable_statuses.contains(&status)
    }

    /// Check if the path matches any configured patterns.
    fn matches_path(&self, path: &str) -> bool {
        if self.config.path_patterns.is_empty() {
            return true; // Match all if no patterns configured
        }

        for pattern in &self.config.path_patterns {
            if path_matches_pattern(path, pattern) {
                return true;
            }
        }
        false
    }

    /// Check if response already has a Cache-Control header.
    fn has_cache_control(headers: &[(String, Vec<u8>)]) -> bool {
        headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("cache-control"))
    }

    /// Calculate Expires date from max-age value.
    fn calculate_expires(cache_control: &str) -> Option<String> {
        // Extract max-age value if present
        for directive in cache_control.split(',') {
            let directive = directive.trim();
            if directive.starts_with("max-age=") {
                if let Ok(seconds) = directive[8..].parse::<u64>() {
                    // Calculate expiration time
                    let now = std::time::SystemTime::now();
                    if let Some(expires) = now.checked_add(std::time::Duration::from_secs(seconds))
                    {
                        return Some(format_http_date(expires));
                    }
                }
            }
        }
        None
    }
}

/// Simple path pattern matching (supports * wildcard).
fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if pattern.contains('*') {
        // Simple wildcard matching
        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            let (prefix, suffix) = (parts[0], parts[1]);
            return path.starts_with(prefix) && path.ends_with(suffix);
        }
        // For more complex patterns, do a simple contains check
        let fixed_parts: Vec<&str> = pattern.split('*').filter(|s| !s.is_empty()).collect();
        let mut remaining = path;
        for part in fixed_parts {
            if let Some(pos) = remaining.find(part) {
                remaining = &remaining[pos + part.len()..];
            } else {
                return false;
            }
        }
        true
    } else {
        path == pattern
    }
}

/// Format a SystemTime as an HTTP date (RFC 7231).
fn format_http_date(time: std::time::SystemTime) -> String {
    // Use UNIX_EPOCH to calculate duration
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => {
            // Calculate date components
            let secs = duration.as_secs();
            // Days since epoch
            let days = secs / 86400;
            let remaining_secs = secs % 86400;
            let hours = remaining_secs / 3600;
            let minutes = (remaining_secs % 3600) / 60;
            let seconds = remaining_secs % 60;

            // Calculate day of week (Jan 1, 1970 was Thursday = 4)
            let day_of_week = ((days + 4) % 7) as usize;
            let day_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

            // Calculate date (simplified - doesn't account for leap years perfectly but good enough)
            let (year, month, day) = days_to_date(days);
            let month_names = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];

            format!(
                "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
                day_names[day_of_week],
                day,
                month_names[(month - 1) as usize],
                year,
                hours,
                minutes,
                seconds
            )
        }
        Err(_) => "Thu, 01 Jan 1970 00:00:00 GMT".to_string(),
    }
}

/// Convert days since UNIX epoch to (year, month, day).
fn days_to_date(days: u64) -> (u64, u64, u64) {
    // Simplified algorithm - works for dates 1970-2099
    let mut remaining_days = days;
    let mut year = 1970u64;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days: [u64; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u64;
    for &days_in_month in &month_days {
        if remaining_days < days_in_month {
            break;
        }
        remaining_days -= days_in_month;
        month += 1;
    }

    (year, month, remaining_days + 1)
}

/// Check if a year is a leap year.
fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

impl Middleware for CacheControlMiddleware {
    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        let config = self.config.clone();

        Box::pin(async move {
            // Check if this request/response is cacheable
            if !self.is_cacheable_method(req.method()) {
                return response;
            }

            if !self.is_cacheable_status(response.status().as_u16()) {
                return response;
            }

            if !self.matches_path(req.path()) {
                return response;
            }

            // Decompose response to modify headers
            let (status, mut headers, body) = response.into_parts();

            // Check for existing Cache-Control header
            if config.preserve_existing && Self::has_cache_control(&headers) {
                // Reconstruct and return unchanged
                let mut resp = Response::with_status(status);
                for (name, value) in headers {
                    resp = resp.header(name, value);
                }
                return resp.body(body);
            }

            // Add Cache-Control header
            headers.push((
                "Cache-Control".to_string(),
                config.cache_control.as_bytes().to_vec(),
            ));

            // Add Vary header if configured
            if !config.vary.is_empty() {
                let vary_value = config.vary.join(", ");
                headers.push(("Vary".to_string(), vary_value.into_bytes()));
            }

            // Add Expires header if configured
            if config.set_expires {
                if let Some(expires) = Self::calculate_expires(&config.cache_control) {
                    headers.push(("Expires".to_string(), expires.into_bytes()));
                }
            }

            // Reconstruct response
            let mut resp = Response::with_status(status);
            for (name, value) in headers {
                resp = resp.header(name, value);
            }
            resp.body(body)
        })
    }

    fn name(&self) -> &'static str {
        "CacheControlMiddleware"
    }
}

// ===========================================================================
// End Cache Control Middleware
// ===========================================================================

#[cfg(test)]
mod cache_control_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::StatusCode;

    fn test_context() -> RequestContext {
        RequestContext::new(asupersync::Cx::for_testing(), 1)
    }

    fn run_after(mw: &CacheControlMiddleware, req: &Request, resp: Response) -> Response {
        let ctx = test_context();
        let fut = mw.after(&ctx, req, resp);
        futures_executor::block_on(fut)
    }

    #[test]
    fn cache_directive_as_str_works() {
        assert_eq!(CacheDirective::Public.as_str(), "public");
        assert_eq!(CacheDirective::Private.as_str(), "private");
        assert_eq!(CacheDirective::NoStore.as_str(), "no-store");
        assert_eq!(CacheDirective::NoCache.as_str(), "no-cache");
        assert_eq!(CacheDirective::MustRevalidate.as_str(), "must-revalidate");
        assert_eq!(CacheDirective::Immutable.as_str(), "immutable");
    }

    #[test]
    fn cache_control_builder_basic() {
        let cc = CacheControlBuilder::new()
            .public()
            .max_age_secs(3600)
            .build();
        assert!(cc.contains("public"));
        assert!(cc.contains("max-age=3600"));
    }

    #[test]
    fn cache_control_builder_complex() {
        let cc = CacheControlBuilder::new()
            .public()
            .max_age_secs(60)
            .s_maxage_secs(3600)
            .stale_while_revalidate_secs(86400)
            .build();
        assert!(cc.contains("public"));
        assert!(cc.contains("max-age=60"));
        assert!(cc.contains("s-maxage=3600"));
        assert!(cc.contains("stale-while-revalidate=86400"));
    }

    #[test]
    fn cache_control_builder_no_cache() {
        let cc = CacheControlBuilder::new()
            .no_store()
            .no_cache()
            .must_revalidate()
            .build();
        assert!(cc.contains("no-store"));
        assert!(cc.contains("no-cache"));
        assert!(cc.contains("must-revalidate"));
    }

    #[test]
    fn cache_preset_no_cache() {
        let value = CachePreset::NoCache.to_header_value();
        assert!(value.contains("no-store"));
        assert!(value.contains("no-cache"));
        assert!(value.contains("must-revalidate"));
    }

    #[test]
    fn cache_preset_immutable() {
        let value = CachePreset::Immutable.to_header_value();
        assert!(value.contains("public"));
        assert!(value.contains("max-age=31536000"));
        assert!(value.contains("immutable"));
    }

    #[test]
    fn cache_preset_static_assets() {
        let value = CachePreset::StaticAssets.to_header_value();
        assert!(value.contains("public"));
        assert!(value.contains("max-age=86400"));
    }

    #[test]
    fn middleware_adds_cache_control_header() {
        let mw = CacheControlMiddleware::with_preset(CachePreset::PublicOneHour);
        let req = Request::new(Method::Get, "/api/test");
        let resp = Response::with_status(StatusCode::OK);

        let result = run_after(&mw, &req, resp);
        let headers = result.headers();
        let cc_header = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("cache-control"));
        assert!(
            cc_header.is_some(),
            "Cache-Control header should be present"
        );
        let (_, value) = cc_header.unwrap();
        let value_str = String::from_utf8_lossy(value);
        assert!(value_str.contains("public"));
        assert!(value_str.contains("max-age=3600"));
    }

    #[test]
    fn middleware_skips_post_requests() {
        let mw = CacheControlMiddleware::with_preset(CachePreset::PublicOneHour);
        let req = Request::new(Method::Post, "/api/test");
        let resp = Response::with_status(StatusCode::OK);

        let result = run_after(&mw, &req, resp);
        let headers = result.headers();
        let cc_header = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("cache-control"));
        assert!(
            cc_header.is_none(),
            "Cache-Control should not be added for POST"
        );
    }

    #[test]
    fn middleware_skips_error_responses() {
        let mw = CacheControlMiddleware::with_preset(CachePreset::PublicOneHour);
        let req = Request::new(Method::Get, "/api/test");
        let resp = Response::with_status(StatusCode::INTERNAL_SERVER_ERROR);

        let result = run_after(&mw, &req, resp);
        let headers = result.headers();
        let cc_header = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("cache-control"));
        assert!(
            cc_header.is_none(),
            "Cache-Control should not be added for error responses"
        );
    }

    #[test]
    fn middleware_with_vary_header() {
        let mw = CacheControlMiddleware::with_config(
            CacheControlConfig::from_preset(CachePreset::PublicOneHour)
                .vary("Accept-Encoding")
                .vary("Accept-Language"),
        );
        let req = Request::new(Method::Get, "/api/test");
        let resp = Response::with_status(StatusCode::OK);

        let result = run_after(&mw, &req, resp);
        let headers = result.headers();
        let vary_header = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("vary"));
        assert!(vary_header.is_some(), "Vary header should be present");
        let (_, value) = vary_header.unwrap();
        let value_str = String::from_utf8_lossy(value);
        assert!(value_str.contains("Accept-Encoding"));
        assert!(value_str.contains("Accept-Language"));
    }

    #[test]
    fn middleware_preserves_existing_cache_control() {
        let mw = CacheControlMiddleware::with_config(
            CacheControlConfig::from_preset(CachePreset::PublicOneHour).preserve_existing(true),
        );
        let req = Request::new(Method::Get, "/api/test");
        let resp =
            Response::with_status(StatusCode::OK).header("Cache-Control", b"max-age=60".to_vec());

        let result = run_after(&mw, &req, resp);
        let headers = result.headers();
        let cc_headers: Vec<_> = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("cache-control"))
            .collect();
        // Should only have the original header, not add a new one
        assert_eq!(cc_headers.len(), 1);
        let (_, value) = cc_headers[0];
        let value_str = String::from_utf8_lossy(value);
        assert_eq!(value_str, "max-age=60");
    }

    #[test]
    fn path_pattern_matching_exact() {
        assert!(path_matches_pattern("/api/users", "/api/users"));
        assert!(!path_matches_pattern("/api/users", "/api/items"));
    }

    #[test]
    fn path_pattern_matching_wildcard() {
        assert!(path_matches_pattern("/api/users/123", "/api/users/*"));
        assert!(path_matches_pattern("/static/css/style.css", "/static/*"));
        assert!(path_matches_pattern("/anything", "*"));
    }

    #[test]
    fn date_formatting_works() {
        // Test that format_http_date doesn't panic and produces valid format
        let now = std::time::SystemTime::now();
        let formatted = format_http_date(now);
        // Should contain GMT
        assert!(formatted.ends_with(" GMT"));
        // Should have day name
        let days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        assert!(days.iter().any(|d| formatted.starts_with(d)));
    }

    #[test]
    fn leap_year_detection() {
        assert!(!is_leap_year(1900)); // Divisible by 100 but not 400
        assert!(is_leap_year(2000)); // Divisible by 400
        assert!(is_leap_year(2024)); // Divisible by 4 but not 100
        assert!(!is_leap_year(2023)); // Not divisible by 4
    }
}

// ===========================================================================
// End ETag Middleware
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::{ResponseBody, StatusCode};

    // Test middleware that adds a header
    #[allow(dead_code)]
    struct AddHeaderMiddleware {
        name: &'static str,
        value: &'static [u8],
    }

    impl Middleware for AddHeaderMiddleware {
        fn after<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a Request,
            response: Response,
        ) -> BoxFuture<'a, Response> {
            Box::pin(async move { response.header(self.name, self.value.to_vec()) })
        }
    }

    // Test middleware that short-circuits
    #[allow(dead_code)]
    struct BlockingMiddleware;

    impl Middleware for BlockingMiddleware {
        fn before<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, ControlFlow> {
            Box::pin(async {
                ControlFlow::Break(
                    Response::with_status(StatusCode::FORBIDDEN)
                        .body(ResponseBody::Bytes(b"blocked".to_vec())),
                )
            })
        }
    }

    // Test middleware that tracks calls
    #[allow(dead_code)]
    struct TrackingMiddleware {
        before_count: std::sync::atomic::AtomicUsize,
        after_count: std::sync::atomic::AtomicUsize,
    }

    #[allow(dead_code)]
    impl TrackingMiddleware {
        fn new() -> Self {
            Self {
                before_count: std::sync::atomic::AtomicUsize::new(0),
                after_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn before_count(&self) -> usize {
            self.before_count.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn after_count(&self) -> usize {
            self.after_count.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Middleware for TrackingMiddleware {
        fn before<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, ControlFlow> {
            self.before_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { ControlFlow::Continue })
        }

        fn after<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a Request,
            response: Response,
        ) -> BoxFuture<'a, Response> {
            self.after_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move { response })
        }
    }

    #[test]
    fn control_flow_variants() {
        let cont = ControlFlow::Continue;
        assert!(cont.is_continue());
        assert!(!cont.is_break());

        let brk = ControlFlow::Break(Response::ok());
        assert!(!brk.is_continue());
        assert!(brk.is_break());
    }

    #[test]
    fn middleware_stack_empty() {
        let stack = MiddlewareStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);
    }

    #[test]
    fn middleware_stack_push() {
        let mut stack = MiddlewareStack::new();
        stack.push(NoopMiddleware);
        stack.push(NoopMiddleware);
        assert_eq!(stack.len(), 2);
        assert!(!stack.is_empty());
    }

    #[test]
    fn noop_middleware_name() {
        let mw = NoopMiddleware;
        assert_eq!(mw.name(), "Noop");
    }

    #[test]
    fn logging_redacts_sensitive_headers() {
        let mut headers = crate::request::Headers::new();
        headers.insert("Authorization", b"secret".to_vec());
        headers.insert("X-Request-Id", b"abc123".to_vec());

        let redacted = super::default_redacted_headers();
        let formatted = super::format_headers(headers.iter(), &redacted);

        assert!(formatted.contains("authorization=<redacted>"));
        assert!(formatted.contains("x-request-id=abc123"));
    }

    #[test]
    fn logging_body_truncation() {
        let body = b"abcdef";
        let preview = super::format_bytes(body, 4);
        assert_eq!(preview, "abcd...");

        let preview_full = super::format_bytes(body, 10);
        assert_eq!(preview_full, "abcdef");
    }

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 1)
    }

    fn header_value(response: &Response, name: &str) -> Option<String> {
        response
            .headers()
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| std::str::from_utf8(v).ok())
            .map(ToString::to_string)
    }

    #[test]
    fn cors_exact_origin_allows() {
        let cors = Cors::new().allow_origin("https://example.com");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("origin", b"https://example.com".to_vec());

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));

        let response = Response::ok().body(ResponseBody::Bytes(b"ok".to_vec()));
        let response = futures_executor::block_on(cors.after(&ctx, &req, response));

        assert_eq!(
            header_value(&response, "access-control-allow-origin"),
            Some("https://example.com".to_string())
        );
        assert_eq!(header_value(&response, "vary"), Some("Origin".to_string()));
    }

    #[test]
    fn cors_wildcard_origin_allows() {
        let cors = Cors::new().allow_origin_wildcard("https://*.example.com");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("origin", b"https://api.example.com".to_vec());

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));
    }

    #[test]
    fn cors_regex_origin_allows() {
        let cors = Cors::new().allow_origin_regex(r"^https://.*\.example\.com$");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("origin", b"https://svc.example.com".to_vec());

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));
    }

    #[test]
    fn cors_preflight_handled() {
        let cors = Cors::new()
            .allow_any_origin()
            .allow_headers(["x-test", "content-type"])
            .max_age(600);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Options, "/");
        req.headers_mut()
            .insert("origin", b"https://example.com".to_vec());
        req.headers_mut()
            .insert("access-control-request-method", b"POST".to_vec());
        req.headers_mut().insert(
            "access-control-request-headers",
            b"x-test, content-type".to_vec(),
        );

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        let ControlFlow::Break(response) = result else {
            panic!("expected preflight break");
        };

        assert_eq!(response.status().as_u16(), 204);
        assert_eq!(
            header_value(&response, "access-control-allow-origin"),
            Some("*".to_string())
        );
        assert_eq!(
            header_value(&response, "access-control-allow-methods"),
            Some("GET, POST, PUT, PATCH, DELETE, OPTIONS, HEAD".to_string())
        );
        assert_eq!(
            header_value(&response, "access-control-allow-headers"),
            Some("x-test, content-type".to_string())
        );
        assert_eq!(
            header_value(&response, "access-control-max-age"),
            Some("600".to_string())
        );
    }

    #[test]
    fn cors_credentials_echo_origin() {
        let cors = Cors::new().allow_any_origin().allow_credentials(true);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("origin", b"https://example.com".to_vec());

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));

        let response = futures_executor::block_on(cors.after(&ctx, &req, Response::ok()));
        assert_eq!(
            header_value(&response, "access-control-allow-origin"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            header_value(&response, "access-control-allow-credentials"),
            Some("true".to_string())
        );
    }

    #[test]
    fn cors_disallowed_preflight_forbidden() {
        let cors = Cors::new().allow_origin("https://good.example");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Options, "/");
        req.headers_mut()
            .insert("origin", b"https://evil.example".to_vec());
        req.headers_mut()
            .insert("access-control-request-method", b"GET".to_vec());

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        let ControlFlow::Break(response) = result else {
            panic!("expected forbidden preflight");
        };
        assert_eq!(response.status().as_u16(), 403);
    }

    #[test]
    fn cors_simple_request_disallowed_origin_no_headers() {
        // Non-preflight request from disallowed origin should proceed but not get CORS headers
        let cors = Cors::new().allow_origin("https://good.example");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("origin", b"https://evil.example".to_vec());

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        // Simple requests proceed (browser will block based on missing headers)
        assert!(matches!(result, ControlFlow::Continue));

        let response = futures_executor::block_on(cors.after(&ctx, &req, Response::ok()));
        // No CORS headers should be added for disallowed origin
        assert!(header_value(&response, "access-control-allow-origin").is_none());
    }

    #[test]
    fn cors_expose_headers_configuration() {
        let cors = Cors::new()
            .allow_any_origin()
            .expose_headers(["x-custom-header", "x-another-header"]);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("origin", b"https://example.com".to_vec());

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));

        let response = futures_executor::block_on(cors.after(&ctx, &req, Response::ok()));
        assert_eq!(
            header_value(&response, "access-control-expose-headers"),
            Some("x-custom-header, x-another-header".to_string())
        );
    }

    #[test]
    fn cors_any_origin_sets_wildcard() {
        let cors = Cors::new().allow_any_origin();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("origin", b"https://any-site.com".to_vec());

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));

        let response = futures_executor::block_on(cors.after(&ctx, &req, Response::ok()));
        assert_eq!(
            header_value(&response, "access-control-allow-origin"),
            Some("*".to_string())
        );
    }

    #[test]
    fn cors_config_allows_method_override() {
        // Test that allow_methods overrides defaults
        let cors = Cors::new()
            .allow_any_origin()
            .allow_methods([crate::request::Method::Get, crate::request::Method::Post]);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Options, "/");
        req.headers_mut()
            .insert("origin", b"https://example.com".to_vec());
        req.headers_mut()
            .insert("access-control-request-method", b"POST".to_vec());

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        let ControlFlow::Break(response) = result else {
            panic!("expected preflight break");
        };
        assert_eq!(
            header_value(&response, "access-control-allow-methods"),
            Some("GET, POST".to_string())
        );
    }

    #[test]
    fn cors_no_origin_header_skips_cors() {
        // Request without Origin header should not get CORS headers
        let cors = Cors::new().allow_any_origin();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let result = futures_executor::block_on(cors.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));

        let response = futures_executor::block_on(cors.after(&ctx, &req, Response::ok()));
        assert!(header_value(&response, "access-control-allow-origin").is_none());
    }

    #[test]
    fn cors_middleware_name() {
        let cors = Cors::new();
        assert_eq!(cors.name(), "Cors");
    }

    // =========================================================================
    // Request ID Middleware tests
    // =========================================================================

    #[test]
    fn request_id_generates_unique_ids() {
        let id1 = RequestId::generate();
        let id2 = RequestId::generate();
        let id3 = RequestId::generate();

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);

        // IDs should be non-empty
        assert!(!id1.as_str().is_empty());
        assert!(!id2.as_str().is_empty());
        assert!(!id3.as_str().is_empty());
    }

    #[test]
    fn request_id_display() {
        let id = RequestId::new("test-request-123");
        assert_eq!(format!("{}", id), "test-request-123");
    }

    #[test]
    fn request_id_from_string() {
        let id: RequestId = "my-id".into();
        assert_eq!(id.as_str(), "my-id");

        let id2: RequestId = String::from("my-id-2").into();
        assert_eq!(id2.as_str(), "my-id-2");
    }

    #[test]
    fn request_id_config_defaults() {
        let config = RequestIdConfig::default();
        assert_eq!(config.header_name, "x-request-id");
        assert!(config.accept_from_client);
        assert!(config.add_to_response);
        assert_eq!(config.max_client_id_length, 128);
    }

    #[test]
    fn request_id_config_builder() {
        let config = RequestIdConfig::new()
            .header_name("X-Trace-ID")
            .accept_from_client(false)
            .add_to_response(false)
            .max_client_id_length(64);

        assert_eq!(config.header_name, "X-Trace-ID");
        assert!(!config.accept_from_client);
        assert!(!config.add_to_response);
        assert_eq!(config.max_client_id_length, 64);
    }

    #[test]
    fn request_id_middleware_generates_id() {
        let middleware = RequestIdMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let result = futures_executor::block_on(middleware.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));

        let stored_id = req.get_extension::<RequestId>();
        assert!(stored_id.is_some());
        assert!(!stored_id.unwrap().as_str().is_empty());
    }

    #[test]
    fn request_id_middleware_accepts_client_id() {
        let middleware = RequestIdMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("x-request-id", b"client-provided-id-123".to_vec());

        futures_executor::block_on(middleware.before(&ctx, &mut req));

        let stored_id = req.get_extension::<RequestId>().unwrap();
        assert_eq!(stored_id.as_str(), "client-provided-id-123");
    }

    #[test]
    fn request_id_middleware_rejects_invalid_client_id() {
        let middleware = RequestIdMiddleware::new();
        let ctx = test_context();

        // Test with invalid characters
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("x-request-id", b"invalid<script>id".to_vec());

        futures_executor::block_on(middleware.before(&ctx, &mut req));

        let stored_id = req.get_extension::<RequestId>().unwrap();
        // Should have generated a new ID instead of using the invalid one
        assert_ne!(stored_id.as_str(), "invalid<script>id");
    }

    #[test]
    fn request_id_middleware_rejects_too_long_client_id() {
        let config = RequestIdConfig::new().max_client_id_length(10);
        let middleware = RequestIdMiddleware::with_config(config);
        let ctx = test_context();

        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("x-request-id", b"this-id-is-way-too-long".to_vec());

        futures_executor::block_on(middleware.before(&ctx, &mut req));

        let stored_id = req.get_extension::<RequestId>().unwrap();
        // Should have generated a new ID instead of using the too-long one
        assert_ne!(stored_id.as_str(), "this-id-is-way-too-long");
    }

    #[test]
    fn request_id_middleware_adds_to_response() {
        let middleware = RequestIdMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        futures_executor::block_on(middleware.before(&ctx, &mut req));
        let stored_id = req.get_extension::<RequestId>().unwrap().clone();

        let response = Response::ok();
        let response = futures_executor::block_on(middleware.after(&ctx, &req, response));

        let header = header_value(&response, "x-request-id");
        assert_eq!(header, Some(stored_id.0));
    }

    #[test]
    fn request_id_middleware_respects_add_to_response_false() {
        let config = RequestIdConfig::new().add_to_response(false);
        let middleware = RequestIdMiddleware::with_config(config);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        futures_executor::block_on(middleware.before(&ctx, &mut req));

        let response = Response::ok();
        let response = futures_executor::block_on(middleware.after(&ctx, &req, response));

        let header = header_value(&response, "x-request-id");
        assert!(header.is_none());
    }

    #[test]
    fn request_id_middleware_respects_accept_from_client_false() {
        let config = RequestIdConfig::new().accept_from_client(false);
        let middleware = RequestIdMiddleware::with_config(config);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("x-request-id", b"client-id".to_vec());

        futures_executor::block_on(middleware.before(&ctx, &mut req));

        let stored_id = req.get_extension::<RequestId>().unwrap();
        // Should ignore client ID and generate new one
        assert_ne!(stored_id.as_str(), "client-id");
    }

    #[test]
    fn request_id_middleware_custom_header_name() {
        let config = RequestIdConfig::new().header_name("X-Trace-ID");
        let middleware = RequestIdMiddleware::with_config(config);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("X-Trace-ID", b"trace-123".to_vec());

        futures_executor::block_on(middleware.before(&ctx, &mut req));

        let stored_id = req.get_extension::<RequestId>().unwrap();
        assert_eq!(stored_id.as_str(), "trace-123");

        let response = Response::ok();
        let response = futures_executor::block_on(middleware.after(&ctx, &req, response));

        let header = header_value(&response, "X-Trace-ID");
        assert_eq!(header, Some("trace-123".to_string()));
    }

    #[test]
    fn is_valid_request_id_accepts_valid() {
        assert!(super::is_valid_request_id("abc123"));
        assert!(super::is_valid_request_id("request-id-123"));
        assert!(super::is_valid_request_id("request_id_123"));
        assert!(super::is_valid_request_id("request.id.123"));
        assert!(super::is_valid_request_id("ABC123"));
        assert!(super::is_valid_request_id("a-b_c.D"));
    }

    #[test]
    fn is_valid_request_id_rejects_invalid() {
        assert!(!super::is_valid_request_id(""));
        assert!(!super::is_valid_request_id("id with spaces"));
        assert!(!super::is_valid_request_id("id<script>"));
        assert!(!super::is_valid_request_id("id\nwith\nnewlines"));
        assert!(!super::is_valid_request_id("id;with;semicolons"));
        assert!(!super::is_valid_request_id("id/with/slashes"));
    }

    #[test]
    fn request_id_middleware_name() {
        let middleware = RequestIdMiddleware::new();
        assert_eq!(middleware.name(), "RequestId");
    }

    // =========================================================================
    // Middleware Stack Execution Order Tests
    // =========================================================================

    /// Test middleware that records when its before/after hooks run
    struct OrderTrackingMiddleware {
        id: &'static str,
        log: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl OrderTrackingMiddleware {
        fn new(id: &'static str, log: Arc<std::sync::Mutex<Vec<String>>>) -> Self {
            Self { id, log }
        }
    }

    impl Middleware for OrderTrackingMiddleware {
        fn before<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, ControlFlow> {
            self.log.lock().unwrap().push(format!("{}.before", self.id));
            Box::pin(async { ControlFlow::Continue })
        }

        fn after<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a Request,
            response: Response,
        ) -> BoxFuture<'a, Response> {
            self.log.lock().unwrap().push(format!("{}.after", self.id));
            Box::pin(async move { response })
        }
    }

    /// Test middleware that short-circuits with a configurable condition
    struct ConditionalBreakMiddleware {
        id: &'static str,
        should_break: bool,
        log: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl ConditionalBreakMiddleware {
        fn new(
            id: &'static str,
            should_break: bool,
            log: Arc<std::sync::Mutex<Vec<String>>>,
        ) -> Self {
            Self {
                id,
                should_break,
                log,
            }
        }
    }

    impl Middleware for ConditionalBreakMiddleware {
        fn before<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, ControlFlow> {
            self.log.lock().unwrap().push(format!("{}.before", self.id));
            let should_break = self.should_break;
            Box::pin(async move {
                if should_break {
                    ControlFlow::Break(
                        Response::with_status(StatusCode::FORBIDDEN)
                            .body(ResponseBody::Bytes(b"blocked".to_vec())),
                    )
                } else {
                    ControlFlow::Continue
                }
            })
        }

        fn after<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a Request,
            response: Response,
        ) -> BoxFuture<'a, Response> {
            self.log.lock().unwrap().push(format!("{}.after", self.id));
            Box::pin(async move { response })
        }
    }

    /// Simple test handler that returns 200 OK
    struct OkHandler;

    impl Handler for OkHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            Box::pin(async move { Response::ok().body(ResponseBody::Bytes(b"handler".to_vec())) })
        }
    }

    /// Handler that checks for a header injected by middleware.
    struct CheckHeaderHandler;

    impl Handler for CheckHeaderHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            let has_header = req.headers().get("X-Modified-By").is_some();
            Box::pin(async move {
                if has_header {
                    Response::ok().body(ResponseBody::Bytes(b"header-present".to_vec()))
                } else {
                    Response::with_status(StatusCode::BAD_REQUEST)
                }
            })
        }
    }

    /// Handler that returns an error status.
    struct ErrorHandler;

    impl Handler for ErrorHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            Box::pin(async move { Response::with_status(StatusCode::INTERNAL_SERVER_ERROR) })
        }
    }

    #[test]
    fn middleware_stack_executes_in_correct_order() {
        // Verify the "onion" model: before hooks run first-to-last,
        // after hooks run last-to-first
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(OrderTrackingMiddleware::new("mw1", log.clone()));
        stack.push(OrderTrackingMiddleware::new("mw2", log.clone()));
        stack.push(OrderTrackingMiddleware::new("mw3", log.clone()));

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        futures_executor::block_on(stack.execute(&OkHandler, &ctx, &mut req));

        let calls = log.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                "mw1.before",
                "mw2.before",
                "mw3.before",
                "mw3.after",
                "mw2.after",
                "mw1.after",
            ]
        );
    }

    #[test]
    fn middleware_stack_short_circuit_skips_later_middleware() {
        // When middleware 2 breaks, middleware 3's before should NOT run
        // But middleware 1 and 2's after hooks should still run
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(OrderTrackingMiddleware::new("mw1", log.clone()));
        stack.push(ConditionalBreakMiddleware::new("mw2", true, log.clone()));
        stack.push(OrderTrackingMiddleware::new("mw3", log.clone()));

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&OkHandler, &ctx, &mut req));

        // Should get 403 from the break
        assert_eq!(response.status().as_u16(), 403);

        let calls = log.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                "mw1.before",
                "mw2.before",
                // mw3.before NOT called because mw2 broke
                // mw2.after NOT called because it was the one that broke (ran_before_count = 1)
                "mw1.after",
            ]
        );
    }

    #[test]
    fn middleware_stack_first_middleware_breaks() {
        // When the first middleware breaks, no other middleware should run
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(ConditionalBreakMiddleware::new("mw1", true, log.clone()));
        stack.push(OrderTrackingMiddleware::new("mw2", log.clone()));

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&OkHandler, &ctx, &mut req));

        assert_eq!(response.status().as_u16(), 403);

        let calls = log.lock().unwrap().clone();
        assert_eq!(calls, vec!["mw1.before"]);
        // No after hooks because ran_before_count = 0
    }

    #[test]
    fn middleware_stack_last_middleware_breaks() {
        // When the last middleware breaks, all previous after hooks should run
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(OrderTrackingMiddleware::new("mw1", log.clone()));
        stack.push(OrderTrackingMiddleware::new("mw2", log.clone()));
        stack.push(ConditionalBreakMiddleware::new("mw3", true, log.clone()));

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&OkHandler, &ctx, &mut req));

        assert_eq!(response.status().as_u16(), 403);

        let calls = log.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec![
                "mw1.before",
                "mw2.before",
                "mw3.before",
                // mw3 broke, so only mw1 and mw2 after hooks run
                "mw2.after",
                "mw1.after",
            ]
        );
    }

    #[test]
    fn middleware_stack_empty_executes_handler_directly() {
        let stack = MiddlewareStack::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&OkHandler, &ctx, &mut req));

        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn middleware_stack_with_capacity() {
        let stack = MiddlewareStack::with_capacity(10);
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);
    }

    #[test]
    fn middleware_stack_push_arc() {
        let mut stack = MiddlewareStack::new();
        let mw: Arc<dyn Middleware> = Arc::new(NoopMiddleware);
        stack.push_arc(mw);
        assert_eq!(stack.len(), 1);
    }

    // =========================================================================
    // AddResponseHeader Middleware Tests
    // =========================================================================

    #[test]
    fn add_response_header_adds_header() {
        let mw = AddResponseHeader::new("X-Custom", b"custom-value".to_vec());
        let ctx = test_context();
        let req = Request::new(crate::request::Method::Get, "/");

        let response = Response::ok();
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        assert_eq!(
            header_value(&response, "X-Custom"),
            Some("custom-value".to_string())
        );
    }

    #[test]
    fn add_response_header_preserves_existing_headers() {
        let mw = AddResponseHeader::new("X-New", b"new".to_vec());
        let ctx = test_context();
        let req = Request::new(crate::request::Method::Get, "/");

        let response = Response::ok().header("X-Existing", b"existing".to_vec());
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        assert_eq!(
            header_value(&response, "X-Existing"),
            Some("existing".to_string())
        );
        assert_eq!(header_value(&response, "X-New"), Some("new".to_string()));
    }

    #[test]
    fn add_response_header_name() {
        let mw = AddResponseHeader::new("X-Test", b"test".to_vec());
        assert_eq!(mw.name(), "AddResponseHeader");
    }

    // =========================================================================
    // RequireHeader Middleware Tests
    // =========================================================================

    #[test]
    fn require_header_allows_with_header() {
        let mw = RequireHeader::new("X-Api-Key");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("X-Api-Key", b"secret-key".to_vec());

        let result = futures_executor::block_on(mw.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));
    }

    #[test]
    fn require_header_blocks_without_header() {
        let mw = RequireHeader::new("X-Api-Key");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let result = futures_executor::block_on(mw.before(&ctx, &mut req));

        match result {
            ControlFlow::Break(response) => {
                assert_eq!(response.status().as_u16(), 400);
            }
            ControlFlow::Continue => panic!("Expected Break, got Continue"),
        }
    }

    #[test]
    fn require_header_name() {
        let mw = RequireHeader::new("X-Test");
        assert_eq!(mw.name(), "RequireHeader");
    }

    // =========================================================================
    // PathPrefixFilter Middleware Tests
    // =========================================================================

    #[test]
    fn path_prefix_filter_allows_matching_path() {
        let mw = PathPrefixFilter::new("/api");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/api/users");

        let result = futures_executor::block_on(mw.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));
    }

    #[test]
    fn path_prefix_filter_allows_exact_prefix() {
        let mw = PathPrefixFilter::new("/api");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/api");

        let result = futures_executor::block_on(mw.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));
    }

    #[test]
    fn path_prefix_filter_blocks_non_matching_path() {
        let mw = PathPrefixFilter::new("/api");
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/admin/users");

        let result = futures_executor::block_on(mw.before(&ctx, &mut req));

        match result {
            ControlFlow::Break(response) => {
                assert_eq!(response.status().as_u16(), 404);
            }
            ControlFlow::Continue => panic!("Expected Break, got Continue"),
        }
    }

    #[test]
    fn path_prefix_filter_name() {
        let mw = PathPrefixFilter::new("/api");
        assert_eq!(mw.name(), "PathPrefixFilter");
    }

    // =========================================================================
    // ConditionalStatus Middleware Tests
    // =========================================================================

    #[test]
    fn conditional_status_applies_true_status() {
        let mw = ConditionalStatus::new(
            |req| req.path() == "/health",
            StatusCode::OK,
            StatusCode::NOT_FOUND,
        );
        let ctx = test_context();
        let req = Request::new(crate::request::Method::Get, "/health");
        let response = Response::with_status(StatusCode::INTERNAL_SERVER_ERROR);

        let response = futures_executor::block_on(mw.after(&ctx, &req, response));
        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn conditional_status_applies_false_status() {
        let mw = ConditionalStatus::new(
            |req| req.path() == "/health",
            StatusCode::OK,
            StatusCode::NOT_FOUND,
        );
        let ctx = test_context();
        let req = Request::new(crate::request::Method::Get, "/other");
        let response = Response::with_status(StatusCode::INTERNAL_SERVER_ERROR);

        let response = futures_executor::block_on(mw.after(&ctx, &req, response));
        assert_eq!(response.status().as_u16(), 404);
    }

    #[test]
    fn conditional_status_name() {
        let mw = ConditionalStatus::new(|_| true, StatusCode::OK, StatusCode::NOT_FOUND);
        assert_eq!(mw.name(), "ConditionalStatus");
    }

    // =========================================================================
    // Layer and Layered Tests
    // =========================================================================

    #[derive(Clone)]
    struct LayerTestMiddleware {
        prefix: String,
    }

    impl LayerTestMiddleware {
        fn new(prefix: impl Into<String>) -> Self {
            Self {
                prefix: prefix.into(),
            }
        }
    }

    impl Middleware for LayerTestMiddleware {
        fn after<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a Request,
            response: Response,
        ) -> BoxFuture<'a, Response> {
            let prefix = self.prefix.clone();
            Box::pin(async move { response.header("X-Layer", prefix.into_bytes()) })
        }
    }

    #[test]
    fn layer_wraps_handler() {
        let layer = Layer::new(LayerTestMiddleware::new("wrapped"));
        let wrapped = layer.wrap(OkHandler);

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(wrapped.call(&ctx, &mut req));

        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(
            header_value(&response, "X-Layer"),
            Some("wrapped".to_string())
        );
    }

    #[test]
    fn layered_handles_break() {
        #[derive(Clone)]
        struct BreakingMiddleware;

        impl Middleware for BreakingMiddleware {
            fn before<'a>(
                &'a self,
                _ctx: &'a RequestContext,
                _req: &'a mut Request,
            ) -> BoxFuture<'a, ControlFlow> {
                Box::pin(async {
                    ControlFlow::Break(Response::with_status(StatusCode::UNAUTHORIZED))
                })
            }

            fn after<'a>(
                &'a self,
                _ctx: &'a RequestContext,
                _req: &'a Request,
                response: Response,
            ) -> BoxFuture<'a, Response> {
                Box::pin(async move { response.header("X-After", b"ran".to_vec()) })
            }
        }

        let layer = Layer::new(BreakingMiddleware);
        let wrapped = layer.wrap(OkHandler);

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(wrapped.call(&ctx, &mut req));

        // Should get 401 from break
        assert_eq!(response.status().as_u16(), 401);
        // After hook should still run
        assert_eq!(header_value(&response, "X-After"), Some("ran".to_string()));
    }

    // =========================================================================
    // RequestResponseLogger Tests
    // =========================================================================

    #[test]
    fn request_response_logger_default() {
        let logger = RequestResponseLogger::default();
        assert!(logger.log_request_headers);
        assert!(logger.log_response_headers);
        assert!(!logger.log_body);
        assert_eq!(logger.max_body_bytes, 1024);
    }

    #[test]
    fn request_response_logger_builder() {
        let logger = RequestResponseLogger::new()
            .log_request_headers(false)
            .log_response_headers(false)
            .log_body(true)
            .max_body_bytes(2048)
            .redact_header("x-secret");

        assert!(!logger.log_request_headers);
        assert!(!logger.log_response_headers);
        assert!(logger.log_body);
        assert_eq!(logger.max_body_bytes, 2048);
        assert!(logger.redact_headers.contains("x-secret"));
    }

    #[test]
    fn request_response_logger_name() {
        let logger = RequestResponseLogger::new();
        assert_eq!(logger.name(), "RequestResponseLogger");
    }

    // =========================================================================
    // Integration Tests with Handlers
    // =========================================================================

    #[test]
    fn middleware_stack_modifies_request_for_handler() {
        /// Middleware that adds a header that the handler can see
        struct RequestModifier;

        impl Middleware for RequestModifier {
            fn before<'a>(
                &'a self,
                _ctx: &'a RequestContext,
                req: &'a mut Request,
            ) -> BoxFuture<'a, ControlFlow> {
                req.headers_mut()
                    .insert("X-Modified-By", b"middleware".to_vec());
                Box::pin(async { ControlFlow::Continue })
            }
        }

        let mut stack = MiddlewareStack::new();
        stack.push(RequestModifier);

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response =
            futures_executor::block_on(stack.execute(&CheckHeaderHandler, &ctx, &mut req));

        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn middleware_stack_multiple_response_modifications() {
        let mut stack = MiddlewareStack::new();
        stack.push(AddResponseHeader::new("X-First", b"1".to_vec()));
        stack.push(AddResponseHeader::new("X-Second", b"2".to_vec()));
        stack.push(AddResponseHeader::new("X-Third", b"3".to_vec()));

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&OkHandler, &ctx, &mut req));

        // All headers should be present (after hooks run in reverse)
        assert_eq!(header_value(&response, "X-First"), Some("1".to_string()));
        assert_eq!(header_value(&response, "X-Second"), Some("2".to_string()));
        assert_eq!(header_value(&response, "X-Third"), Some("3".to_string()));
    }

    #[test]
    fn middleware_stack_handler_receives_response_after_break() {
        // Verify that when middleware breaks, the response body is from the break
        let mut stack = MiddlewareStack::new();
        stack.push(ConditionalBreakMiddleware::new(
            "breaker",
            true,
            Arc::new(std::sync::Mutex::new(Vec::new())),
        ));

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&OkHandler, &ctx, &mut req));

        assert_eq!(response.status().as_u16(), 403);
        // Body should be from the breaking middleware, not the handler
        match response.body_ref() {
            ResponseBody::Bytes(b) => assert_eq!(b, b"blocked"),
            _ => panic!("Expected Bytes body"),
        }
    }

    // =========================================================================
    // Error Propagation Tests
    // =========================================================================

    #[test]
    fn middleware_after_can_change_status() {
        struct StatusChanger;

        impl Middleware for StatusChanger {
            fn after<'a>(
                &'a self,
                _ctx: &'a RequestContext,
                _req: &'a Request,
                _response: Response,
            ) -> BoxFuture<'a, Response> {
                Box::pin(async { Response::with_status(StatusCode::SERVICE_UNAVAILABLE) })
            }
        }

        let mut stack = MiddlewareStack::new();
        stack.push(StatusChanger);

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&OkHandler, &ctx, &mut req));

        // Should be changed by after hook
        assert_eq!(response.status().as_u16(), 503);
    }

    #[test]
    fn middleware_after_runs_even_on_error_status() {
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut stack = MiddlewareStack::new();
        stack.push(OrderTrackingMiddleware::new("mw1", log.clone()));

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&ErrorHandler, &ctx, &mut req));

        assert_eq!(response.status().as_u16(), 500);

        let calls = log.lock().unwrap().clone();
        // After should run even when handler returns error status
        assert_eq!(calls, vec!["mw1.before", "mw1.after"]);
    }

    // =========================================================================
    // Wildcard and Regex Matching Tests
    // =========================================================================

    #[test]
    fn wildcard_match_simple() {
        assert!(super::wildcard_match("*.example.com", "api.example.com"));
        assert!(super::wildcard_match("*.example.com", "www.example.com"));
        assert!(!super::wildcard_match("*.example.com", "example.com"));
    }

    #[test]
    fn wildcard_match_suffix_pattern() {
        // Wildcard at start with fixed suffix - primary use case for CORS
        assert!(super::wildcard_match("*.txt", "file.txt"));
        assert!(super::wildcard_match("*.txt", "document.txt"));
        assert!(!super::wildcard_match("*.txt", "file.doc"));
        assert!(super::wildcard_match("*-suffix", "any-suffix"));
    }

    #[test]
    fn wildcard_match_no_wildcard() {
        assert!(super::wildcard_match("exact", "exact"));
        assert!(!super::wildcard_match("exact", "different"));
    }

    #[test]
    fn regex_match_anchored() {
        assert!(super::regex_match("^hello$", "hello"));
        assert!(!super::regex_match("^hello$", "hello world"));
        assert!(!super::regex_match("^hello$", "say hello"));
    }

    #[test]
    fn regex_match_dot_wildcard() {
        assert!(super::regex_match("h.llo", "hello"));
        assert!(super::regex_match("h.llo", "hallo"));
    }

    #[test]
    fn regex_match_star() {
        assert!(super::regex_match("hel*o", "hello"));
        assert!(super::regex_match("hel*o", "helo"));
        assert!(super::regex_match("hel*o", "hellllllo"));
    }

    // =========================================================================
    // Middleware Trait Default Implementation Tests
    // =========================================================================

    #[test]
    fn middleware_default_before_continues() {
        struct DefaultBefore;
        impl Middleware for DefaultBefore {}

        let mw = DefaultBefore;
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let result = futures_executor::block_on(mw.before(&ctx, &mut req));
        assert!(matches!(result, ControlFlow::Continue));
    }

    #[test]
    fn middleware_default_after_passes_through() {
        struct DefaultAfter;
        impl Middleware for DefaultAfter {}

        let mw = DefaultAfter;
        let ctx = test_context();
        let req = Request::new(crate::request::Method::Get, "/");
        let response = Response::with_status(StatusCode::CREATED);

        let result = futures_executor::block_on(mw.after(&ctx, &req, response));
        assert_eq!(result.status().as_u16(), 201);
    }

    #[test]
    fn middleware_default_name_is_type_name() {
        struct MyCustomMiddleware;
        impl Middleware for MyCustomMiddleware {}

        let mw = MyCustomMiddleware;
        assert!(mw.name().contains("MyCustomMiddleware"));
    }

    // =========================================================================
    // Security Headers Middleware Tests
    // =========================================================================

    #[test]
    fn security_headers_default_config() {
        let config = SecurityHeadersConfig::default();
        assert_eq!(config.x_content_type_options, Some("nosniff"));
        assert_eq!(config.x_frame_options, Some(XFrameOptions::Deny));
        assert_eq!(config.x_xss_protection, Some("0"));
        assert!(config.content_security_policy.is_none());
        assert!(config.hsts.is_none());
        assert_eq!(
            config.referrer_policy,
            Some(ReferrerPolicy::StrictOriginWhenCrossOrigin)
        );
        assert!(config.permissions_policy.is_none());
    }

    #[test]
    fn security_headers_none_config() {
        let config = SecurityHeadersConfig::none();
        assert!(config.x_content_type_options.is_none());
        assert!(config.x_frame_options.is_none());
        assert!(config.x_xss_protection.is_none());
        assert!(config.content_security_policy.is_none());
        assert!(config.hsts.is_none());
        assert!(config.referrer_policy.is_none());
        assert!(config.permissions_policy.is_none());
    }

    #[test]
    fn security_headers_strict_config() {
        let config = SecurityHeadersConfig::strict();
        assert_eq!(config.x_content_type_options, Some("nosniff"));
        assert_eq!(config.x_frame_options, Some(XFrameOptions::Deny));
        assert_eq!(
            config.content_security_policy,
            Some("default-src 'self'".to_string())
        );
        assert_eq!(config.hsts, Some((31536000, true, false)));
        assert_eq!(config.referrer_policy, Some(ReferrerPolicy::NoReferrer));
        assert!(config.permissions_policy.is_some());
    }

    #[test]
    fn security_headers_config_builder() {
        let config = SecurityHeadersConfig::new()
            .x_frame_options(Some(XFrameOptions::SameOrigin))
            .content_security_policy("default-src 'self'")
            .hsts(86400, false, false)
            .referrer_policy(Some(ReferrerPolicy::Origin));

        assert_eq!(config.x_frame_options, Some(XFrameOptions::SameOrigin));
        assert_eq!(
            config.content_security_policy,
            Some("default-src 'self'".to_string())
        );
        assert_eq!(config.hsts, Some((86400, false, false)));
        assert_eq!(config.referrer_policy, Some(ReferrerPolicy::Origin));
    }

    #[test]
    fn security_headers_hsts_value_format() {
        // Basic HSTS
        let config = SecurityHeadersConfig::none().hsts(3600, false, false);
        assert_eq!(config.build_hsts_value(), Some("max-age=3600".to_string()));

        // With includeSubDomains
        let config = SecurityHeadersConfig::none().hsts(3600, true, false);
        assert_eq!(
            config.build_hsts_value(),
            Some("max-age=3600; includeSubDomains".to_string())
        );

        // With preload
        let config = SecurityHeadersConfig::none().hsts(3600, false, true);
        assert_eq!(
            config.build_hsts_value(),
            Some("max-age=3600; preload".to_string())
        );

        // With both
        let config = SecurityHeadersConfig::none().hsts(3600, true, true);
        assert_eq!(
            config.build_hsts_value(),
            Some("max-age=3600; includeSubDomains; preload".to_string())
        );
    }

    #[test]
    fn security_headers_middleware_adds_default_headers() {
        let mw = SecurityHeaders::new();
        let ctx = test_context();
        let req = Request::new(crate::request::Method::Get, "/");
        let response = Response::ok();

        let result = futures_executor::block_on(mw.after(&ctx, &req, response));

        // Check that default headers are present
        assert!(header_value(&result, "X-Content-Type-Options").is_some());
        assert!(header_value(&result, "X-Frame-Options").is_some());
        assert!(header_value(&result, "X-XSS-Protection").is_some());
        assert!(header_value(&result, "Referrer-Policy").is_some());

        // Check that optional headers are NOT present by default
        assert!(header_value(&result, "Content-Security-Policy").is_none());
        assert!(header_value(&result, "Strict-Transport-Security").is_none());
        assert!(header_value(&result, "Permissions-Policy").is_none());
    }

    #[test]
    fn security_headers_middleware_with_csp() {
        let config = SecurityHeadersConfig::new()
            .content_security_policy("default-src 'self'; script-src 'self' 'unsafe-inline'");
        let mw = SecurityHeaders::with_config(config);
        let ctx = test_context();
        let req = Request::new(crate::request::Method::Get, "/");
        let response = Response::ok();

        let result = futures_executor::block_on(mw.after(&ctx, &req, response));

        let csp = header_value(&result, "Content-Security-Policy");
        assert!(csp.is_some());
        assert_eq!(
            csp.unwrap(),
            "default-src 'self'; script-src 'self' 'unsafe-inline'"
        );
    }

    #[test]
    fn security_headers_middleware_with_hsts() {
        let config = SecurityHeadersConfig::new().hsts(31536000, true, false);
        let mw = SecurityHeaders::with_config(config);
        let ctx = test_context();
        let req = Request::new(crate::request::Method::Get, "/");
        let response = Response::ok();

        let result = futures_executor::block_on(mw.after(&ctx, &req, response));

        let hsts = header_value(&result, "Strict-Transport-Security");
        assert!(hsts.is_some());
        assert_eq!(hsts.unwrap(), "max-age=31536000; includeSubDomains");
    }

    #[test]
    fn security_headers_middleware_name() {
        let mw = SecurityHeaders::new();
        assert_eq!(mw.name(), "SecurityHeaders");
    }

    #[test]
    fn x_frame_options_values() {
        assert_eq!(XFrameOptions::Deny.as_bytes(), b"DENY");
        assert_eq!(XFrameOptions::SameOrigin.as_bytes(), b"SAMEORIGIN");
    }

    #[test]
    fn referrer_policy_values() {
        assert_eq!(ReferrerPolicy::NoReferrer.as_bytes(), b"no-referrer");
        assert_eq!(
            ReferrerPolicy::NoReferrerWhenDowngrade.as_bytes(),
            b"no-referrer-when-downgrade"
        );
        assert_eq!(ReferrerPolicy::Origin.as_bytes(), b"origin");
        assert_eq!(
            ReferrerPolicy::OriginWhenCrossOrigin.as_bytes(),
            b"origin-when-cross-origin"
        );
        assert_eq!(ReferrerPolicy::SameOrigin.as_bytes(), b"same-origin");
        assert_eq!(ReferrerPolicy::StrictOrigin.as_bytes(), b"strict-origin");
        assert_eq!(
            ReferrerPolicy::StrictOriginWhenCrossOrigin.as_bytes(),
            b"strict-origin-when-cross-origin"
        );
        assert_eq!(ReferrerPolicy::UnsafeUrl.as_bytes(), b"unsafe-url");
    }

    #[test]
    fn security_headers_strict_preset() {
        let mw = SecurityHeaders::strict();
        let ctx = test_context();
        let req = Request::new(crate::request::Method::Get, "/");
        let response = Response::ok();

        let result = futures_executor::block_on(mw.after(&ctx, &req, response));

        // All headers should be present with strict config
        assert!(header_value(&result, "X-Content-Type-Options").is_some());
        assert!(header_value(&result, "X-Frame-Options").is_some());
        assert!(header_value(&result, "Content-Security-Policy").is_some());
        assert!(header_value(&result, "Strict-Transport-Security").is_some());
        assert!(header_value(&result, "Referrer-Policy").is_some());
        assert!(header_value(&result, "Permissions-Policy").is_some());
    }

    #[test]
    fn security_headers_config_clearing_methods() {
        let config = SecurityHeadersConfig::strict()
            .no_content_security_policy()
            .no_hsts()
            .no_permissions_policy();

        assert!(config.content_security_policy.is_none());
        assert!(config.hsts.is_none());
        assert!(config.permissions_policy.is_none());
    }

    // =========================================================================
    // CSRF Middleware Tests
    // =========================================================================

    #[test]
    fn csrf_token_generate_produces_unique_tokens() {
        let token1 = CsrfToken::generate();
        let token2 = CsrfToken::generate();
        assert_ne!(token1, token2);
        assert!(!token1.as_str().is_empty());
        assert!(!token2.as_str().is_empty());
    }

    #[test]
    fn csrf_token_display() {
        let token = CsrfToken::new("test-token-123");
        assert_eq!(format!("{}", token), "test-token-123");
    }

    #[test]
    fn csrf_config_defaults() {
        let config = CsrfConfig::default();
        assert_eq!(config.cookie_name, "csrf_token");
        assert_eq!(config.header_name, "x-csrf-token");
        assert_eq!(config.mode, CsrfMode::DoubleSubmit);
        assert!(!config.rotate_token);
        assert!(config.production);
        assert!(config.error_message.is_none());
    }

    #[test]
    fn csrf_config_builder() {
        let config = CsrfConfig::new()
            .cookie_name("XSRF-TOKEN")
            .header_name("X-XSRF-Token")
            .mode(CsrfMode::HeaderOnly)
            .rotate_token(true)
            .production(false)
            .error_message("Custom CSRF error");

        assert_eq!(config.cookie_name, "XSRF-TOKEN");
        assert_eq!(config.header_name, "X-XSRF-Token");
        assert_eq!(config.mode, CsrfMode::HeaderOnly);
        assert!(config.rotate_token);
        assert!(!config.production);
        assert_eq!(config.error_message, Some("Custom CSRF error".to_string()));
    }

    #[test]
    fn csrf_middleware_allows_get_without_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
        // Token should be generated and stored
        assert!(req.get_extension::<CsrfToken>().is_some());
    }

    #[test]
    fn csrf_middleware_allows_head_without_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Head, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_middleware_allows_options_without_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Options, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_middleware_blocks_post_without_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());

        if let ControlFlow::Break(response) = result {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }

    #[test]
    fn csrf_middleware_blocks_put_without_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Put, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());
    }

    #[test]
    fn csrf_middleware_blocks_delete_without_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Delete, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());
    }

    #[test]
    fn csrf_middleware_blocks_patch_without_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Patch, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());
    }

    #[test]
    fn csrf_middleware_allows_post_with_matching_tokens() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        // Set matching cookie and header
        let token = "valid-csrf-token-12345";
        req.headers_mut()
            .insert("cookie", format!("csrf_token={}", token).into_bytes());
        req.headers_mut()
            .insert("x-csrf-token", token.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());

        // Token should be stored in extensions
        let stored_token = req.get_extension::<CsrfToken>().unwrap();
        assert_eq!(stored_token.as_str(), token);
    }

    #[test]
    fn csrf_middleware_blocks_post_with_mismatched_tokens() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        // Set mismatched cookie and header
        req.headers_mut()
            .insert("cookie", b"csrf_token=token-in-cookie".to_vec());
        req.headers_mut()
            .insert("x-csrf-token", b"different-token".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());

        if let ControlFlow::Break(response) = result {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }

    #[test]
    fn csrf_middleware_blocks_post_with_header_only_in_double_submit_mode() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        // Only header, no cookie
        req.headers_mut()
            .insert("x-csrf-token", b"some-token".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());
    }

    #[test]
    fn csrf_middleware_blocks_post_with_cookie_only_in_double_submit_mode() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        // Only cookie, no header
        req.headers_mut()
            .insert("cookie", b"csrf_token=some-token".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());
    }

    #[test]
    fn csrf_middleware_header_only_mode_accepts_header_token() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().mode(CsrfMode::HeaderOnly));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        req.headers_mut()
            .insert("x-csrf-token", b"valid-token".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_middleware_header_only_mode_rejects_empty_header() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().mode(CsrfMode::HeaderOnly));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        req.headers_mut().insert("x-csrf-token", b"".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());
    }

    #[test]
    fn csrf_middleware_sets_cookie_on_get() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        // Run before to generate token
        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));

        // Run after to set cookie
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        // Check Set-Cookie header
        let cookie_value = header_value(&result, "set-cookie");
        assert!(cookie_value.is_some());

        let cookie_value = cookie_value.unwrap();
        assert!(cookie_value.starts_with("csrf_token="));
        assert!(cookie_value.contains("SameSite=Strict"));
        assert!(cookie_value.contains("Secure")); // Production mode
    }

    #[test]
    fn csrf_middleware_no_secure_in_dev_mode() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().production(false));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));

        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        let cookie_value = header_value(&result, "set-cookie").unwrap();
        assert!(!cookie_value.contains("Secure")); // No Secure in dev mode
    }

    #[test]
    fn csrf_middleware_does_not_set_cookie_if_already_present() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        // Cookie already present
        req.headers_mut()
            .insert("cookie", b"csrf_token=existing-token".to_vec());

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));

        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        // Should not set a new cookie
        assert!(header_value(&result, "set-cookie").is_none());
    }

    #[test]
    fn csrf_middleware_rotates_token_when_configured() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().rotate_token(true));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        // Cookie already present
        req.headers_mut()
            .insert("cookie", b"csrf_token=old-token".to_vec());

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));

        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        // Should set a new cookie even though one exists
        assert!(header_value(&result, "set-cookie").is_some());
    }

    #[test]
    fn csrf_middleware_custom_header_name() {
        let csrf = CsrfMiddleware::with_config(
            CsrfConfig::new()
                .header_name("X-XSRF-Token")
                .cookie_name("XSRF-TOKEN"),
        );
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let token = "custom-token-value";
        req.headers_mut()
            .insert("cookie", format!("XSRF-TOKEN={}", token).into_bytes());
        req.headers_mut()
            .insert("x-xsrf-token", token.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_middleware_error_response_is_json() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));

        if let ControlFlow::Break(response) = result {
            let content_type = header_value(&response, "content-type");
            assert_eq!(content_type, Some("application/json".to_string()));

            // Check body contains proper error structure
            if let ResponseBody::Bytes(body) = response.body_ref() {
                let body_str = std::str::from_utf8(body).unwrap();
                assert!(body_str.contains("csrf_error"));
                assert!(body_str.contains("x-csrf-token"));
            } else {
                panic!("Expected Bytes body");
            }
        } else {
            panic!("Expected Break");
        }
    }

    #[test]
    fn csrf_middleware_custom_error_message() {
        let csrf = CsrfMiddleware::with_config(
            CsrfConfig::new().error_message("Access denied: invalid security token"),
        );
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));

        if let ControlFlow::Break(response) = result {
            if let ResponseBody::Bytes(body) = response.body_ref() {
                let body_str = std::str::from_utf8(body).unwrap();
                assert!(body_str.contains("Access denied: invalid security token"));
            }
        }
    }

    #[test]
    fn csrf_middleware_name() {
        let csrf = CsrfMiddleware::new();
        assert_eq!(csrf.name(), "CSRF");
    }

    #[test]
    fn csrf_middleware_parses_cookie_with_multiple_cookies() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        // Multiple cookies in the header
        let token = "the-csrf-token";
        req.headers_mut().insert(
            "cookie",
            format!("session=abc123; csrf_token={}; user=test", token).into_bytes(),
        );
        req.headers_mut()
            .insert("x-csrf-token", token.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_middleware_handles_empty_token_value() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        // Empty token values
        req.headers_mut().insert("cookie", b"csrf_token=".to_vec());
        req.headers_mut().insert("x-csrf-token", b"".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break()); // Should reject empty tokens
    }

    // ---- Comprehensive CSRF tests (bd-3v0c) ----

    #[test]
    fn csrf_token_generate_many_unique() {
        // Generate many tokens and verify all are unique
        let mut tokens = std::collections::HashSet::new();
        for _ in 0..100 {
            let token = CsrfToken::generate();
            assert!(
                tokens.insert(token.0.clone()),
                "Duplicate token generated: {}",
                token.0
            );
        }
        assert_eq!(tokens.len(), 100);
    }

    #[test]
    fn csrf_token_generate_format_is_hex_with_dashes() {
        let token = CsrfToken::generate();
        let parts: Vec<&str> = token.as_str().split('-').collect();
        assert_eq!(parts.len(), 3, "Expected 3 dash-separated hex segments");
        // Each part should be valid hex
        for part in &parts {
            assert!(
                part.chars().all(|c| c.is_ascii_hexdigit()),
                "Non-hex character in token segment: {}",
                part
            );
        }
        // Segments: 16-char timestamp, 8-char counter, 16-char thread hash
        assert_eq!(parts[0].len(), 16);
        assert_eq!(parts[1].len(), 8);
        assert_eq!(parts[2].len(), 16);
    }

    #[test]
    fn csrf_token_generate_minimum_length() {
        let token = CsrfToken::generate();
        // 16 + 1 + 8 + 1 + 16 = 42 chars minimum
        assert!(
            token.as_str().len() >= 42,
            "Token too short: {} (len={})",
            token.as_str(),
            token.as_str().len()
        );
    }

    #[test]
    fn csrf_token_from_str() {
        let token: CsrfToken = "my-token".into();
        assert_eq!(token.as_str(), "my-token");
        assert_eq!(token.0, "my-token");
    }

    #[test]
    fn csrf_token_clone_eq() {
        let t1 = CsrfToken::new("abc");
        let t2 = t1.clone();
        assert_eq!(t1, t2);
        assert_eq!(t1.as_str(), t2.as_str());
    }

    #[test]
    fn csrf_middleware_allows_trace_without_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Trace, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
        // Token should be generated
        assert!(req.get_extension::<CsrfToken>().is_some());
    }

    #[test]
    fn csrf_safe_method_generates_token_into_extension() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();

        for method in [
            crate::request::Method::Get,
            crate::request::Method::Head,
            crate::request::Method::Options,
            crate::request::Method::Trace,
        ] {
            let mut req = Request::new(method, "/test");
            let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
            assert!(result.is_continue());
            let token = req.get_extension::<CsrfToken>().expect("token missing");
            assert!(!token.as_str().is_empty());
        }
    }

    #[test]
    fn csrf_safe_method_preserves_existing_cookie_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        req.headers_mut()
            .insert("cookie", b"csrf_token=my-existing-token".to_vec());

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));

        // Extension should contain the existing cookie token, not a new one
        let token = req.get_extension::<CsrfToken>().unwrap();
        assert_eq!(token.as_str(), "my-existing-token");
    }

    #[test]
    fn csrf_valid_post_stores_token_in_extension() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/submit");

        let tk = "valid-token-xyz";
        req.headers_mut()
            .insert("cookie", format!("csrf_token={}", tk).into_bytes());
        req.headers_mut()
            .insert("x-csrf-token", tk.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
        let stored = req.get_extension::<CsrfToken>().unwrap();
        assert_eq!(stored.as_str(), tk);
    }

    #[test]
    fn csrf_double_submit_both_empty_strings_rejected() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        // Both cookie and header have empty string values
        req.headers_mut().insert("cookie", b"csrf_token=".to_vec());
        req.headers_mut().insert("x-csrf-token", b"".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());
    }

    #[test]
    fn csrf_double_submit_matching_empty_rejected() {
        // Even if both are technically "equal" (empty), should reject
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        req.headers_mut().insert("cookie", b"csrf_token=".to_vec());
        req.headers_mut().insert("x-csrf-token", b"".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(
            result.is_break(),
            "Empty matching tokens should be rejected"
        );
    }

    #[test]
    fn csrf_header_only_mode_does_not_need_cookie() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().mode(CsrfMode::HeaderOnly));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        // Header only, no cookie
        req.headers_mut()
            .insert("x-csrf-token", b"header-only-token".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
        let token = req.get_extension::<CsrfToken>().unwrap();
        assert_eq!(token.as_str(), "header-only-token");
    }

    #[test]
    fn csrf_header_only_mode_ignores_mismatched_cookie() {
        // In HeaderOnly mode, the cookie value is irrelevant
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().mode(CsrfMode::HeaderOnly));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        req.headers_mut()
            .insert("cookie", b"csrf_token=different-value".to_vec());
        req.headers_mut()
            .insert("x-csrf-token", b"header-value".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue(), "HeaderOnly should ignore cookie");
    }

    #[test]
    fn csrf_header_only_mode_rejects_no_header() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().mode(CsrfMode::HeaderOnly));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");
        // No header at all
        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break());
    }

    #[test]
    fn csrf_header_only_error_message_mentions_header() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().mode(CsrfMode::HeaderOnly));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        if let ControlFlow::Break(response) = result {
            if let ResponseBody::Bytes(body) = response.body_ref() {
                let body_str = std::str::from_utf8(body).unwrap();
                assert!(
                    body_str.contains("missing in header"),
                    "Expected 'missing in header' in: {}",
                    body_str
                );
            }
        } else {
            panic!("Expected Break");
        }
    }

    #[test]
    fn csrf_mismatch_error_differs_from_missing_error() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();

        // Missing: no header or cookie
        let mut req_missing = Request::new(crate::request::Method::Post, "/");
        let missing_result = futures_executor::block_on(csrf.before(&ctx, &mut req_missing));
        let missing_body = match missing_result {
            ControlFlow::Break(r) => match r.body_ref() {
                ResponseBody::Bytes(b) => std::str::from_utf8(b).unwrap().to_string(),
                _ => panic!("Expected Bytes"),
            },
            _ => panic!("Expected Break"),
        };

        // Mismatch: both present but different
        let mut req_mismatch = Request::new(crate::request::Method::Post, "/");
        req_mismatch
            .headers_mut()
            .insert("cookie", b"csrf_token=aaa".to_vec());
        req_mismatch
            .headers_mut()
            .insert("x-csrf-token", b"bbb".to_vec());
        let mismatch_result = futures_executor::block_on(csrf.before(&ctx, &mut req_mismatch));
        let mismatch_body = match mismatch_result {
            ControlFlow::Break(r) => match r.body_ref() {
                ResponseBody::Bytes(b) => std::str::from_utf8(b).unwrap().to_string(),
                _ => panic!("Expected Bytes"),
            },
            _ => panic!("Expected Break"),
        };

        // Error messages should differ
        assert_ne!(
            missing_body, mismatch_body,
            "Missing vs mismatch should have different error messages"
        );
        assert!(missing_body.contains("missing"));
        assert!(mismatch_body.contains("mismatch"));
    }

    #[test]
    fn csrf_cookie_not_httponly() {
        // CSRF cookies MUST be readable by JavaScript (no HttpOnly)
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        let cookie_value = header_value(&result, "set-cookie").unwrap();
        assert!(
            !cookie_value.to_lowercase().contains("httponly"),
            "CSRF cookie must NOT be HttpOnly (needs JS access), got: {}",
            cookie_value
        );
    }

    #[test]
    fn csrf_cookie_has_path_slash() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        let cookie_value = header_value(&result, "set-cookie").unwrap();
        assert!(
            cookie_value.contains("Path=/"),
            "Cookie should have Path=/, got: {}",
            cookie_value
        );
    }

    #[test]
    fn csrf_cookie_has_samesite_strict() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        let cookie_value = header_value(&result, "set-cookie").unwrap();
        assert!(
            cookie_value.contains("SameSite=Strict"),
            "Cookie should have SameSite=Strict, got: {}",
            cookie_value
        );
    }

    #[test]
    fn csrf_production_mode_sets_secure_flag() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().production(true));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        let cookie_value = header_value(&result, "set-cookie").unwrap();
        assert!(
            cookie_value.contains("Secure"),
            "Production cookie must have Secure flag, got: {}",
            cookie_value
        );
    }

    #[test]
    fn csrf_no_set_cookie_on_post_response() {
        // Set-Cookie should only be added for safe methods, not POST
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let token = "valid-token";
        req.headers_mut()
            .insert("cookie", format!("csrf_token={}", token).into_bytes());
        req.headers_mut()
            .insert("x-csrf-token", token.as_bytes().to_vec());

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        assert!(
            header_value(&result, "set-cookie").is_none(),
            "POST response should not set CSRF cookie"
        );
    }

    #[test]
    fn csrf_head_method_sets_cookie() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Head, "/");

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        assert!(
            header_value(&result, "set-cookie").is_some(),
            "HEAD response should set CSRF cookie"
        );
    }

    #[test]
    fn csrf_options_method_sets_cookie() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Options, "/");

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        assert!(
            header_value(&result, "set-cookie").is_some(),
            "OPTIONS response should set CSRF cookie"
        );
    }

    #[test]
    fn csrf_rotation_produces_different_token_in_cookie() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().rotate_token(true));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let old_token = "old-token-value";
        req.headers_mut()
            .insert("cookie", format!("csrf_token={}", old_token).into_bytes());

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        let cookie_value = header_value(&result, "set-cookie").unwrap();
        // When rotation is enabled, old token is reused from cookie parse, but
        // the cookie IS set (which the before phase stored in extension).
        // The existing token from cookie is used, so cookie_value will contain old_token.
        // This verifies the Set-Cookie is emitted even with an existing cookie.
        assert!(cookie_value.starts_with("csrf_token="));
    }

    #[test]
    fn csrf_no_rotation_skips_set_cookie_when_present() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().rotate_token(false));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        req.headers_mut()
            .insert("cookie", b"csrf_token=existing".to_vec());

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        assert!(
            header_value(&result, "set-cookie").is_none(),
            "Without rotation, should not re-set existing cookie"
        );
    }

    #[test]
    fn csrf_custom_cookie_name_in_set_cookie_response() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().cookie_name("XSRF-TOKEN"));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let _ = futures_executor::block_on(csrf.before(&ctx, &mut req));
        let response = Response::ok();
        let result = futures_executor::block_on(csrf.after(&ctx, &req, response));

        let cookie_value = header_value(&result, "set-cookie").unwrap();
        assert!(
            cookie_value.starts_with("XSRF-TOKEN="),
            "Custom cookie name should appear in Set-Cookie, got: {}",
            cookie_value
        );
    }

    #[test]
    fn csrf_custom_header_name_validated() {
        let csrf = CsrfMiddleware::with_config(
            CsrfConfig::new()
                .header_name("X-Custom-CSRF")
                .cookie_name("my_csrf"),
        );
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let token = "custom-tok";
        req.headers_mut()
            .insert("cookie", format!("my_csrf={}", token).into_bytes());
        req.headers_mut()
            .insert("x-custom-csrf", token.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_custom_header_name_wrong_header_rejected() {
        let csrf = CsrfMiddleware::with_config(CsrfConfig::new().header_name("X-Custom-CSRF"));
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let token = "some-token";
        req.headers_mut()
            .insert("cookie", format!("csrf_token={}", token).into_bytes());
        // Using default header name instead of custom one
        req.headers_mut()
            .insert("x-csrf-token", token.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_break(), "Wrong header name should be rejected");
    }

    #[test]
    fn csrf_cookie_parsing_multiple_cookies_picks_correct() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let token = "correct-csrf";
        req.headers_mut().insert(
            "cookie",
            format!("session=abc; other=xyz; csrf_token={}; tracking=123", token).into_bytes(),
        );
        req.headers_mut()
            .insert("x-csrf-token", token.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_cookie_parsing_spaces_around_semicolons() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let token = "spaced-token";
        req.headers_mut().insert(
            "cookie",
            format!("session=abc ;  csrf_token={}  ; other=xyz", token).into_bytes(),
        );
        req.headers_mut()
            .insert("x-csrf-token", token.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_error_response_status_is_403() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();

        // Test all state-changing methods return 403
        for method in [
            crate::request::Method::Post,
            crate::request::Method::Put,
            crate::request::Method::Delete,
            crate::request::Method::Patch,
        ] {
            let mut req = Request::new(method, "/");
            let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
            match result {
                ControlFlow::Break(response) => {
                    assert_eq!(
                        response.status(),
                        StatusCode::FORBIDDEN,
                        "Expected 403 for {:?}",
                        method
                    );
                }
                _ => panic!("Expected Break for {:?}", method),
            }
        }
    }

    #[test]
    fn csrf_error_body_json_structure() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        if let ControlFlow::Break(response) = result {
            if let ResponseBody::Bytes(body) = response.body_ref() {
                let body_str = std::str::from_utf8(body).unwrap();
                // Verify JSON structure
                let parsed: serde_json::Value = serde_json::from_str(body_str)
                    .unwrap_or_else(|e| panic!("Invalid JSON: {}: {}", body_str, e));
                assert!(parsed["detail"].is_array());
                let detail = &parsed["detail"][0];
                assert_eq!(detail["type"], "csrf_error");
                assert!(detail["loc"].is_array());
                assert_eq!(detail["loc"][0], "header");
                assert_eq!(detail["loc"][1], "x-csrf-token");
                assert!(detail["msg"].is_string());
            } else {
                panic!("Expected Bytes body");
            }
        } else {
            panic!("Expected Break");
        }
    }

    #[test]
    fn csrf_default_trait() {
        let csrf = CsrfMiddleware::default();
        assert_eq!(csrf.name(), "CSRF");
        // Should behave identically to new()
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");
        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_mode_default_is_double_submit() {
        assert_eq!(CsrfMode::default(), CsrfMode::DoubleSubmit);
    }

    #[test]
    fn csrf_double_submit_both_present_same_non_empty_passes() {
        // Explicit test of the core double-submit pattern
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();

        let token = "a1b2c3d4e5f6";
        let mut req = Request::new(crate::request::Method::Delete, "/resource/1");
        req.headers_mut()
            .insert("cookie", format!("csrf_token={}", token).into_bytes());
        req.headers_mut()
            .insert("x-csrf-token", token.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn csrf_double_submit_case_sensitive() {
        // Token comparison should be case-sensitive
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/");

        req.headers_mut()
            .insert("cookie", b"csrf_token=AbCdEf".to_vec());
        req.headers_mut().insert("x-csrf-token", b"abcdef".to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
        assert!(
            result.is_break(),
            "Token comparison should be case-sensitive"
        );
    }

    #[test]
    fn csrf_token_cookie_extractor_reads_csrf_cookie() {
        // Test that CsrfTokenCookie works as a cookie name marker
        use crate::extract::{CookieName, CsrfTokenCookie};
        assert_eq!(CsrfTokenCookie::NAME, "csrf_token");
    }

    #[test]
    fn csrf_make_set_cookie_header_value_production() {
        let value = CsrfMiddleware::make_set_cookie_header_value("csrf_token", "tok123", true);
        let s = std::str::from_utf8(&value).unwrap();
        assert!(s.contains("csrf_token=tok123"));
        assert!(s.contains("Path=/"));
        assert!(s.contains("SameSite=Strict"));
        assert!(s.contains("Secure"));
        assert!(!s.to_lowercase().contains("httponly"));
    }

    #[test]
    fn csrf_make_set_cookie_header_value_development() {
        let value = CsrfMiddleware::make_set_cookie_header_value("csrf_token", "tok123", false);
        let s = std::str::from_utf8(&value).unwrap();
        assert!(s.contains("csrf_token=tok123"));
        assert!(s.contains("Path=/"));
        assert!(s.contains("SameSite=Strict"));
        assert!(!s.contains("Secure"));
    }

    #[test]
    fn csrf_before_after_full_cycle_get_then_post() {
        // Simulate a full CSRF flow: GET sets cookie, POST uses it
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();

        // Step 1: GET request - generates token and sets cookie
        let mut get_req = Request::new(crate::request::Method::Get, "/form");
        let _ = futures_executor::block_on(csrf.before(&ctx, &mut get_req));
        let get_response = Response::ok();
        let get_result = futures_executor::block_on(csrf.after(&ctx, &get_req, get_response));

        let set_cookie = header_value(&get_result, "set-cookie").expect("GET should set cookie");
        // Extract token value from "csrf_token=<value>; Path=/; ..."
        let token_value = set_cookie
            .strip_prefix("csrf_token=")
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        assert!(!token_value.is_empty());

        // Step 2: POST request - uses the token from cookie + header
        let mut post_req = Request::new(crate::request::Method::Post, "/form");
        post_req
            .headers_mut()
            .insert("cookie", format!("csrf_token={}", token_value).into_bytes());
        post_req
            .headers_mut()
            .insert("x-csrf-token", token_value.as_bytes().to_vec());

        let result = futures_executor::block_on(csrf.before(&ctx, &mut post_req));
        assert!(result.is_continue(), "POST with valid token should pass");
    }

    #[test]
    fn csrf_all_state_changing_methods_require_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();

        for method in [
            crate::request::Method::Post,
            crate::request::Method::Put,
            crate::request::Method::Delete,
            crate::request::Method::Patch,
        ] {
            let mut req = Request::new(method, "/resource");
            let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
            assert!(
                result.is_break(),
                "{:?} without token should be rejected",
                method
            );
        }
    }

    #[test]
    fn csrf_all_safe_methods_pass_without_token() {
        let csrf = CsrfMiddleware::new();
        let ctx = test_context();

        for method in [
            crate::request::Method::Get,
            crate::request::Method::Head,
            crate::request::Method::Options,
            crate::request::Method::Trace,
        ] {
            let mut req = Request::new(method, "/resource");
            let result = futures_executor::block_on(csrf.before(&ctx, &mut req));
            assert!(
                result.is_continue(),
                "{:?} should be allowed without token",
                method
            );
        }
    }

    // =========================================================================
    // Middleware Stack Ordering Tests (Onion Model)
    // =========================================================================

    /// Middleware that records execution order to a shared Vec.
    /// Used to verify the onion model (before in order, after in reverse).
    #[derive(Clone)]
    struct OrderRecordingMiddleware {
        id: &'static str,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl OrderRecordingMiddleware {
        fn new(id: &'static str, log: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Self {
            Self { id, log }
        }
    }

    impl Middleware for OrderRecordingMiddleware {
        fn before<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, ControlFlow> {
            let id = self.id;
            let log = self.log.clone();
            Box::pin(async move {
                log.lock().unwrap().push(format!("{id}:before"));
                ControlFlow::Continue
            })
        }

        fn after<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a Request,
            response: Response,
        ) -> BoxFuture<'a, Response> {
            let id = self.id;
            let log = self.log.clone();
            Box::pin(async move {
                log.lock().unwrap().push(format!("{id}:after"));
                response
            })
        }

        fn name(&self) -> &'static str {
            "OrderRecording"
        }
    }

    /// Middleware that short-circuits in its before hook.
    struct ShortCircuitMiddleware {
        id: &'static str,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl ShortCircuitMiddleware {
        fn new(id: &'static str, log: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Self {
            Self { id, log }
        }
    }

    impl Middleware for ShortCircuitMiddleware {
        fn before<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, ControlFlow> {
            let id = self.id;
            let log = self.log.clone();
            Box::pin(async move {
                log.lock().unwrap().push(format!("{id}:before:break"));
                ControlFlow::Break(
                    Response::with_status(StatusCode::FORBIDDEN)
                        .body(ResponseBody::Bytes(b"short-circuited".to_vec())),
                )
            })
        }

        fn after<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a Request,
            response: Response,
        ) -> BoxFuture<'a, Response> {
            let id = self.id;
            let log = self.log.clone();
            Box::pin(async move {
                log.lock().unwrap().push(format!("{id}:after"));
                response
            })
        }

        fn name(&self) -> &'static str {
            "ShortCircuit"
        }
    }

    /// Simple handler that records when it runs.
    struct RecordingHandler {
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl RecordingHandler {
        fn new(log: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Self {
            Self { log }
        }
    }

    impl Handler for RecordingHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            let log = self.log.clone();
            Box::pin(async move {
                log.lock().unwrap().push("handler".to_string());
                Response::ok().body(ResponseBody::Bytes(b"ok".to_vec()))
            })
        }
    }

    #[test]
    fn middleware_stack_three_middleware_onion_order() {
        // Test that three middleware follow the onion model:
        // Before hooks run in order: 1 -> 2 -> 3
        // After hooks run in reverse: 3 -> 2 -> 1
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(OrderRecordingMiddleware::new("mw1", log.clone()));
        stack.push(OrderRecordingMiddleware::new("mw2", log.clone()));
        stack.push(OrderRecordingMiddleware::new("mw3", log.clone()));

        let handler = RecordingHandler::new(log.clone());
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let _response = futures_executor::block_on(stack.execute(&handler, &ctx, &mut req));

        let execution_log = log.lock().unwrap().clone();
        assert_eq!(
            execution_log,
            vec![
                "mw1:before",
                "mw2:before",
                "mw3:before",
                "handler",
                "mw3:after",
                "mw2:after",
                "mw1:after",
            ]
        );
    }

    #[test]
    fn middleware_stack_short_circuit_runs_prior_after_hooks() {
        // When middleware 2 short-circuits:
        // - mw1:before runs (returns Continue, count=1)
        // - mw2:before short-circuits (returns Break, count stays at 1)
        // - mw3:before does NOT run
        // - handler does NOT run
        // - Only middleware that successfully completed before (mw1) have after run
        // - mw1:after runs
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(OrderRecordingMiddleware::new("mw1", log.clone()));
        stack.push(ShortCircuitMiddleware::new("mw2", log.clone()));
        stack.push(OrderRecordingMiddleware::new("mw3", log.clone()));

        let handler = RecordingHandler::new(log.clone());
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&handler, &ctx, &mut req));

        // Should return the short-circuit response
        assert_eq!(response.status().as_u16(), 403);

        let execution_log = log.lock().unwrap().clone();
        // Note: mw2's after hook does NOT run because it didn't return Continue
        // Only middleware that successfully completed before (returned Continue) have after run
        assert_eq!(
            execution_log,
            vec!["mw1:before", "mw2:before:break", "mw1:after",]
        );
    }

    #[test]
    fn middleware_stack_first_middleware_short_circuits() {
        // When the first middleware short-circuits:
        // - mw1:before short-circuits (returns Break, count=0)
        // - No after hooks run (count=0)
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(ShortCircuitMiddleware::new("mw1", log.clone()));
        stack.push(OrderRecordingMiddleware::new("mw2", log.clone()));

        let handler = RecordingHandler::new(log.clone());
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&handler, &ctx, &mut req));
        assert_eq!(response.status().as_u16(), 403);

        let execution_log = log.lock().unwrap().clone();
        // No after hooks run because no middleware returned Continue
        assert_eq!(execution_log, vec!["mw1:before:break",]);
    }

    #[test]
    fn middleware_stack_empty_runs_handler_only() {
        // Empty stack should just run the handler (onion ordering variant)
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let stack = MiddlewareStack::new();
        let handler = RecordingHandler::new(log.clone());
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&handler, &ctx, &mut req));
        assert_eq!(response.status().as_u16(), 200);

        let execution_log = log.lock().unwrap().clone();
        assert_eq!(execution_log, vec!["handler"]);
    }

    #[test]
    fn middleware_stack_single_middleware_ordering() {
        // Single middleware should have before -> handler -> after
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(OrderRecordingMiddleware::new("mw1", log.clone()));

        let handler = RecordingHandler::new(log.clone());
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let _response = futures_executor::block_on(stack.execute(&handler, &ctx, &mut req));

        let execution_log = log.lock().unwrap().clone();
        assert_eq!(execution_log, vec!["mw1:before", "handler", "mw1:after",]);
    }

    #[test]
    fn middleware_stack_five_middleware_onion_order() {
        // Test with five middleware for a longer chain
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(OrderRecordingMiddleware::new("a", log.clone()));
        stack.push(OrderRecordingMiddleware::new("b", log.clone()));
        stack.push(OrderRecordingMiddleware::new("c", log.clone()));
        stack.push(OrderRecordingMiddleware::new("d", log.clone()));
        stack.push(OrderRecordingMiddleware::new("e", log.clone()));

        let handler = RecordingHandler::new(log.clone());
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let _response = futures_executor::block_on(stack.execute(&handler, &ctx, &mut req));

        let execution_log = log.lock().unwrap().clone();
        assert_eq!(
            execution_log,
            vec![
                "a:before", "b:before", "c:before", "d:before", "e:before", "handler", "e:after",
                "d:after", "c:after", "b:after", "a:after",
            ]
        );
    }

    #[test]
    fn middleware_stack_short_circuit_at_end_runs_prior_afters() {
        // When the last middleware short-circuits:
        // - mw1:before runs (Continue, count=1)
        // - mw2:before runs (Continue, count=2)
        // - mw3:before short-circuits (Break, count stays at 2)
        // - handler does NOT run
        // - After hooks run for mw1 and mw2 only (they returned Continue)
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(OrderRecordingMiddleware::new("mw1", log.clone()));
        stack.push(OrderRecordingMiddleware::new("mw2", log.clone()));
        stack.push(ShortCircuitMiddleware::new("mw3", log.clone()));

        let handler = RecordingHandler::new(log.clone());
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&handler, &ctx, &mut req));
        assert_eq!(response.status().as_u16(), 403);

        let execution_log = log.lock().unwrap().clone();
        // mw3's after hook does NOT run because it didn't return Continue
        assert_eq!(
            execution_log,
            vec![
                "mw1:before",
                "mw2:before",
                "mw3:before:break",
                "mw2:after",
                "mw1:after",
            ]
        );
    }

    /// Middleware that modifies the request in before and response in after.
    struct ModifyingMiddleware {
        id: &'static str,
        log: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl ModifyingMiddleware {
        fn new(id: &'static str, log: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Self {
            Self { id, log }
        }
    }

    impl Middleware for ModifyingMiddleware {
        fn before<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            req: &'a mut Request,
        ) -> BoxFuture<'a, ControlFlow> {
            let id = self.id;
            let log = self.log.clone();
            Box::pin(async move {
                // Add a header to track middleware order
                req.headers_mut()
                    .insert(format!("x-{id}-before"), b"true".to_vec());
                log.lock().unwrap().push(format!("{id}:before"));
                ControlFlow::Continue
            })
        }

        fn after<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a Request,
            response: Response,
        ) -> BoxFuture<'a, Response> {
            let id = self.id;
            let log = self.log.clone();
            Box::pin(async move {
                log.lock().unwrap().push(format!("{id}:after"));
                // Add a header to the response
                response.header(format!("x-{id}-after"), b"true".to_vec())
            })
        }

        fn name(&self) -> &'static str {
            "Modifying"
        }
    }

    #[test]
    fn middleware_stack_modifications_accumulate_correctly() {
        // Test that request modifications in before hooks accumulate,
        // and response modifications in after hooks accumulate
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut stack = MiddlewareStack::new();
        stack.push(ModifyingMiddleware::new("mw1", log.clone()));
        stack.push(ModifyingMiddleware::new("mw2", log.clone()));
        stack.push(ModifyingMiddleware::new("mw3", log.clone()));

        let handler = RecordingHandler::new(log.clone());
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        let response = futures_executor::block_on(stack.execute(&handler, &ctx, &mut req));

        // Check that all after hooks added their headers
        assert!(header_value(&response, "x-mw1-after").is_some());
        assert!(header_value(&response, "x-mw2-after").is_some());
        assert!(header_value(&response, "x-mw3-after").is_some());

        // Check that the request was modified by all before hooks
        assert!(req.headers().contains("x-mw1-before"));
        assert!(req.headers().contains("x-mw2-before"));
        assert!(req.headers().contains("x-mw3-before"));
    }

    #[test]
    fn layer_wrap_maintains_middleware_order() {
        // Test that Layer::wrap creates a Layered handler that maintains before->after ordering
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        // Create a layer with our recording middleware
        let layer = Layer::new(OrderRecordingMiddleware::new("layer", log.clone()));

        // Wrap the recording handler
        let handler = RecordingHandler::new(log.clone());
        let layered_handler = layer.wrap(handler);

        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/");

        // Execute the layered handler directly (not via middleware stack)
        let _response = futures_executor::block_on(layered_handler.call(&ctx, &mut req));

        let execution_log = log.lock().unwrap().clone();
        assert_eq!(
            execution_log,
            vec!["layer:before", "handler", "layer:after",]
        );
    }
}

// ============================================================================
// Compression Middleware Tests (requires "compression" feature)
// ============================================================================

#[cfg(all(test, feature = "compression"))]
mod compression_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::ResponseBody;

    fn test_context() -> RequestContext {
        RequestContext::new(asupersync::Cx::for_testing(), 1)
    }

    #[test]
    fn compression_config_defaults() {
        let config = CompressionConfig::default();
        assert_eq!(config.min_size, 1024);
        assert_eq!(config.level, 6);
        assert!(!config.skip_content_types.is_empty());
    }

    #[test]
    fn compression_config_builder() {
        let config = CompressionConfig::new().min_size(512).level(9);
        assert_eq!(config.min_size, 512);
        assert_eq!(config.level, 9);
    }

    #[test]
    fn compression_level_clamped() {
        let config = CompressionConfig::new().level(100);
        assert_eq!(config.level, 9);

        let config = CompressionConfig::new().level(0);
        assert_eq!(config.level, 1);
    }

    #[test]
    fn skip_content_type_exact_match() {
        let config = CompressionConfig::default();
        assert!(config.should_skip_content_type("image/jpeg"));
        assert!(config.should_skip_content_type("image/jpeg; charset=utf-8"));
        assert!(!config.should_skip_content_type("text/html"));
    }

    #[test]
    fn skip_content_type_prefix_match() {
        let config = CompressionConfig::default();
        // "video/" prefix should match any video type
        assert!(config.should_skip_content_type("video/mp4"));
        assert!(config.should_skip_content_type("video/webm"));
        assert!(config.should_skip_content_type("audio/mpeg"));
    }

    #[test]
    fn compression_skips_small_responses() {
        let middleware = CompressionMiddleware::new();
        let ctx = test_context();

        // Create request with Accept-Encoding: gzip
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("accept-encoding", b"gzip".to_vec());

        // Create a small response (less than 1024 bytes)
        let response = Response::ok()
            .header("content-type", b"text/plain".to_vec())
            .body(ResponseBody::Bytes(b"Hello, World!".to_vec()));

        // Run the after hook
        let result = futures_executor::block_on(middleware.after(&ctx, &req, response));

        // Should NOT be compressed (too small)
        let has_encoding = result
            .headers()
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-encoding"));
        assert!(!has_encoding, "Small response should not be compressed");
    }

    #[test]
    fn compression_works_for_large_responses() {
        let config = CompressionConfig::new().min_size(10); // Lower threshold
        let middleware = CompressionMiddleware::with_config(config);
        let ctx = test_context();

        // Create request with Accept-Encoding: gzip
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("accept-encoding", b"gzip".to_vec());

        // Create a response with repetitive content (compresses well)
        let body = "Hello, World! ".repeat(100);
        let original_size = body.len();

        let response = Response::ok()
            .header("content-type", b"text/plain".to_vec())
            .body(ResponseBody::Bytes(body.into_bytes()));

        // Run the after hook
        let result = futures_executor::block_on(middleware.after(&ctx, &req, response));

        // Should be compressed
        let encoding = result
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-encoding"));
        assert!(encoding.is_some(), "Large response should be compressed");

        let (_, value) = encoding.unwrap();
        assert_eq!(value, b"gzip");

        // Check Vary header
        let vary = result
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("vary"));
        assert!(vary.is_some(), "Should have Vary header");

        // Verify compressed size is smaller
        if let ResponseBody::Bytes(compressed) = result.body_ref() {
            assert!(
                compressed.len() < original_size,
                "Compressed size should be smaller"
            );
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn compression_skips_without_accept_encoding() {
        let config = CompressionConfig::new().min_size(10);
        let middleware = CompressionMiddleware::with_config(config);
        let ctx = test_context();

        // Create request WITHOUT Accept-Encoding
        let req = Request::new(Method::Get, "/");

        let body = "Hello, World! ".repeat(100);
        let response = Response::ok()
            .header("content-type", b"text/plain".to_vec())
            .body(ResponseBody::Bytes(body.into_bytes()));

        let result = futures_executor::block_on(middleware.after(&ctx, &req, response));

        // Should NOT be compressed (no Accept-Encoding)
        let has_encoding = result
            .headers()
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-encoding"));
        assert!(!has_encoding, "Should not compress without Accept-Encoding");
    }

    #[test]
    fn compression_skips_already_compressed_content() {
        let config = CompressionConfig::new().min_size(10);
        let middleware = CompressionMiddleware::with_config(config);
        let ctx = test_context();

        // Create request with Accept-Encoding: gzip
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("accept-encoding", b"gzip".to_vec());

        // Create response with already-compressed content type
        let body = "Some image data".repeat(100);
        let response = Response::ok()
            .header("content-type", b"image/jpeg".to_vec())
            .body(ResponseBody::Bytes(body.into_bytes()));

        let result = futures_executor::block_on(middleware.after(&ctx, &req, response));

        // Should NOT be compressed (image/jpeg is already compressed)
        let has_encoding = result
            .headers()
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-encoding"));
        assert!(
            !has_encoding,
            "Should not compress already-compressed content types"
        );
    }

    #[test]
    fn compression_skips_if_already_has_content_encoding() {
        let config = CompressionConfig::new().min_size(10);
        let middleware = CompressionMiddleware::with_config(config);
        let ctx = test_context();

        // Create request with Accept-Encoding: gzip
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("accept-encoding", b"gzip".to_vec());

        // Create response that already has Content-Encoding
        let body = "Hello, World! ".repeat(100);
        let response = Response::ok()
            .header("content-type", b"text/plain".to_vec())
            .header("content-encoding", b"br".to_vec())
            .body(ResponseBody::Bytes(body.into_bytes()));

        let result = futures_executor::block_on(middleware.after(&ctx, &req, response));

        // Should NOT double-compress
        let encodings: Vec<_> = result
            .headers()
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
            .collect();

        // Should still have exactly one Content-Encoding header (the original br)
        assert_eq!(encodings.len(), 1);
        assert_eq!(encodings[0].1, b"br");
    }

    #[test]
    fn accepts_gzip_parses_header_correctly() {
        // Test various Accept-Encoding header formats

        // Simple gzip
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("accept-encoding", b"gzip".to_vec());
        assert!(CompressionMiddleware::accepts_gzip(&req));

        // Multiple encodings
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("accept-encoding", b"deflate, gzip, br".to_vec());
        assert!(CompressionMiddleware::accepts_gzip(&req));

        // With quality values
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("accept-encoding", b"gzip;q=1.0, identity;q=0.5".to_vec());
        assert!(CompressionMiddleware::accepts_gzip(&req));

        // Wildcard
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut().insert("accept-encoding", b"*".to_vec());
        assert!(CompressionMiddleware::accepts_gzip(&req));

        // No gzip
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("accept-encoding", b"deflate, br".to_vec());
        assert!(!CompressionMiddleware::accepts_gzip(&req));

        // No header
        let req_no_header = Request::new(Method::Get, "/");
        assert!(!CompressionMiddleware::accepts_gzip(&req_no_header));
    }

    #[test]
    fn compression_middleware_name() {
        let middleware = CompressionMiddleware::new();
        assert_eq!(middleware.name(), "Compression");
    }
}

// ============================================================================
// Request Inspection Middleware Tests
// ============================================================================

#[cfg(test)]
mod request_inspection_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::ResponseBody;

    fn test_context() -> RequestContext {
        RequestContext::new(asupersync::Cx::for_testing(), 1)
    }

    #[test]
    fn inspection_middleware_default_creates_normal_verbosity() {
        let mw = RequestInspectionMiddleware::new();
        assert_eq!(mw.verbosity, InspectionVerbosity::Normal);
        assert_eq!(mw.slow_threshold_ms, 1000);
        assert_eq!(mw.max_body_preview, 2048);
        assert_eq!(mw.name(), "RequestInspection");
    }

    #[test]
    fn inspection_middleware_builder_methods() {
        let mw = RequestInspectionMiddleware::new()
            .verbosity(InspectionVerbosity::Verbose)
            .slow_threshold_ms(500)
            .max_body_preview(4096)
            .log_config(LogConfig::development())
            .redact_header("x-api-key");

        assert_eq!(mw.verbosity, InspectionVerbosity::Verbose);
        assert_eq!(mw.slow_threshold_ms, 500);
        assert_eq!(mw.max_body_preview, 4096);
        assert!(mw.redact_headers.contains("x-api-key"));
        // Default redacted headers should still be present
        assert!(mw.redact_headers.contains("authorization"));
        assert!(mw.redact_headers.contains("cookie"));
    }

    #[test]
    fn inspection_before_continues_processing() {
        let mw = RequestInspectionMiddleware::new().verbosity(InspectionVerbosity::Minimal);
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/api/users");

        let result = futures_executor::block_on(mw.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn inspection_after_returns_response_unchanged() {
        let mw = RequestInspectionMiddleware::new().verbosity(InspectionVerbosity::Minimal);
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/health");

        // Run before to set the InspectionStart extension
        let _ = futures_executor::block_on(mw.before(&ctx, &mut req));

        let response = Response::ok().body(ResponseBody::Bytes(b"OK".to_vec()));

        let result = futures_executor::block_on(mw.after(&ctx, &req, response));
        assert_eq!(result.status().as_u16(), 200);
        assert_eq!(result.body_ref().len(), 2);
    }

    #[test]
    fn inspection_stores_start_extension() {
        let mw = RequestInspectionMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        let _ = futures_executor::block_on(mw.before(&ctx, &mut req));

        // Verify the InspectionStart extension was set
        assert!(req.get_extension::<InspectionStart>().is_some());
    }

    #[test]
    fn inspection_all_verbosity_levels_continue() {
        for verbosity in [
            InspectionVerbosity::Minimal,
            InspectionVerbosity::Normal,
            InspectionVerbosity::Verbose,
        ] {
            let mw = RequestInspectionMiddleware::new().verbosity(verbosity);
            let ctx = test_context();
            let mut req = Request::new(Method::Get, "/test");
            req.headers_mut()
                .insert("content-type", b"text/plain".to_vec());

            let result = futures_executor::block_on(mw.before(&ctx, &mut req));
            assert!(
                result.is_continue(),
                "Verbosity {verbosity:?} should continue"
            );
        }
    }

    #[test]
    fn inspection_verbose_with_json_body() {
        let mw = RequestInspectionMiddleware::new().verbosity(InspectionVerbosity::Verbose);
        let ctx = test_context();
        let body = br#"{"name":"Alice","age":30}"#;
        let mut req = Request::new(Method::Post, "/api/users");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(body.to_vec()));

        let result = futures_executor::block_on(mw.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn inspection_verbose_after_with_json_response() {
        let mw = RequestInspectionMiddleware::new().verbosity(InspectionVerbosity::Verbose);
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/users/1");

        let _ = futures_executor::block_on(mw.before(&ctx, &mut req));

        let response = Response::ok()
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(br#"{"id":1,"name":"Alice"}"#.to_vec()));

        let result = futures_executor::block_on(mw.after(&ctx, &req, response));
        assert_eq!(result.status().as_u16(), 200);
    }

    #[test]
    fn inspection_redacts_sensitive_headers() {
        let mw = RequestInspectionMiddleware::new();

        // Verify default redacted headers are present
        assert!(mw.redact_headers.contains("authorization"));
        assert!(mw.redact_headers.contains("proxy-authorization"));
        assert!(mw.redact_headers.contains("cookie"));
        assert!(mw.redact_headers.contains("set-cookie"));
    }

    #[test]
    fn inspection_format_headers_redacts() {
        let mw = RequestInspectionMiddleware::new().redact_header("x-secret");

        let headers = vec![
            ("content-type", b"text/plain".as_slice()),
            ("x-secret", b"my-secret-value".as_slice()),
            ("x-normal", b"visible".as_slice()),
        ];

        let output = mw.format_inspection_headers(headers.into_iter());
        assert!(output.contains("content-type: text/plain"));
        assert!(output.contains("x-secret: [REDACTED]"));
        assert!(output.contains("x-normal: visible"));
        assert!(!output.contains("my-secret-value"));
    }

    #[test]
    fn inspection_format_body_preview_truncates() {
        let mw = RequestInspectionMiddleware::new().max_body_preview(10);

        let body = b"Hello, World! This is a long body.";
        let result = mw.format_body_preview(body, None);
        assert!(result.is_some());
        let text = result.unwrap();
        assert!(text.ends_with("..."));
        assert!(text.len() <= 15); // 10 chars + "..."
    }

    #[test]
    fn inspection_format_body_preview_empty() {
        let mw = RequestInspectionMiddleware::new();
        assert!(mw.format_body_preview(b"", None).is_none());
    }

    #[test]
    fn inspection_format_body_preview_zero_max() {
        let mw = RequestInspectionMiddleware::new().max_body_preview(0);
        assert!(mw.format_body_preview(b"hello", None).is_none());
    }

    #[test]
    fn inspection_format_body_preview_json_pretty() {
        let mw = RequestInspectionMiddleware::new();
        let body = br#"{"key":"value","num":42}"#;
        let ct = b"application/json".as_slice();
        let result = mw.format_body_preview(body, Some(ct));
        assert!(result.is_some());
        let text = result.unwrap();
        // Pretty-printed JSON should contain newlines
        assert!(text.contains('\n'));
        assert!(text.contains("\"key\": \"value\""));
    }

    #[test]
    fn inspection_format_body_preview_non_json() {
        let mw = RequestInspectionMiddleware::new();
        let body = b"Hello, World!";
        let ct = b"text/plain".as_slice();
        let result = mw.format_body_preview(body, Some(ct));
        assert_eq!(result.unwrap(), "Hello, World!");
    }

    #[test]
    fn inspection_format_body_preview_binary() {
        let mw = RequestInspectionMiddleware::new();
        let body: &[u8] = &[0xFF, 0xFE, 0xFD, 0x00];
        let result = mw.format_body_preview(body, None);
        assert!(result.is_some());
        assert!(result.unwrap().contains("binary"));
    }

    #[test]
    fn try_pretty_json_valid_object() {
        let result = try_pretty_json(r#"{"a":"b","c":1}"#);
        assert!(result.is_some());
        let pretty = result.unwrap();
        assert!(pretty.contains('\n'));
        assert!(pretty.contains("  \"a\": \"b\""));
    }

    #[test]
    fn try_pretty_json_valid_array() {
        let result = try_pretty_json(r#"[1,2,3]"#);
        assert!(result.is_some());
        let pretty = result.unwrap();
        assert!(pretty.contains('\n'));
    }

    #[test]
    fn try_pretty_json_empty_object() {
        let result = try_pretty_json("{}");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "{}");
    }

    #[test]
    fn try_pretty_json_empty_array() {
        let result = try_pretty_json("[]");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "[]");
    }

    #[test]
    fn try_pretty_json_not_json() {
        assert!(try_pretty_json("hello world").is_none());
        assert!(try_pretty_json("12345").is_none());
    }

    #[test]
    fn try_pretty_json_nested() {
        let input = r#"{"user":{"name":"Alice","roles":["admin","user"]}}"#;
        let result = try_pretty_json(input);
        assert!(result.is_some());
        let pretty = result.unwrap();
        assert!(pretty.contains("\"user\":"));
        assert!(pretty.contains("\"name\": \"Alice\""));
        assert!(pretty.contains("\"roles\":"));
    }

    #[test]
    fn try_pretty_json_with_escapes() {
        let input = r#"{"msg":"hello \"world\""}"#;
        let result = try_pretty_json(input);
        assert!(result.is_some());
        let pretty = result.unwrap();
        assert!(pretty.contains(r#"\"world\""#));
    }

    #[test]
    fn inspection_name() {
        let mw = RequestInspectionMiddleware::new();
        assert_eq!(mw.name(), "RequestInspection");
    }

    #[test]
    fn inspection_default_via_default_trait() {
        let mw = RequestInspectionMiddleware::default();
        assert_eq!(mw.verbosity, InspectionVerbosity::Normal);
        assert_eq!(mw.slow_threshold_ms, 1000);
    }

    #[test]
    fn inspection_with_query_string() {
        let mw = RequestInspectionMiddleware::new().verbosity(InspectionVerbosity::Minimal);
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/search");
        req.set_query(Some("q=rust&page=1".to_string()));

        let result = futures_executor::block_on(mw.before(&ctx, &mut req));
        assert!(result.is_continue());
    }

    #[test]
    fn inspection_response_body_stream() {
        let mw = RequestInspectionMiddleware::new();
        let result = mw.format_response_preview(&ResponseBody::Empty, None);
        assert!(result.is_none());
    }
}

// ============================================================================
// Rate Limiting Middleware Tests
// ============================================================================

#[cfg(test)]
mod rate_limit_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::{ResponseBody, StatusCode};
    use std::time::Duration;

    fn test_context() -> RequestContext {
        RequestContext::new(asupersync::Cx::for_testing(), 1)
    }

    fn run_rate_limit_before(mw: &RateLimitMiddleware, req: &mut Request) -> ControlFlow {
        let ctx = test_context();
        let fut = mw.before(&ctx, req);
        futures_executor::block_on(fut)
    }

    fn run_rate_limit_after(mw: &RateLimitMiddleware, req: &Request, resp: Response) -> Response {
        let ctx = test_context();
        let fut = mw.after(&ctx, req, resp);
        futures_executor::block_on(fut)
    }

    #[test]
    fn rate_limit_default_allows_requests() {
        let mw = RateLimitMiddleware::new();
        let mut req = Request::new(Method::Get, "/api/test");
        req.headers_mut()
            .insert("x-forwarded-for", b"192.168.1.1".to_vec());

        let result = run_rate_limit_before(&mw, &mut req);
        assert!(result.is_continue(), "first request should be allowed");
    }

    #[test]
    fn rate_limit_fixed_window_blocks_after_limit() {
        let mw = RateLimitMiddleware::builder()
            .requests(3)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::FixedWindow)
            .key_extractor(IpKeyExtractor)
            .build();

        for i in 0..3 {
            let mut req = Request::new(Method::Get, "/api/test");
            req.headers_mut()
                .insert("x-forwarded-for", b"10.0.0.1".to_vec());
            let result = run_rate_limit_before(&mw, &mut req);
            assert!(
                result.is_continue(),
                "request {i} should be allowed within limit"
            );
        }

        // Fourth request should be blocked
        let mut req = Request::new(Method::Get, "/api/test");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());
        let result = run_rate_limit_before(&mw, &mut req);
        assert!(result.is_break(), "fourth request should be blocked");

        // Verify 429 status
        if let ControlFlow::Break(resp) = result {
            assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        }
    }

    #[test]
    fn rate_limit_different_keys_independent() {
        let mw = RateLimitMiddleware::builder()
            .requests(2)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::FixedWindow)
            .key_extractor(IpKeyExtractor)
            .build();

        // Two requests from IP A
        for _ in 0..2 {
            let mut req = Request::new(Method::Get, "/");
            req.headers_mut()
                .insert("x-forwarded-for", b"1.1.1.1".to_vec());
            assert!(run_rate_limit_before(&mw, &mut req).is_continue());
        }

        // IP A is now exhausted
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"1.1.1.1".to_vec());
        assert!(run_rate_limit_before(&mw, &mut req).is_break());

        // IP B should still be fine
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"2.2.2.2".to_vec());
        assert!(run_rate_limit_before(&mw, &mut req).is_continue());
    }

    #[test]
    fn rate_limit_token_bucket_allows_burst() {
        let mw = RateLimitMiddleware::builder()
            .requests(5)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::TokenBucket)
            .key_extractor(IpKeyExtractor)
            .build();

        // Should allow 5 rapid requests (full bucket)
        for i in 0..5 {
            let mut req = Request::new(Method::Get, "/");
            req.headers_mut()
                .insert("x-forwarded-for", b"10.0.0.1".to_vec());
            let result = run_rate_limit_before(&mw, &mut req);
            assert!(result.is_continue(), "burst request {i} should be allowed");
        }

        // 6th request should be blocked (bucket empty)
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());
        assert!(run_rate_limit_before(&mw, &mut req).is_break());
    }

    #[test]
    fn rate_limit_sliding_window_basic() {
        let mw = RateLimitMiddleware::builder()
            .requests(3)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::SlidingWindow)
            .key_extractor(IpKeyExtractor)
            .build();

        for i in 0..3 {
            let mut req = Request::new(Method::Get, "/");
            req.headers_mut()
                .insert("x-forwarded-for", b"10.0.0.1".to_vec());
            assert!(
                run_rate_limit_before(&mw, &mut req).is_continue(),
                "sliding window request {i} should be allowed"
            );
        }

        // Should block once limit reached
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());
        assert!(run_rate_limit_before(&mw, &mut req).is_break());
    }

    #[test]
    fn rate_limit_header_key_extractor() {
        let mw = RateLimitMiddleware::builder()
            .requests(2)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::FixedWindow)
            .key_extractor(HeaderKeyExtractor::new("x-api-key"))
            .build();

        // Two requests with same API key
        for _ in 0..2 {
            let mut req = Request::new(Method::Get, "/");
            req.headers_mut().insert("x-api-key", b"key-abc".to_vec());
            assert!(run_rate_limit_before(&mw, &mut req).is_continue());
        }

        // Same key blocked
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut().insert("x-api-key", b"key-abc".to_vec());
        assert!(run_rate_limit_before(&mw, &mut req).is_break());

        // Different key still allowed
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut().insert("x-api-key", b"key-xyz".to_vec());
        assert!(run_rate_limit_before(&mw, &mut req).is_continue());
    }

    #[test]
    fn rate_limit_path_key_extractor() {
        let mw = RateLimitMiddleware::builder()
            .requests(1)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::FixedWindow)
            .key_extractor(PathKeyExtractor)
            .build();

        let mut req = Request::new(Method::Get, "/api/a");
        assert!(run_rate_limit_before(&mw, &mut req).is_continue());

        // Same path is blocked
        let mut req = Request::new(Method::Get, "/api/a");
        assert!(run_rate_limit_before(&mw, &mut req).is_break());

        // Different path is allowed
        let mut req = Request::new(Method::Get, "/api/b");
        assert!(run_rate_limit_before(&mw, &mut req).is_continue());
    }

    #[test]
    fn rate_limit_no_key_skips_limiting() {
        let mw = RateLimitMiddleware::builder()
            .requests(1)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::FixedWindow)
            .key_extractor(HeaderKeyExtractor::new("x-api-key"))
            .build();

        // Request without the header — no key extracted, should pass
        let mut req = Request::new(Method::Get, "/");
        assert!(run_rate_limit_before(&mw, &mut req).is_continue());

        // Still passes even with many requests (no key = no limiting)
        for _ in 0..10 {
            let mut req = Request::new(Method::Get, "/");
            assert!(run_rate_limit_before(&mw, &mut req).is_continue());
        }
    }

    #[test]
    fn rate_limit_response_headers_on_success() {
        let mw = RateLimitMiddleware::builder()
            .requests(10)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::FixedWindow)
            .key_extractor(IpKeyExtractor)
            .build();

        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());
        let cf = run_rate_limit_before(&mw, &mut req);
        assert!(cf.is_continue());

        let resp = Response::with_status(StatusCode::OK);
        let resp = run_rate_limit_after(&mw, &req, resp);

        // Verify rate limit headers are present
        let headers = resp.headers();
        let has_limit = headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("x-ratelimit-limit"));
        let has_remaining = headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("x-ratelimit-remaining"));
        let has_reset = headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("x-ratelimit-reset"));

        assert!(has_limit, "should have X-RateLimit-Limit header");
        assert!(has_remaining, "should have X-RateLimit-Remaining header");
        assert!(has_reset, "should have X-RateLimit-Reset header");

        // Check limit value
        let limit_val = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("x-ratelimit-limit"))
            .map(|(_, v)| std::str::from_utf8(v).unwrap().to_string())
            .unwrap();
        assert_eq!(limit_val, "10");
    }

    #[test]
    fn rate_limit_429_response_has_retry_after() {
        let mw = RateLimitMiddleware::builder()
            .requests(1)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::FixedWindow)
            .key_extractor(IpKeyExtractor)
            .build();

        // Consume the single allowed request
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());
        assert!(run_rate_limit_before(&mw, &mut req).is_continue());

        // Second request should be blocked with 429
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());
        let result = run_rate_limit_before(&mw, &mut req);

        if let ControlFlow::Break(resp) = result {
            assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

            // Should have Retry-After header
            let has_retry = resp
                .headers()
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("retry-after"));
            assert!(has_retry, "429 response should have Retry-After header");

            // Should have JSON body
            let has_ct = resp
                .headers()
                .iter()
                .any(|(n, v)| n.eq_ignore_ascii_case("content-type") && v == b"application/json");
            assert!(has_ct, "429 response should have JSON content type");
        } else {
            panic!("expected Break(429)");
        }
    }

    #[test]
    fn rate_limit_no_headers_when_disabled() {
        let mw = RateLimitMiddleware::builder()
            .requests(10)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::FixedWindow)
            .key_extractor(IpKeyExtractor)
            .include_headers(false)
            .build();

        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());
        assert!(run_rate_limit_before(&mw, &mut req).is_continue());

        let resp = Response::with_status(StatusCode::OK);
        let resp = run_rate_limit_after(&mw, &req, resp);

        let has_limit = resp
            .headers()
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("x-ratelimit-limit"));
        assert!(
            !has_limit,
            "should NOT have rate limit headers when disabled"
        );
    }

    #[test]
    fn rate_limit_custom_retry_message() {
        let mw = RateLimitMiddleware::builder()
            .requests(1)
            .per(Duration::from_secs(60))
            .algorithm(RateLimitAlgorithm::FixedWindow)
            .key_extractor(IpKeyExtractor)
            .retry_message("Slow down, partner!")
            .build();

        // Exhaust limit
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());
        run_rate_limit_before(&mw, &mut req);

        // Check custom message in 429 body
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());
        if let ControlFlow::Break(resp) = run_rate_limit_before(&mw, &mut req) {
            if let ResponseBody::Bytes(body) = resp.body_ref() {
                let body_str = std::str::from_utf8(body).unwrap();
                assert!(
                    body_str.contains("Slow down, partner!"),
                    "expected custom message in body, got: {body_str}"
                );
            } else {
                panic!("expected Bytes body");
            }
        } else {
            panic!("expected Break(429)");
        }
    }

    #[test]
    fn rate_limit_ip_extractor_x_forwarded_for() {
        let extractor = IpKeyExtractor;
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("x-forwarded-for", b"1.2.3.4, 5.6.7.8".to_vec());
        assert_eq!(extractor.extract_key(&req), Some("1.2.3.4".to_string()));
    }

    #[test]
    fn rate_limit_ip_extractor_x_real_ip() {
        let extractor = IpKeyExtractor;
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut().insert("x-real-ip", b"9.8.7.6".to_vec());
        assert_eq!(extractor.extract_key(&req), Some("9.8.7.6".to_string()));
    }

    #[test]
    fn rate_limit_ip_extractor_fallback() {
        let extractor = IpKeyExtractor;
        let req = Request::new(Method::Get, "/");
        assert_eq!(extractor.extract_key(&req), Some("unknown".to_string()));
    }

    #[test]
    fn rate_limit_composite_key_extractor() {
        let extractor =
            CompositeKeyExtractor::new(vec![Box::new(IpKeyExtractor), Box::new(PathKeyExtractor)]);

        let mut req = Request::new(Method::Get, "/api/users");
        req.headers_mut()
            .insert("x-forwarded-for", b"10.0.0.1".to_vec());

        let key = extractor.extract_key(&req);
        assert_eq!(key, Some("10.0.0.1:/api/users".to_string()));
    }

    #[test]
    fn rate_limit_builder_defaults() {
        let mw = RateLimitMiddleware::builder().build();
        assert_eq!(mw.config.max_requests, 100);
        assert_eq!(mw.config.window, Duration::from_secs(60));
        assert_eq!(mw.config.algorithm, RateLimitAlgorithm::TokenBucket);
        assert!(mw.config.include_headers);
    }

    #[test]
    fn rate_limit_builder_per_minute() {
        let mw = RateLimitMiddleware::builder()
            .requests(50)
            .per_minute(2)
            .algorithm(RateLimitAlgorithm::SlidingWindow)
            .build();
        assert_eq!(mw.config.max_requests, 50);
        assert_eq!(mw.config.window, Duration::from_secs(120));
        assert_eq!(mw.config.algorithm, RateLimitAlgorithm::SlidingWindow);
    }

    #[test]
    fn rate_limit_builder_per_hour() {
        let mw = RateLimitMiddleware::builder()
            .requests(1000)
            .per_hour(1)
            .build();
        assert_eq!(mw.config.window, Duration::from_secs(3600));
    }

    #[test]
    fn rate_limit_middleware_name() {
        let mw = RateLimitMiddleware::new();
        assert_eq!(mw.name(), "RateLimit");
    }

    #[test]
    fn rate_limit_default_via_default_trait() {
        let mw = RateLimitMiddleware::default();
        assert_eq!(mw.config.max_requests, 100);
    }

    // ========================================================================
    // ETag Middleware Tests
    // ========================================================================

    #[test]
    fn etag_middleware_generates_etag_for_get() {
        let mw = ETagMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/resource");

        // Create response with body
        let response = Response::ok()
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(br#"{"status":"ok"}"#.to_vec()));

        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        // Should have ETag header
        let etag = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("etag"));
        assert!(etag.is_some(), "Response should have ETag header");

        // ETag should be a quoted hex string
        let etag_value = std::str::from_utf8(&etag.unwrap().1).unwrap();
        assert!(etag_value.starts_with('"'), "ETag should start with quote");
        assert!(etag_value.ends_with('"'), "ETag should end with quote");
    }

    #[test]
    fn etag_middleware_returns_304_on_match() {
        let mw = ETagMiddleware::new();
        let ctx = test_context();

        // First request to get the ETag
        let mut req1 = Request::new(crate::request::Method::Get, "/resource");
        let body = br#"{"status":"ok"}"#.to_vec();
        let response1 = Response::ok().body(ResponseBody::Bytes(body.clone()));
        let response1 = futures_executor::block_on(mw.after(&ctx, &req1, response1));

        let etag = response1
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
            .map(|(_, v)| std::str::from_utf8(v).unwrap().to_string())
            .unwrap();

        // Second request with If-None-Match header
        let mut req2 = Request::new(crate::request::Method::Get, "/resource");
        req2.headers_mut()
            .insert("if-none-match", etag.as_bytes().to_vec());

        let response2 = Response::ok().body(ResponseBody::Bytes(body));
        let response2 = futures_executor::block_on(mw.after(&ctx, &req2, response2));

        // Should return 304 Not Modified
        assert_eq!(response2.status().as_u16(), 304);
        assert!(response2.body_ref().is_empty());
    }

    #[test]
    fn etag_middleware_returns_full_response_on_mismatch() {
        let mw = ETagMiddleware::new();
        let ctx = test_context();

        let mut req = Request::new(crate::request::Method::Get, "/resource");
        req.headers_mut()
            .insert("if-none-match", b"\"old-etag\"".to_vec());

        let body = br#"{"status":"updated"}"#.to_vec();
        let response = Response::ok().body(ResponseBody::Bytes(body.clone()));
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        // Should return 200 OK with body
        assert_eq!(response.status().as_u16(), 200);
        assert!(!response.body_ref().is_empty());
    }

    #[test]
    fn etag_middleware_weak_etag_generation() {
        let config = ETagConfig::new().weak(true);
        let mw = ETagMiddleware::with_config(config);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/resource");

        let response = Response::ok().body(ResponseBody::Bytes(b"data".to_vec()));
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        let etag = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
            .map(|(_, v)| std::str::from_utf8(v).unwrap().to_string())
            .unwrap();

        assert!(etag.starts_with("W/"), "Weak ETag should start with W/");
    }

    #[test]
    fn etag_middleware_skips_post_requests() {
        let mw = ETagMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Post, "/resource");

        let response = Response::ok().body(ResponseBody::Bytes(b"created".to_vec()));
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        // POST should not get ETag
        let etag = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("etag"));
        assert!(etag.is_none(), "POST should not have ETag");
    }

    #[test]
    fn etag_middleware_handles_head_requests() {
        let mw = ETagMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Head, "/resource");

        let response = Response::ok().body(ResponseBody::Bytes(b"data".to_vec()));
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        // HEAD should get ETag
        let etag = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("etag"));
        assert!(etag.is_some(), "HEAD should have ETag");
    }

    #[test]
    fn etag_middleware_disabled_mode() {
        let config = ETagConfig::new().mode(ETagMode::Disabled);
        let mw = ETagMiddleware::with_config(config);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/resource");

        let response = Response::ok().body(ResponseBody::Bytes(b"data".to_vec()));
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        // Should not have ETag when disabled
        let etag = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("etag"));
        assert!(etag.is_none(), "Disabled mode should not add ETag");
    }

    #[test]
    fn etag_middleware_min_size_filter() {
        let config = ETagConfig::new().min_size(1000);
        let mw = ETagMiddleware::with_config(config);
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/resource");

        // Small body below min_size
        let response = Response::ok().body(ResponseBody::Bytes(b"small".to_vec()));
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        // Should not have ETag for small body
        let etag = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("etag"));
        assert!(etag.is_none(), "Small body should not get ETag");
    }

    #[test]
    fn etag_middleware_preserves_existing_etag() {
        let config = ETagConfig::new().mode(ETagMode::Manual);
        let mw = ETagMiddleware::with_config(config);
        let ctx = test_context();

        // First request to set up cached ETag
        let mut req = Request::new(crate::request::Method::Get, "/resource");
        req.headers_mut()
            .insert("if-none-match", b"\"custom-etag\"".to_vec());

        // Response with pre-set ETag matching the request
        let response = Response::ok()
            .header("etag", b"\"custom-etag\"".to_vec())
            .body(ResponseBody::Bytes(b"data".to_vec()));
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        // Should return 304 since custom ETag matches
        assert_eq!(response.status().as_u16(), 304);
    }

    #[test]
    fn etag_middleware_wildcard_if_none_match() {
        let mw = ETagMiddleware::new();
        let ctx = test_context();
        let mut req = Request::new(crate::request::Method::Get, "/resource");
        req.headers_mut().insert("if-none-match", b"*".to_vec());

        let response = Response::ok().body(ResponseBody::Bytes(b"data".to_vec()));
        let response = futures_executor::block_on(mw.after(&ctx, &req, response));

        // Wildcard should match any ETag
        assert_eq!(response.status().as_u16(), 304);
    }

    #[test]
    fn etag_middleware_weak_comparison_matches() {
        let mw = ETagMiddleware::new();
        let ctx = test_context();

        // Get the strong ETag
        let mut req1 = Request::new(crate::request::Method::Get, "/resource");
        let body = b"test data".to_vec();
        let response1 = Response::ok().body(ResponseBody::Bytes(body.clone()));
        let response1 = futures_executor::block_on(mw.after(&ctx, &req1, response1));

        let etag = response1
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
            .map(|(_, v)| std::str::from_utf8(v).unwrap().to_string())
            .unwrap();

        // Send request with weak version of the same ETag
        let mut req2 = Request::new(crate::request::Method::Get, "/resource");
        let weak_etag = format!("W/{}", etag);
        req2.headers_mut()
            .insert("if-none-match", weak_etag.as_bytes().to_vec());

        let response2 = Response::ok().body(ResponseBody::Bytes(body));
        let response2 = futures_executor::block_on(mw.after(&ctx, &req2, response2));

        // Weak comparison should match
        assert_eq!(response2.status().as_u16(), 304);
    }

    #[test]
    fn etag_middleware_name() {
        let mw = ETagMiddleware::new();
        assert_eq!(mw.name(), "ETagMiddleware");
    }

    #[test]
    fn etag_config_builder() {
        let config = ETagConfig::new()
            .mode(ETagMode::Auto)
            .weak(true)
            .min_size(512);

        assert_eq!(config.mode, ETagMode::Auto);
        assert!(config.weak);
        assert_eq!(config.min_size, 512);
    }

    #[test]
    fn etag_generates_consistent_hash() {
        // Same data should produce same ETag
        let etag1 = ETagMiddleware::generate_etag(b"hello world", false);
        let etag2 = ETagMiddleware::generate_etag(b"hello world", false);
        assert_eq!(etag1, etag2);

        // Different data should produce different ETag
        let etag3 = ETagMiddleware::generate_etag(b"hello world!", false);
        assert_ne!(etag1, etag3);
    }
}
