//! Request context with asupersync integration.
//!
//! [`RequestContext`] wraps asupersync's [`Cx`] to provide request-scoped
//! capabilities for HTTP request handling.

use asupersync::types::CancelReason;
use asupersync::{Budget, Cx, Outcome, RegionId, TaskId};
use std::sync::Arc;

use crate::dependency::{CleanupStack, DependencyCache, DependencyOverrides, ResolutionStack};

/// Default maximum body size: 1MB.
pub const DEFAULT_MAX_BODY_SIZE: usize = 1024 * 1024;

/// Configuration for request body limits.
///
/// This struct holds the body size limit configuration that applies to a request.
/// It can be configured at the application level (via `AppConfig`) and optionally
/// overridden on a per-route basis.
#[derive(Debug, Clone, Copy)]
pub struct BodyLimitConfig {
    /// Maximum body size in bytes.
    max_size: usize,
}

impl Default for BodyLimitConfig {
    fn default() -> Self {
        Self {
            max_size: DEFAULT_MAX_BODY_SIZE,
        }
    }
}

impl BodyLimitConfig {
    /// Creates a new body limit config with the specified maximum size.
    #[must_use]
    pub fn new(max_size: usize) -> Self {
        Self { max_size }
    }

    /// Returns the maximum body size in bytes.
    #[must_use]
    pub fn max_size(&self) -> usize {
        self.max_size
    }
}

/// Request context that wraps asupersync's capability context.
///
/// `RequestContext` provides access to:
/// - Request-scoped identity (request ID, trace context)
/// - Cancellation checkpoints for cancel-safe handlers
/// - Budget/deadline awareness for timeout enforcement
/// - Region-scoped spawning for background work
/// - Body size limit configuration for DoS prevention
///
/// # Example
///
/// ```ignore
/// async fn handler(ctx: &RequestContext) -> impl IntoResponse {
///     // Check for client disconnect
///     ctx.checkpoint()?;
///
///     // Get remaining time budget
///     let remaining = ctx.remaining_budget();
///
///     // Check body size limit
///     let max_body = ctx.body_limit().max_size();
///
///     // Do work...
///     "Hello, World!"
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// The underlying capability context.
    cx: Cx,
    /// Unique request identifier for tracing.
    request_id: u64,
    /// Request-scoped dependency cache.
    dependency_cache: Arc<DependencyCache>,
    /// Dependency overrides (primarily for testing).
    dependency_overrides: Arc<DependencyOverrides>,
    /// Stack tracking dependencies currently being resolved (for cycle detection).
    resolution_stack: Arc<ResolutionStack>,
    /// Cleanup functions to run after handler completion (LIFO order).
    cleanup_stack: Arc<CleanupStack>,
    /// Body size limit configuration for this request.
    body_limit: BodyLimitConfig,
}

impl RequestContext {
    /// Creates a new request context from an asupersync Cx.
    ///
    /// This is typically called by the server when accepting a new request,
    /// creating a new region for the request lifecycle. Uses the default
    /// body size limit (1MB).
    #[must_use]
    pub fn new(cx: Cx, request_id: u64) -> Self {
        Self {
            cx,
            request_id,
            dependency_cache: Arc::new(DependencyCache::new()),
            dependency_overrides: Arc::new(DependencyOverrides::new()),
            resolution_stack: Arc::new(ResolutionStack::new()),
            cleanup_stack: Arc::new(CleanupStack::new()),
            body_limit: BodyLimitConfig::default(),
        }
    }

    /// Creates a new request context with a custom body size limit.
    ///
    /// Use this when the application has configured a specific `max_body_size`
    /// in `AppConfig`, or when a route has an override.
    #[must_use]
    pub fn with_body_limit(cx: Cx, request_id: u64, max_body_size: usize) -> Self {
        Self {
            cx,
            request_id,
            dependency_cache: Arc::new(DependencyCache::new()),
            dependency_overrides: Arc::new(DependencyOverrides::new()),
            resolution_stack: Arc::new(ResolutionStack::new()),
            cleanup_stack: Arc::new(CleanupStack::new()),
            body_limit: BodyLimitConfig::new(max_body_size),
        }
    }

    /// Creates a new request context with shared dependency overrides.
    #[must_use]
    pub fn with_overrides(cx: Cx, request_id: u64, overrides: Arc<DependencyOverrides>) -> Self {
        Self {
            cx,
            request_id,
            dependency_cache: Arc::new(DependencyCache::new()),
            dependency_overrides: overrides,
            resolution_stack: Arc::new(ResolutionStack::new()),
            cleanup_stack: Arc::new(CleanupStack::new()),
            body_limit: BodyLimitConfig::default(),
        }
    }

    /// Creates a new request context with overrides and a custom body size limit.
    #[must_use]
    pub fn with_overrides_and_body_limit(
        cx: Cx,
        request_id: u64,
        overrides: Arc<DependencyOverrides>,
        max_body_size: usize,
    ) -> Self {
        Self {
            cx,
            request_id,
            dependency_cache: Arc::new(DependencyCache::new()),
            dependency_overrides: overrides,
            resolution_stack: Arc::new(ResolutionStack::new()),
            cleanup_stack: Arc::new(CleanupStack::new()),
            body_limit: BodyLimitConfig::new(max_body_size),
        }
    }

    /// Returns the unique request identifier.
    ///
    /// Useful for logging and tracing across the request lifecycle.
    #[must_use]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    /// Returns the dependency cache for this request.
    #[must_use]
    pub fn dependency_cache(&self) -> &DependencyCache {
        &self.dependency_cache
    }

    /// Returns the dependency overrides registry.
    #[must_use]
    pub fn dependency_overrides(&self) -> &DependencyOverrides {
        &self.dependency_overrides
    }

    /// Returns the resolution stack for cycle detection.
    #[must_use]
    pub fn resolution_stack(&self) -> &ResolutionStack {
        &self.resolution_stack
    }

    /// Returns the cleanup stack for registering cleanup functions.
    ///
    /// Cleanup functions run after the handler completes in LIFO order.
    #[must_use]
    pub fn cleanup_stack(&self) -> &CleanupStack {
        &self.cleanup_stack
    }

    /// Returns the body limit configuration for this request.
    ///
    /// This can be used by body extractors (e.g., `Json<T>`) to enforce
    /// size limits and prevent DoS attacks.
    #[must_use]
    pub fn body_limit(&self) -> &BodyLimitConfig {
        &self.body_limit
    }

    /// Returns the maximum body size in bytes for this request.
    ///
    /// This is a convenience method equivalent to `ctx.body_limit().max_size()`.
    #[must_use]
    pub fn max_body_size(&self) -> usize {
        self.body_limit.max_size()
    }

    /// Returns the underlying region ID from asupersync.
    ///
    /// The region represents the request's lifecycle scope - all spawned
    /// tasks belong to this region and will be cleaned up when the
    /// request completes or is cancelled.
    #[must_use]
    pub fn region_id(&self) -> RegionId {
        self.cx.region_id()
    }

    /// Returns the current task ID.
    #[must_use]
    pub fn task_id(&self) -> TaskId {
        self.cx.task_id()
    }

    /// Returns the current budget.
    ///
    /// The budget represents the remaining computational resources (time, polls)
    /// available for this request. When exhausted, the request should be
    /// cancelled gracefully.
    #[must_use]
    pub fn budget(&self) -> Budget {
        self.cx.budget()
    }

    /// Checks if cancellation has been requested.
    ///
    /// This includes client disconnection, timeout, or explicit cancellation.
    /// Handlers should check this periodically and exit early if true.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cx.is_cancel_requested()
    }

    /// Cooperative cancellation checkpoint.
    ///
    /// Call this at natural suspension points in your handler to allow
    /// graceful cancellation. Returns `Err` if cancellation is pending.
    ///
    /// # Errors
    ///
    /// Returns an error if the request has been cancelled and cancellation
    /// is not currently masked.
    ///
    /// # Example
    ///
    /// ```ignore
    /// async fn process_items(ctx: &RequestContext, items: Vec<Item>) -> Result<(), HttpError> {
    ///     for item in items {
    ///         ctx.checkpoint()?;  // Allow cancellation between items
    ///         process_item(item).await?;
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn checkpoint(&self) -> Result<(), CancelledError> {
        self.cx.checkpoint().map_err(|_| CancelledError)
    }

    /// Executes a closure with cancellation masked.
    ///
    /// While masked, `checkpoint()` will not return an error even if
    /// cancellation is pending. Use this for critical sections that
    /// must complete atomically.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Commit transaction - must not be interrupted
    /// ctx.masked(|| {
    ///     db.commit().await?;
    ///     Ok(())
    /// })
    /// ```
    pub fn masked<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.cx.masked(f)
    }

    /// Records a trace event for this request.
    ///
    /// Events are associated with the request's trace context and can be
    /// used for debugging and observability.
    pub fn trace(&self, message: &str) {
        self.cx.trace(message);
    }

    /// Returns a reference to the underlying asupersync Cx.
    ///
    /// Use this when you need direct access to asupersync primitives,
    /// such as spawning tasks or using combinators.
    #[must_use]
    pub fn cx(&self) -> &Cx {
        &self.cx
    }
}

/// Error returned when a request has been cancelled.
///
/// This is returned by `checkpoint()` when the request should stop
/// processing. The server will convert this to an appropriate HTTP
/// response (typically 499 Client Closed Request or 504 Gateway Timeout).
#[derive(Debug, Clone, Copy)]
pub struct CancelledError;

impl std::fmt::Display for CancelledError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "request cancelled")
    }
}

impl std::error::Error for CancelledError {}

/// Extension trait for converting HTTP results to asupersync Outcome.
///
/// This bridges the HTTP error model with asupersync's 4-valued outcome
/// (Ok, Err, Cancelled, Panicked).
pub trait IntoOutcome<T, E> {
    /// Converts this result into an asupersync Outcome.
    fn into_outcome(self) -> Outcome<T, E>;
}

impl<T, E> IntoOutcome<T, E> for Result<T, E> {
    fn into_outcome(self) -> Outcome<T, E> {
        match self {
            Ok(v) => Outcome::Ok(v),
            Err(e) => Outcome::Err(e),
        }
    }
}

impl<T, E> IntoOutcome<T, E> for Result<T, CancelledError>
where
    E: Default,
{
    fn into_outcome(self) -> Outcome<T, E> {
        match self {
            Ok(v) => Outcome::Ok(v),
            Err(CancelledError) => Outcome::Cancelled(CancelReason::user("request cancelled")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_error_display() {
        let err = CancelledError;
        assert_eq!(format!("{err}"), "request cancelled");
    }

    #[test]
    fn checkpoint_returns_error_when_cancel_requested() {
        let cx = Cx::for_testing();
        let ctx = RequestContext::new(cx, 1);
        ctx.cx().set_cancel_requested(true);
        assert!(ctx.checkpoint().is_err());
    }

    #[test]
    fn masked_defers_cancellation_at_checkpoint() {
        let cx = Cx::for_testing();
        let ctx = RequestContext::new(cx, 1);
        ctx.cx().set_cancel_requested(true);

        let result = ctx.masked(|| ctx.checkpoint());
        assert!(result.is_ok());
        assert!(ctx.checkpoint().is_err());
    }

    // ========================================================================
    // Body Limit Tests
    // ========================================================================

    #[test]
    fn body_limit_config_default() {
        let config = BodyLimitConfig::default();
        assert_eq!(config.max_size(), DEFAULT_MAX_BODY_SIZE);
        assert_eq!(config.max_size(), 1024 * 1024); // 1MB
    }

    #[test]
    fn body_limit_config_custom() {
        let config = BodyLimitConfig::new(512 * 1024);
        assert_eq!(config.max_size(), 512 * 1024); // 512KB
    }

    #[test]
    fn request_context_default_body_limit() {
        let cx = Cx::for_testing();
        let ctx = RequestContext::new(cx, 1);
        assert_eq!(ctx.max_body_size(), DEFAULT_MAX_BODY_SIZE);
        assert_eq!(ctx.body_limit().max_size(), DEFAULT_MAX_BODY_SIZE);
    }

    #[test]
    fn request_context_custom_body_limit() {
        let cx = Cx::for_testing();
        let ctx = RequestContext::with_body_limit(cx, 1, 2 * 1024 * 1024);
        assert_eq!(ctx.max_body_size(), 2 * 1024 * 1024); // 2MB
    }

    #[test]
    fn request_context_with_overrides_has_default_limit() {
        let cx = Cx::for_testing();
        let overrides = Arc::new(DependencyOverrides::new());
        let ctx = RequestContext::with_overrides(cx, 1, overrides);
        assert_eq!(ctx.max_body_size(), DEFAULT_MAX_BODY_SIZE);
    }

    #[test]
    fn request_context_with_overrides_and_custom_limit() {
        let cx = Cx::for_testing();
        let overrides = Arc::new(DependencyOverrides::new());
        let ctx = RequestContext::with_overrides_and_body_limit(cx, 1, overrides, 4 * 1024 * 1024);
        assert_eq!(ctx.max_body_size(), 4 * 1024 * 1024); // 4MB
    }
}
