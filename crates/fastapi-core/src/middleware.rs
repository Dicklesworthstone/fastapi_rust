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
            match self.middleware.before(ctx, req).await {
                ControlFlow::Continue => {
                    // Call inner handler
                    let response = self.inner.call(ctx, req).await;
                    // Run after hook
                    self.middleware.after(ctx, req, response).await
                }
                ControlFlow::Break(response) => {
                    // Short-circuit: still run after for this middleware
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
}
