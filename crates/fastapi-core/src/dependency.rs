//! Dependency injection support.
//!
//! This module provides the `Depends` extractor and supporting types for
//! request-scoped dependency resolution with optional caching and overrides.
//!
//! # Circular Dependency Detection
//!
//! The DI system detects circular dependencies at runtime. If a dependency
//! graph contains a cycle (e.g., A -> B -> C -> A), the resolution will fail
//! with a [`CircularDependencyError`] containing the full cycle path.
//!
//! ```text
//! Circular dependency detected: DbPool -> UserService -> AuthService -> DbPool
//! ```

use crate::context::RequestContext;
use crate::extract::FromRequest;
use crate::request::Request;
use crate::response::{IntoResponse, Response, ResponseBody, StatusCode};
use parking_lot::Mutex;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use parking_lot::RwLock;

/// Dependency resolution scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyScope {
    /// No request-level caching; resolve on each call.
    Function,
    /// Cache for the lifetime of the request.
    Request,
}

// ============================================================================
// Cleanup Stack (for Generator-style Dependencies)
// ============================================================================

/// Type alias for cleanup functions.
///
/// A cleanup function is an async closure that performs teardown work after
/// the request handler completes. Cleanup functions run in LIFO (last-in,
/// first-out) order, similar to Python's `contextlib.ExitStack`.
pub type CleanupFn = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// Stack of cleanup functions to run after handler completion.
///
/// `CleanupStack` provides generator-style dependency lifecycle management
/// similar to FastAPI's `yield` dependencies. Cleanup functions are registered
/// during dependency resolution and run in LIFO order after the handler
/// completes, even on error or panic.
///
/// # Example
///
/// ```ignore
/// // Dependency with cleanup
/// impl FromDependencyWithCleanup for DbConnection {
///     type Value = DbConnection;
///     type Error = HttpError;
///
///     async fn setup(ctx: &RequestContext, req: &mut Request)
///         -> Result<(Self::Value, Option<CleanupFn>), Self::Error>
///     {
///         let conn = DbPool::get_connection().await?;
///         let cleanup = {
///             let conn = conn.clone();
///             Box::new(move || {
///                 Box::pin(async move {
///                     conn.release().await;
///                 }) as Pin<Box<dyn Future<Output = ()> + Send>>
///             }) as CleanupFn
///         };
///         Ok((conn, Some(cleanup)))
///     }
/// }
/// ```
///
/// # Cleanup Order
///
/// Cleanup functions run in LIFO order (last registered runs first):
///
/// ```text
/// Setup order:   A -> B -> C
/// Cleanup order: C -> B -> A
/// ```
///
/// This ensures that dependencies are cleaned up in the reverse order
/// of their setup, maintaining proper resource lifecycle semantics.
pub struct CleanupStack {
    /// Cleanup functions in registration order (will be reversed when running).
    cleanups: Mutex<Vec<CleanupFn>>,
}

impl CleanupStack {
    /// Create an empty cleanup stack.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cleanups: Mutex::new(Vec::new()),
        }
    }

    /// Register a cleanup function to run after handler completion.
    ///
    /// Cleanup functions are run in LIFO order (last registered runs first).
    pub fn push(&self, cleanup: CleanupFn) {
        let mut guard = self.cleanups.lock();
        guard.push(cleanup);
    }

    /// Take all cleanup functions for execution.
    ///
    /// Returns cleanups in LIFO order (reversed from registration order).
    /// After calling this, the stack is empty.
    pub fn take_cleanups(&self) -> Vec<CleanupFn> {
        let mut guard = self.cleanups.lock();
        let mut cleanups = std::mem::take(&mut *guard);
        cleanups.reverse(); // LIFO order
        cleanups
    }

    /// Returns the number of registered cleanup functions.
    #[must_use]
    pub fn len(&self) -> usize {
        let guard = self.cleanups.lock();
        guard.len()
    }

    /// Returns true if no cleanup functions are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Run all cleanup functions in LIFO order.
    ///
    /// This consumes all registered cleanup functions. Each cleanup is
    /// awaited in sequence. If a cleanup function panics, the remaining
    /// cleanups are still attempted.
    ///
    /// # Returns
    ///
    /// The number of cleanup functions that completed successfully.
    pub async fn run_cleanups(&self) -> usize {
        let cleanups = self.take_cleanups();
        let mut completed = 0;

        for cleanup in cleanups {
            // Call the cleanup function to get the future, catching panics
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (cleanup)()));
            match result {
                Ok(future) => {
                    // Now await the returned future
                    future.await;
                    completed += 1;
                }
                Err(_) => {
                    // Cleanup panicked during creation, continue with remaining cleanups
                }
            }
        }

        completed
    }
}

impl Default for CleanupStack {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CleanupStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CleanupStack")
            .field("count", &self.len())
            .finish()
    }
}

/// Trait for dependencies that require cleanup after handler completion.
///
/// This is similar to FastAPI's `yield` dependencies. Dependencies implementing
/// this trait can perform setup logic and register cleanup functions that
/// run after the request handler completes.
///
/// # Example
///
/// ```ignore
/// struct DbTransaction {
///     tx: Transaction,
/// }
///
/// impl FromDependencyWithCleanup for DbTransaction {
///     type Value = DbTransaction;
///     type Error = HttpError;
///
///     async fn setup(ctx: &RequestContext, req: &mut Request)
///         -> Result<(Self::Value, Option<CleanupFn>), Self::Error>
///     {
///         let pool = Depends::<DbPool>::from_request(ctx, req).await?;
///         let tx = pool.begin().await.map_err(|_| HttpError::internal())?;
///
///         let cleanup_tx = tx.clone();
///         let cleanup = Box::new(move || {
///             Box::pin(async move {
///                 // Commit or rollback happens here
///                 cleanup_tx.commit().await.ok();
///             }) as Pin<Box<dyn Future<Output = ()> + Send>>
///         }) as CleanupFn;
///
///         Ok((DbTransaction { tx }, Some(cleanup)))
///     }
/// }
/// ```
pub trait FromDependencyWithCleanup: Clone + Send + Sync + 'static {
    /// The value type produced by setup.
    type Value: Clone + Send + Sync + 'static;
    /// Error type when setup fails.
    type Error: IntoResponse + Send + Sync + 'static;

    /// Set up the dependency and optionally return a cleanup function.
    ///
    /// The cleanup function (if provided) will run after the request handler
    /// completes, even on error or panic.
    fn setup(
        ctx: &RequestContext,
        req: &mut Request,
    ) -> impl Future<Output = Result<(Self::Value, Option<CleanupFn>), Self::Error>> + Send;
}

/// Wrapper for dependencies that have cleanup callbacks.
///
/// `DependsCleanup` works like `Depends` but for types implementing
/// `FromDependencyWithCleanup`. It automatically registers cleanup
/// functions with the request's cleanup stack.
#[derive(Debug, Clone)]
pub struct DependsCleanup<T, C = DefaultDependencyConfig>(pub T, PhantomData<C>);

impl<T, C> DependsCleanup<T, C> {
    /// Create a new `DependsCleanup` wrapper.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self(value, PhantomData)
    }

    /// Unwrap the inner value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, C> Deref for DependsCleanup<T, C> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, C> DerefMut for DependsCleanup<T, C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T, C> FromRequest for DependsCleanup<T, C>
where
    T: FromDependencyWithCleanup<Value = T>,
    C: DependsConfig,
{
    type Error = T::Error;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Check cancellation before resolving dependency
        let _ = ctx.checkpoint();

        let scope = C::SCOPE.unwrap_or(DependencyScope::Request);
        let use_cache = C::USE_CACHE && scope == DependencyScope::Request;

        // Check cache first
        if use_cache {
            if let Some(cached) = ctx.dependency_cache().get::<T::Value>() {
                return Ok(DependsCleanup::new(cached));
            }
        }

        // Check for circular dependency
        let type_name = std::any::type_name::<T>();
        if let Some(cycle) = ctx.resolution_stack().check_cycle::<T>(type_name) {
            let err = CircularDependencyError::new(cycle);
            panic!("{}", err);
        }

        // Check for scope violation: request-scoped cannot depend on function-scoped
        if let Some(scope_err) = ctx
            .resolution_stack()
            .check_scope_violation(type_name, scope)
        {
            panic!("{}", scope_err);
        }

        // Push onto resolution stack
        ctx.resolution_stack().push::<T>(type_name, scope);
        let _guard = ResolutionGuard::new(ctx.resolution_stack());

        // Setup the dependency
        let _ = ctx.checkpoint();
        let (value, cleanup) = T::setup(ctx, req).await?;

        // Register cleanup if provided
        if let Some(cleanup_fn) = cleanup {
            ctx.cleanup_stack().push(cleanup_fn);
        }

        // Cache if needed
        if use_cache {
            ctx.dependency_cache().insert::<T::Value>(value.clone());
        }

        Ok(DependsCleanup::new(value))
    }
}

/// Configuration for `Depends` resolution.
pub trait DependsConfig {
    /// Whether to use caching.
    const USE_CACHE: bool;
    /// Optional scope override.
    const SCOPE: Option<DependencyScope>;
}

/// Default dependency configuration (cache per request).
#[derive(Debug, Clone, Copy)]
pub struct DefaultDependencyConfig;

/// Backwards-friendly alias for the default config.
pub type DefaultConfig = DefaultDependencyConfig;

impl DependsConfig for DefaultDependencyConfig {
    const USE_CACHE: bool = true;
    const SCOPE: Option<DependencyScope> = None;
}

/// Disable caching for this dependency.
#[derive(Debug, Clone, Copy)]
pub struct NoCache;

impl DependsConfig for NoCache {
    const USE_CACHE: bool = false;
    const SCOPE: Option<DependencyScope> = Some(DependencyScope::Function);
}

// ============================================================================
// Circular Dependency Detection
// ============================================================================

/// Error returned when a circular dependency is detected.
///
/// This error contains the full cycle path showing which types form the cycle.
/// For example, if `DbPool` depends on `UserService` which depends on `DbPool`,
/// the cycle would be: `["DbPool", "UserService", "DbPool"]`.
///
/// # Example
///
/// ```text
/// Circular dependency detected: DbPool -> UserService -> DbPool
/// ```
#[derive(Debug, Clone)]
pub struct CircularDependencyError {
    /// The names of the types forming the cycle, in resolution order.
    /// The first and last element are the same type (completing the cycle).
    pub cycle: Vec<String>,
}

/// Error returned when a dependency scope constraint is violated.
///
/// This occurs when a request-scoped dependency depends on a function-scoped
/// dependency. Since request-scoped dependencies are cached for the lifetime
/// of the request, they would hold stale values from function-scoped
/// dependencies (which should be resolved fresh on each call).
///
/// # Example
///
/// ```text
/// Dependency scope violation: request-scoped 'CachedUser' depends on
/// function-scoped 'DbConnection'. Request-scoped dependencies cannot
/// depend on function-scoped dependencies because the cached value
/// would become stale.
/// ```
///
/// # Why This Matters
///
/// Consider this scenario:
/// - `CachedUser` is request-scoped (cached for the request)
/// - `DbConnection` is function-scoped (fresh connection each call)
///
/// If `CachedUser` depends on `DbConnection`:
/// 1. First request: `CachedUser` resolved, caches `DbConnection` value
/// 2. Later in same request: `CachedUser` retrieved from cache
/// 3. Problem: The cached `CachedUser` holds a stale `DbConnection`
///
/// This violates the contract that function-scoped dependencies are fresh.
#[derive(Debug, Clone)]
pub struct DependencyScopeError {
    /// The name of the request-scoped dependency (the outer one).
    pub request_scoped_type: String,
    /// The name of the function-scoped dependency (the inner one).
    pub function_scoped_type: String,
}

impl CircularDependencyError {
    /// Create a new circular dependency error with the given cycle path.
    #[must_use]
    pub fn new(cycle: Vec<String>) -> Self {
        Self { cycle }
    }

    /// Get a human-readable representation of the cycle.
    #[must_use]
    pub fn cycle_path(&self) -> String {
        self.cycle.join(" -> ")
    }
}

impl std::fmt::Display for CircularDependencyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Circular dependency detected: {}", self.cycle_path())
    }
}

impl std::error::Error for CircularDependencyError {}

impl IntoResponse for CircularDependencyError {
    fn into_response(self) -> Response {
        let body = format!(
            r#"{{"detail":"Circular dependency detected: {}"}}"#,
            self.cycle_path()
        );
        Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.into_bytes()))
    }
}

impl DependencyScopeError {
    /// Create a new scope violation error.
    #[must_use]
    pub fn new(request_scoped_type: String, function_scoped_type: String) -> Self {
        Self {
            request_scoped_type,
            function_scoped_type,
        }
    }
}

impl std::fmt::Display for DependencyScopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Dependency scope violation: request-scoped '{}' depends on function-scoped '{}'. \
             Request-scoped dependencies cannot depend on function-scoped dependencies \
             because the cached value would become stale.",
            self.request_scoped_type, self.function_scoped_type
        )
    }
}

impl std::error::Error for DependencyScopeError {}

impl IntoResponse for DependencyScopeError {
    fn into_response(self) -> Response {
        let body = format!(
            r#"{{"detail":"Dependency scope violation: request-scoped '{}' depends on function-scoped '{}'. Request-scoped dependencies cannot depend on function-scoped dependencies."}}"#,
            self.request_scoped_type, self.function_scoped_type
        );
        Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.into_bytes()))
    }
}

/// Tracks which types are currently being resolved to detect cycles and scope violations.
///
/// This uses a stack-based approach: when we start resolving a type, we push
/// its `TypeId`, name, and scope onto the stack. If we see the same `TypeId` again
/// before finishing, we have a cycle. Additionally, if a request-scoped dependency
/// tries to resolve a function-scoped dependency, we detect a scope violation.
pub struct ResolutionStack {
    /// Stack of (TypeId, type_name, scope) tuples currently being resolved.
    stack: RwLock<Vec<(TypeId, String, DependencyScope)>>,
}

impl ResolutionStack {
    /// Create an empty resolution stack.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stack: RwLock::new(Vec::new()),
        }
    }

    /// Check if a type is currently being resolved (would form a cycle).
    ///
    /// Returns `Some(cycle_path)` if the type is already on the stack,
    /// or `None` if it's safe to proceed.
    pub fn check_cycle<T: 'static>(&self, type_name: &str) -> Option<Vec<String>> {
        let type_id = TypeId::of::<T>();
        let guard = self.stack.read();

        // Check if this type is already being resolved
        if let Some(pos) = guard.iter().position(|(id, _, _)| *id == type_id) {
            // Build the cycle path from the position where we first saw this type
            let mut cycle: Vec<String> = guard[pos..]
                .iter()
                .map(|(_, name, _)| name.clone())
                .collect();
            // Add the current type to complete the cycle
            cycle.push(type_name.to_owned());
            return Some(cycle);
        }

        None
    }

    /// Check for scope violations.
    ///
    /// Returns `Some(DependencyScopeError)` if there is a request-scoped dependency
    /// on the stack and we're trying to resolve a function-scoped dependency.
    ///
    /// # Scope Rules
    ///
    /// - Request-scoped can depend on request-scoped (both cached, OK)
    /// - Function-scoped can depend on function-scoped (both fresh, OK)
    /// - Function-scoped can depend on request-scoped (inner cached, outer fresh, OK)
    /// - Request-scoped CANNOT depend on function-scoped (outer cached with stale inner, BAD)
    pub fn check_scope_violation(
        &self,
        type_name: &str,
        scope: DependencyScope,
    ) -> Option<DependencyScopeError> {
        // Only check if we're resolving a function-scoped dependency
        if scope != DependencyScope::Function {
            return None;
        }

        let guard = self.stack.read();

        // Find any request-scoped dependency on the stack
        // (i.e., an outer request-scoped dependency trying to use this function-scoped one)
        for (_, name, dep_scope) in guard.iter().rev() {
            if *dep_scope == DependencyScope::Request {
                return Some(DependencyScopeError::new(
                    name.clone(),
                    type_name.to_owned(),
                ));
            }
        }

        None
    }

    /// Push a type onto the resolution stack with its scope.
    ///
    /// Call this when starting to resolve a dependency.
    pub fn push<T: 'static>(&self, type_name: &str, scope: DependencyScope) {
        let mut guard = self.stack.write();
        guard.push((TypeId::of::<T>(), type_name.to_owned(), scope));
    }

    /// Pop a type from the resolution stack.
    ///
    /// Call this when done resolving a dependency (success or error).
    pub fn pop(&self) {
        let mut guard = self.stack.write();
        guard.pop();
    }

    /// Get the current depth of the resolution stack.
    #[must_use]
    pub fn depth(&self) -> usize {
        let guard = self.stack.read();
        guard.len()
    }

    /// Check if the stack is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.depth() == 0
    }
}

impl Default for ResolutionStack {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ResolutionStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let guard = self.stack.read();
        f.debug_struct("ResolutionStack")
            .field("depth", &guard.len())
            .field(
                "types",
                &guard
                    .iter()
                    .map(|(_, name, scope)| format!("{}({:?})", name, scope))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// RAII guard that automatically pops from the resolution stack on drop.
///
/// This ensures the stack is always properly maintained, even if the
/// resolution fails or panics.
pub struct ResolutionGuard<'a> {
    stack: &'a ResolutionStack,
}

impl<'a> ResolutionGuard<'a> {
    /// Create a new guard that will pop from the stack when dropped.
    fn new(stack: &'a ResolutionStack) -> Self {
        Self { stack }
    }
}

impl Drop for ResolutionGuard<'_> {
    fn drop(&mut self) {
        self.stack.pop();
    }
}

/// Dependency injection extractor.
#[derive(Debug, Clone)]
pub struct Depends<T, C = DefaultDependencyConfig>(pub T, PhantomData<C>);

impl<T, C> Depends<T, C> {
    /// Create a new `Depends` wrapper.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self(value, PhantomData)
    }

    /// Unwrap the inner value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, C> Deref for Depends<T, C> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, C> DerefMut for Depends<T, C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Trait for types that can be injected as dependencies.
pub trait FromDependency: Clone + Send + Sync + 'static {
    /// Error type when dependency resolution fails.
    type Error: IntoResponse + Send + Sync + 'static;

    /// Resolve the dependency.
    fn from_dependency(
        ctx: &RequestContext,
        req: &mut Request,
    ) -> impl Future<Output = Result<Self, Self::Error>> + Send;
}

impl<T, C> FromRequest for Depends<T, C>
where
    T: FromDependency,
    C: DependsConfig,
{
    type Error = T::Error;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Check cancellation before resolving dependency (overrides or normal)
        let _ = ctx.checkpoint();

        // Check overrides first (testing support)
        if let Some(result) = ctx.dependency_overrides().resolve::<T>(ctx, req).await {
            return result.map(Depends::new);
        }

        let scope = C::SCOPE.unwrap_or(DependencyScope::Request);
        let use_cache = C::USE_CACHE && scope == DependencyScope::Request;

        // Check cache - if already resolved, no cycle possible
        if use_cache {
            if let Some(cached) = ctx.dependency_cache().get::<T>() {
                return Ok(Depends::new(cached));
            }
        }

        // Check for circular dependency before attempting resolution
        let type_name = std::any::type_name::<T>();
        if let Some(cycle) = ctx.resolution_stack().check_cycle::<T>(type_name) {
            let err = CircularDependencyError::new(cycle);
            panic!("{}", err);
        }

        // Check for scope violation: request-scoped cannot depend on function-scoped
        if let Some(scope_err) = ctx
            .resolution_stack()
            .check_scope_violation(type_name, scope)
        {
            panic!("{}", scope_err);
        }

        // Push onto resolution stack and create guard for automatic cleanup
        ctx.resolution_stack().push::<T>(type_name, scope);
        let _guard = ResolutionGuard::new(ctx.resolution_stack());

        // Resolve the dependency
        let _ = ctx.checkpoint();
        let value = T::from_dependency(ctx, req).await?;

        // Cache if needed (guard will pop stack when dropped)
        if use_cache {
            ctx.dependency_cache().insert::<T>(value.clone());
        }

        Ok(Depends::new(value))
    }
}

/// Request-scoped dependency cache.
pub struct DependencyCache {
    inner: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
}

impl DependencyCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Get a cached dependency by type.
    #[must_use]
    pub fn get<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        let guard = self.inner.read();
        guard
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
            .cloned()
    }

    /// Insert a dependency into the cache.
    pub fn insert<T: Clone + Send + Sync + 'static>(&self, value: T) {
        let mut guard = self.inner.write();
        guard.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Clear all cached dependencies.
    pub fn clear(&self) {
        let mut guard = self.inner.write();
        guard.clear();
    }

    /// Return the number of cached dependencies.
    #[must_use]
    pub fn len(&self) -> usize {
        let guard = self.inner.read();
        guard.len()
    }

    /// Returns true if no dependencies are cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for DependencyCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for DependencyCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DependencyCache")
            .field("size", &self.len())
            .finish()
    }
}

type OverrideBox = Box<dyn Any + Send + Sync>;
type OverrideFuture = Pin<Box<dyn Future<Output = Result<OverrideBox, OverrideBox>> + Send>>;
type OverrideFn = Arc<dyn Fn(&RequestContext, &mut Request) -> OverrideFuture + Send + Sync>;

/// Dependency override registry (primarily for testing).
pub struct DependencyOverrides {
    inner: RwLock<HashMap<TypeId, OverrideFn>>,
}

impl DependencyOverrides {
    /// Create an empty overrides registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Register an override resolver for a dependency type.
    pub fn insert<T, F, Fut>(&self, f: F)
    where
        T: FromDependency,
        F: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, T::Error>> + Send + 'static,
    {
        let wrapper: OverrideFn = Arc::new(move |ctx, req| {
            let fut = f(ctx, req);
            Box::pin(async move {
                match fut.await {
                    Ok(value) => Ok(Box::new(value) as OverrideBox),
                    Err(err) => Err(Box::new(err) as OverrideBox),
                }
            })
        });

        let mut guard = self.inner.write();
        guard.insert(TypeId::of::<T>(), wrapper);
    }

    /// Register a fixed override value for a dependency type.
    pub fn insert_value<T>(&self, value: T)
    where
        T: FromDependency,
    {
        self.insert::<T, _, _>(move |_ctx, _req| {
            let value = value.clone();
            async move { Ok(value) }
        });
    }

    /// Clear all overrides.
    pub fn clear(&self) {
        let mut guard = self.inner.write();
        guard.clear();
    }

    /// Resolve an override if one exists for `T`.
    pub async fn resolve<T>(
        &self,
        ctx: &RequestContext,
        req: &mut Request,
    ) -> Option<Result<T, T::Error>>
    where
        T: FromDependency,
    {
        let override_fn = {
            let guard = self.inner.read();
            guard.get(&TypeId::of::<T>()).cloned()
        };

        let override_fn = override_fn?;
        match override_fn(ctx, req).await {
            Ok(value) => {
                let value = value
                    .downcast::<T>()
                    .expect("dependency override type mismatch");
                Some(Ok(*value))
            }
            Err(err) => {
                let err = err
                    .downcast::<T::Error>()
                    .expect("dependency override error type mismatch");
                Some(Err(*err))
            }
        }
    }

    /// Return the number of overrides registered.
    #[must_use]
    pub fn len(&self) -> usize {
        let guard = self.inner.read();
        guard.len()
    }

    /// Returns true if no overrides are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for DependencyOverrides {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for DependencyOverrides {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DependencyOverrides")
            .field("size", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HttpError;
    use crate::request::Method;
    use asupersync::Cx;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn test_context(overrides: Option<Arc<DependencyOverrides>>) -> RequestContext {
        let cx = Cx::for_testing();
        let request_id = 1;
        if let Some(overrides) = overrides {
            RequestContext::with_overrides(cx, request_id, overrides)
        } else {
            RequestContext::new(cx, request_id)
        }
    }

    fn empty_request() -> Request {
        Request::new(Method::Get, "/")
    }

    #[derive(Clone)]
    struct CounterDep {
        value: usize,
    }

    impl FromDependency for CounterDep {
        type Error = HttpError;

        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Ok(CounterDep { value: 1 })
        }
    }

    #[test]
    fn depends_basic_resolution() {
        let ctx = test_context(None);
        let mut req = empty_request();
        let dep = futures_executor::block_on(Depends::<CounterDep>::from_request(&ctx, &mut req))
            .expect("dependency resolution failed");
        assert_eq!(dep.value, 1);
    }

    #[derive(Clone)]
    struct CountingDep;

    impl FromDependency for CountingDep {
        type Error = HttpError;

        async fn from_dependency(
            ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            let count = ctx
                .dependency_cache()
                .get::<Arc<AtomicUsize>>()
                .unwrap_or_else(|| Arc::new(AtomicUsize::new(0)));
            count.fetch_add(1, Ordering::SeqCst);
            ctx.dependency_cache().insert(Arc::clone(&count));
            Ok(CountingDep)
        }
    }

    #[test]
    fn depends_caches_per_request() {
        let ctx = test_context(None);
        let mut req = empty_request();

        let _ = futures_executor::block_on(Depends::<CountingDep>::from_request(&ctx, &mut req))
            .expect("first resolution failed");
        let _ = futures_executor::block_on(Depends::<CountingDep>::from_request(&ctx, &mut req))
            .expect("second resolution failed");

        let counter = ctx
            .dependency_cache()
            .get::<Arc<AtomicUsize>>()
            .expect("missing counter");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn depends_no_cache_config() {
        let ctx = test_context(None);
        let mut req = empty_request();

        let _ = futures_executor::block_on(Depends::<CountingDep, NoCache>::from_request(
            &ctx, &mut req,
        ))
        .expect("first resolution failed");
        let _ = futures_executor::block_on(Depends::<CountingDep, NoCache>::from_request(
            &ctx, &mut req,
        ))
        .expect("second resolution failed");

        let counter = ctx
            .dependency_cache()
            .get::<Arc<AtomicUsize>>()
            .expect("missing counter");
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[derive(Clone)]
    struct DepB;

    impl FromDependency for DepB {
        type Error = HttpError;

        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Ok(DepB)
        }
    }

    #[derive(Clone)]
    struct DepA;

    impl FromDependency for DepA {
        type Error = HttpError;

        async fn from_dependency(
            ctx: &RequestContext,
            req: &mut Request,
        ) -> Result<Self, Self::Error> {
            let _ = Depends::<DepB>::from_request(ctx, req).await?;
            Ok(DepA)
        }
    }

    #[test]
    fn depends_nested_resolution() {
        let ctx = test_context(None);
        let mut req = empty_request();
        let _ = futures_executor::block_on(Depends::<DepA>::from_request(&ctx, &mut req))
            .expect("nested resolution failed");
    }

    #[derive(Clone, Debug)]
    struct OverrideDep {
        value: usize,
    }

    impl FromDependency for OverrideDep {
        type Error = HttpError;

        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Ok(OverrideDep { value: 1 })
        }
    }

    #[test]
    fn depends_override_substitution() {
        let overrides = Arc::new(DependencyOverrides::new());
        overrides.insert_value(OverrideDep { value: 42 });
        let ctx = test_context(Some(overrides));
        let mut req = empty_request();

        let dep = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .expect("override resolution failed");
        assert_eq!(dep.value, 42);
    }

    #[derive(Clone, Debug)]
    struct ErrorDep;

    impl FromDependency for ErrorDep {
        type Error = HttpError;

        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Err(HttpError::bad_request().with_detail("boom"))
        }
    }

    #[test]
    fn depends_error_propagation() {
        let ctx = test_context(None);
        let mut req = empty_request();
        let err = futures_executor::block_on(Depends::<ErrorDep>::from_request(&ctx, &mut req))
            .expect_err("expected dependency error");
        assert_eq!(err.status.as_u16(), 400);
    }

    #[derive(Clone)]
    struct DepC;

    impl FromDependency for DepC {
        type Error = HttpError;

        async fn from_dependency(
            ctx: &RequestContext,
            req: &mut Request,
        ) -> Result<Self, Self::Error> {
            let _ = Depends::<DepA>::from_request(ctx, req).await?;
            let _ = Depends::<DepB>::from_request(ctx, req).await?;
            Ok(DepC)
        }
    }

    #[test]
    fn depends_complex_graph() {
        let ctx = test_context(None);
        let mut req = empty_request();
        let _ = futures_executor::block_on(Depends::<DepC>::from_request(&ctx, &mut req))
            .expect("complex graph resolution failed");
    }

    // ========================================================================
    // Circular Dependency Detection Tests
    // ========================================================================
    //
    // NOTE: Testing actual runtime circular dependencies between types is
    // difficult because Rust's async trait machinery detects type-level
    // cycles at compile time. Instead, we test:
    // 1. The ResolutionStack directly (the core mechanism)
    // 2. CircularDependencyError formatting
    // 3. Diamond patterns (no false positives)
    // ========================================================================

    // Test: ResolutionStack detects simple A -> B -> A cycle
    #[test]
    fn resolution_stack_detects_simple_cycle() {
        // Create unique marker types for cycle detection
        struct TypeA;
        struct TypeB;

        let stack = ResolutionStack::new();

        // Simulate: A starts resolving
        // Before pushing, check that TypeA is not on stack
        assert!(stack.check_cycle::<TypeA>("TypeA").is_none());
        stack.push::<TypeA>("TypeA", DependencyScope::Request);

        // Simulate: A resolves B (different type, should not detect cycle)
        assert!(stack.check_cycle::<TypeB>("TypeB").is_none());
        stack.push::<TypeB>("TypeB", DependencyScope::Request);

        // Simulate: B tries to resolve A (cycle!)
        let cycle = stack.check_cycle::<TypeA>("TypeA");
        assert!(cycle.is_some(), "Should detect A -> B -> A cycle");
        let cycle_path = cycle.unwrap();
        assert_eq!(cycle_path.len(), 3); // TypeA, TypeB, TypeA
        assert_eq!(cycle_path[0], "TypeA");
        assert_eq!(cycle_path[1], "TypeB");
        assert_eq!(cycle_path[2], "TypeA");
    }

    // Test: ResolutionStack detects long cycle A -> B -> C -> D -> A
    #[test]
    fn resolution_stack_detects_long_cycle() {
        // Create unique marker types for the test
        struct TypeA;
        struct TypeB;
        struct TypeC;
        struct TypeD;

        let stack = ResolutionStack::new();

        // Simulate: A -> B -> C -> D -> A
        stack.push::<TypeA>("TypeA", DependencyScope::Request);
        stack.push::<TypeB>("TypeB", DependencyScope::Request);
        stack.push::<TypeC>("TypeC", DependencyScope::Request);
        stack.push::<TypeD>("TypeD", DependencyScope::Request);

        // D tries to resolve A (cycle!)
        let cycle = stack.check_cycle::<TypeA>("TypeA");
        assert!(cycle.is_some(), "Should detect A -> B -> C -> D -> A cycle");
        let cycle_path = cycle.unwrap();
        assert_eq!(cycle_path.len(), 5); // TypeA, TypeB, TypeC, TypeD, TypeA
        assert_eq!(cycle_path[0], "TypeA");
        assert_eq!(cycle_path[4], "TypeA");
    }

    // Test: ResolutionStack allows diamond pattern (no false positive)
    #[test]
    fn resolution_stack_allows_diamond() {
        struct Top;
        struct Left;
        struct Right;
        struct Bottom;

        let stack = ResolutionStack::new();

        // Simulate: Top -> Left -> Bottom (complete Left branch)
        stack.push::<Top>("Top", DependencyScope::Request);
        stack.push::<Left>("Left", DependencyScope::Request);
        stack.push::<Bottom>("Bottom", DependencyScope::Request);
        stack.pop(); // Bottom done
        stack.pop(); // Left done

        // Simulate: Top -> Right -> Bottom (Right branch)
        stack.push::<Right>("Right", DependencyScope::Request);
        // Bottom again - but it's NOT a cycle because we popped it
        assert!(
            stack.check_cycle::<Bottom>("Bottom").is_none(),
            "Diamond pattern should not be detected as a cycle"
        );
        stack.push::<Bottom>("Bottom", DependencyScope::Request);
        stack.pop(); // Bottom done
        stack.pop(); // Right done
        stack.pop(); // Top done

        assert!(stack.is_empty());
    }

    // Test: ResolutionStack detects self-dependency
    #[test]
    fn resolution_stack_detects_self_dependency() {
        struct SelfRef;

        let stack = ResolutionStack::new();
        stack.push::<SelfRef>("SelfRef", DependencyScope::Request);

        // Try to resolve same type again (self-cycle!)
        let cycle = stack.check_cycle::<SelfRef>("SelfRef");
        assert!(cycle.is_some(), "Should detect self-dependency");
        let cycle_path = cycle.unwrap();
        assert_eq!(cycle_path.len(), 2); // SelfRef, SelfRef
        assert_eq!(cycle_path[0], "SelfRef");
        assert_eq!(cycle_path[1], "SelfRef");
    }

    // Test: ResolutionStack basic functionality
    #[test]
    fn resolution_stack_basic() {
        let stack = ResolutionStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.depth(), 0);

        stack.push::<CounterDep>("CounterDep", DependencyScope::Request);
        assert!(!stack.is_empty());
        assert_eq!(stack.depth(), 1);

        // No cycle yet - different type
        assert!(stack.check_cycle::<ErrorDep>("ErrorDep").is_none());

        // Push another type
        stack.push::<ErrorDep>("ErrorDep", DependencyScope::Request);
        assert_eq!(stack.depth(), 2);

        // Check for cycle with first type - should detect it
        let cycle = stack.check_cycle::<CounterDep>("CounterDep");
        assert!(cycle.is_some());
        let cycle_path = cycle.unwrap();
        assert!(cycle_path.contains(&"CounterDep".to_string()));

        // Pop and verify
        stack.pop();
        assert_eq!(stack.depth(), 1);
        stack.pop();
        assert!(stack.is_empty());
    }

    // Test: CircularDependencyError formatting
    #[test]
    fn circular_dependency_error_formatting() {
        let err = CircularDependencyError::new(vec![
            "DbPool".to_string(),
            "UserService".to_string(),
            "AuthService".to_string(),
            "DbPool".to_string(),
        ]);
        let msg = err.to_string();
        assert!(msg.contains("Circular dependency detected"));
        assert!(msg.contains("DbPool -> UserService -> AuthService -> DbPool"));
        assert_eq!(
            err.cycle_path(),
            "DbPool -> UserService -> AuthService -> DbPool"
        );
    }

    // Test: CircularDependencyError into_response
    #[test]
    fn circular_dependency_error_into_response() {
        let err =
            CircularDependencyError::new(vec!["A".to_string(), "B".to_string(), "A".to_string()]);
        let response = err.into_response();
        assert_eq!(response.status().as_u16(), 500);
    }

    // Test: Diamond dependency graph resolves correctly (runtime test)
    #[derive(Clone)]
    struct DiamondBottom {
        id: u32,
    }
    #[derive(Clone)]
    struct DiamondLeft {
        bottom_id: u32,
    }
    #[derive(Clone)]
    struct DiamondRight {
        bottom_id: u32,
    }
    #[derive(Clone)]
    struct DiamondTop {
        left_id: u32,
        right_id: u32,
    }

    impl FromDependency for DiamondBottom {
        type Error = HttpError;
        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Ok(DiamondBottom { id: 42 })
        }
    }

    impl FromDependency for DiamondLeft {
        type Error = HttpError;
        async fn from_dependency(
            ctx: &RequestContext,
            req: &mut Request,
        ) -> Result<Self, Self::Error> {
            let bottom = Depends::<DiamondBottom>::from_request(ctx, req).await?;
            Ok(DiamondLeft {
                bottom_id: bottom.id,
            })
        }
    }

    impl FromDependency for DiamondRight {
        type Error = HttpError;
        async fn from_dependency(
            ctx: &RequestContext,
            req: &mut Request,
        ) -> Result<Self, Self::Error> {
            let bottom = Depends::<DiamondBottom>::from_request(ctx, req).await?;
            Ok(DiamondRight {
                bottom_id: bottom.id,
            })
        }
    }

    impl FromDependency for DiamondTop {
        type Error = HttpError;
        async fn from_dependency(
            ctx: &RequestContext,
            req: &mut Request,
        ) -> Result<Self, Self::Error> {
            let left = Depends::<DiamondLeft>::from_request(ctx, req).await?;
            let right = Depends::<DiamondRight>::from_request(ctx, req).await?;
            Ok(DiamondTop {
                left_id: left.bottom_id,
                right_id: right.bottom_id,
            })
        }
    }

    #[test]
    fn diamond_pattern_resolves_correctly() {
        let ctx = test_context(None);
        let mut req = empty_request();
        // Diamond pattern should NOT trigger circular dependency detection
        let result =
            futures_executor::block_on(Depends::<DiamondTop>::from_request(&ctx, &mut req));
        assert!(result.is_ok(), "Diamond pattern should not be a cycle");
        let top = result.unwrap();
        assert_eq!(top.left_id, 42);
        assert_eq!(top.right_id, 42);
    }

    // ========================================================================
    // Additional DI Tests for Coverage (fastapi_rust-zf4)
    // ========================================================================

    // Test: Override affects nested dependencies
    #[derive(Clone)]
    struct NestedInnerDep {
        value: String,
    }

    impl FromDependency for NestedInnerDep {
        type Error = HttpError;
        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Ok(NestedInnerDep {
                value: "original".to_string(),
            })
        }
    }

    #[derive(Clone)]
    struct NestedOuterDep {
        inner_value: String,
    }

    impl FromDependency for NestedOuterDep {
        type Error = HttpError;
        async fn from_dependency(
            ctx: &RequestContext,
            req: &mut Request,
        ) -> Result<Self, Self::Error> {
            let inner = Depends::<NestedInnerDep>::from_request(ctx, req).await?;
            Ok(NestedOuterDep {
                inner_value: inner.value.clone(),
            })
        }
    }

    #[test]
    fn override_affects_nested_dependencies() {
        let overrides = Arc::new(DependencyOverrides::new());
        overrides.insert_value(NestedInnerDep {
            value: "overridden".to_string(),
        });
        let ctx = test_context(Some(overrides));
        let mut req = empty_request();

        let result =
            futures_executor::block_on(Depends::<NestedOuterDep>::from_request(&ctx, &mut req))
                .expect("nested override resolution failed");

        assert_eq!(
            result.inner_value, "overridden",
            "Override should propagate to nested dependencies"
        );
    }

    // Test: Clear overrides works
    #[test]
    fn clear_overrides_restores_original() {
        let overrides = Arc::new(DependencyOverrides::new());
        overrides.insert_value(OverrideDep { value: 99 });
        assert_eq!(overrides.len(), 1);

        overrides.clear();
        assert_eq!(overrides.len(), 0);
        assert!(overrides.is_empty());

        // After clear, should return None (no override)
        let ctx = test_context(Some(overrides));
        let mut req = empty_request();
        let result =
            futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
                .expect("resolution after clear failed");

        // Should get original value (1) not overridden value (99)
        assert_eq!(result.value, 1, "After clear, should resolve to original");
    }

    // Test: Multiple dependencies in handler simulation
    #[derive(Clone)]
    struct DepX {
        x: i32,
    }
    #[derive(Clone)]
    struct DepY {
        y: i32,
    }
    #[derive(Clone)]
    struct DepZ {
        z: i32,
    }

    impl FromDependency for DepX {
        type Error = HttpError;
        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Ok(DepX { x: 10 })
        }
    }

    impl FromDependency for DepY {
        type Error = HttpError;
        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Ok(DepY { y: 20 })
        }
    }

    impl FromDependency for DepZ {
        type Error = HttpError;
        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Ok(DepZ { z: 30 })
        }
    }

    #[test]
    fn multiple_independent_dependencies() {
        let ctx = test_context(None);
        let mut req = empty_request();

        // Simulate a handler that needs X, Y, and Z
        let dep_x =
            futures_executor::block_on(Depends::<DepX>::from_request(&ctx, &mut req)).unwrap();
        let dep_y =
            futures_executor::block_on(Depends::<DepY>::from_request(&ctx, &mut req)).unwrap();
        let dep_z =
            futures_executor::block_on(Depends::<DepZ>::from_request(&ctx, &mut req)).unwrap();

        assert_eq!(dep_x.x, 10);
        assert_eq!(dep_y.y, 20);
        assert_eq!(dep_z.z, 30);
    }

    // Test: Request scope isolation (different contexts have independent caches)
    #[test]
    fn request_scope_isolation() {
        // Request 1
        let ctx1 = test_context(None);
        let mut req1 = empty_request();

        let _ = futures_executor::block_on(Depends::<CountingDep>::from_request(&ctx1, &mut req1))
            .unwrap();
        let counter1 = ctx1.dependency_cache().get::<Arc<AtomicUsize>>().unwrap();

        // Request 2 - fresh context, fresh cache
        let ctx2 = test_context(None);
        let mut req2 = empty_request();

        let _ = futures_executor::block_on(Depends::<CountingDep>::from_request(&ctx2, &mut req2))
            .unwrap();
        let counter2 = ctx2.dependency_cache().get::<Arc<AtomicUsize>>().unwrap();

        // Each request should have its own counter
        assert_eq!(counter1.load(Ordering::SeqCst), 1);
        assert_eq!(counter2.load(Ordering::SeqCst), 1);

        // They should be different Arc instances
        assert!(!Arc::ptr_eq(&counter1, &counter2));
    }

    // Test: Function scope (NoCache) creates fresh instance each time
    #[test]
    fn function_scope_no_caching() {
        let ctx = test_context(None);
        let mut req = empty_request();

        // First call with NoCache
        let _ = futures_executor::block_on(Depends::<CountingDep, NoCache>::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        // Second call with NoCache - should create new instance
        let _ = futures_executor::block_on(Depends::<CountingDep, NoCache>::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        // Third call with NoCache
        let _ = futures_executor::block_on(Depends::<CountingDep, NoCache>::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        let counter = ctx.dependency_cache().get::<Arc<AtomicUsize>>().unwrap();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "NoCache should resolve 3 times"
        );
    }

    // Test: Async dependency execution (verifies async works correctly)
    #[derive(Clone)]
    struct AsyncDep {
        computed: u64,
    }

    impl FromDependency for AsyncDep {
        type Error = HttpError;
        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            // Simulate some async computation
            let result = async {
                let a = 21u64;
                let b = 21u64;
                a + b
            }
            .await;
            Ok(AsyncDep { computed: result })
        }
    }

    #[test]
    fn async_dependency_resolution() {
        let ctx = test_context(None);
        let mut req = empty_request();

        let dep = futures_executor::block_on(Depends::<AsyncDep>::from_request(&ctx, &mut req))
            .expect("async dependency resolution failed");

        assert_eq!(dep.computed, 42);
    }

    // Test: Error in nested dependency propagates correctly
    #[derive(Clone, Debug)]
    struct DepThatDependsOnError;

    impl FromDependency for DepThatDependsOnError {
        type Error = HttpError;
        async fn from_dependency(
            ctx: &RequestContext,
            req: &mut Request,
        ) -> Result<Self, Self::Error> {
            // This will fail because ErrorDep always returns an error
            let _ = Depends::<ErrorDep>::from_request(ctx, req).await?;
            Ok(DepThatDependsOnError)
        }
    }

    #[test]
    fn nested_error_propagation() {
        let ctx = test_context(None);
        let mut req = empty_request();

        let result = futures_executor::block_on(Depends::<DepThatDependsOnError>::from_request(
            &ctx, &mut req,
        ));

        assert!(result.is_err(), "Nested error should propagate");
        let err = result.unwrap_err();
        assert_eq!(err.status.as_u16(), 400);
    }

    // Test: DependencyCache clear and len methods
    #[test]
    fn dependency_cache_operations() {
        let cache = DependencyCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);

        cache.insert::<String>("test".to_string());
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);

        cache.insert::<i32>(42);
        assert_eq!(cache.len(), 2);

        let retrieved = cache.get::<String>().unwrap();
        assert_eq!(retrieved, "test");

        cache.clear();
        assert!(cache.is_empty());
        assert!(cache.get::<String>().is_none());
    }

    // Test: DependencyOverrides dynamic resolver
    #[test]
    fn dynamic_override_resolver() {
        let overrides = Arc::new(DependencyOverrides::new());

        // Register a dynamic resolver that computes value based on context
        overrides.insert::<OverrideDep, _, _>(|_ctx, _req| async move {
            // Could use ctx/req to compute different values
            Ok(OverrideDep { value: 100 })
        });

        let ctx = test_context(Some(overrides));
        let mut req = empty_request();

        let dep = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .expect("dynamic override failed");

        assert_eq!(dep.value, 100);
    }

    // ---- Comprehensive DependencyOverrides tests (bd-3pbd) ----

    #[test]
    fn overrides_new_is_empty() {
        let overrides = DependencyOverrides::new();
        assert!(overrides.is_empty());
        assert_eq!(overrides.len(), 0);
    }

    #[test]
    fn overrides_default_is_empty() {
        let overrides = DependencyOverrides::default();
        assert!(overrides.is_empty());
        assert_eq!(overrides.len(), 0);
    }

    #[test]
    fn overrides_debug_format() {
        let overrides = DependencyOverrides::new();
        let debug = format!("{:?}", overrides);
        assert!(debug.contains("DependencyOverrides"));
        assert!(debug.contains("size"));
    }

    #[test]
    fn overrides_insert_value_increments_len() {
        let overrides = DependencyOverrides::new();
        assert_eq!(overrides.len(), 0);

        overrides.insert_value(OverrideDep { value: 42 });
        assert_eq!(overrides.len(), 1);
        assert!(!overrides.is_empty());
    }

    #[test]
    fn overrides_multiple_types_registered() {
        let overrides = Arc::new(DependencyOverrides::new());
        overrides.insert_value(OverrideDep { value: 10 });
        overrides.insert_value(NestedInnerDep {
            value: "mocked".to_string(),
        });
        assert_eq!(overrides.len(), 2);

        // Resolve each type independently
        let ctx = test_context(Some(overrides.clone()));
        let mut req = empty_request();

        let dep1 = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .expect("OverrideDep override failed");
        assert_eq!(dep1.value, 10);

        let mut req2 = empty_request();
        let dep2 =
            futures_executor::block_on(Depends::<NestedInnerDep>::from_request(&ctx, &mut req2))
                .expect("NestedInnerDep override failed");
        assert_eq!(dep2.value, "mocked");
    }

    #[test]
    fn overrides_replace_same_type() {
        let overrides = Arc::new(DependencyOverrides::new());
        overrides.insert_value(OverrideDep { value: 1 });
        assert_eq!(overrides.len(), 1);

        // Replace with a different value
        overrides.insert_value(OverrideDep { value: 999 });
        assert_eq!(overrides.len(), 1); // Still 1, replaced

        let ctx = test_context(Some(overrides));
        let mut req = empty_request();

        let dep = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .expect("override resolution failed");
        assert_eq!(dep.value, 999);
    }

    #[test]
    fn overrides_clear_removes_all() {
        let overrides = DependencyOverrides::new();
        overrides.insert_value(OverrideDep { value: 42 });
        overrides.insert_value(NestedInnerDep {
            value: "mock".to_string(),
        });
        assert_eq!(overrides.len(), 2);

        overrides.clear();
        assert!(overrides.is_empty());
        assert_eq!(overrides.len(), 0);
    }

    #[test]
    fn overrides_resolve_returns_none_for_unregistered_type() {
        let overrides = Arc::new(DependencyOverrides::new());
        // Only register OverrideDep
        overrides.insert_value(OverrideDep { value: 42 });

        let ctx = test_context(Some(overrides.clone()));
        let mut req = empty_request();

        // NestedInnerDep is NOT overridden, resolve should return None
        let result =
            futures_executor::block_on(overrides.resolve::<NestedInnerDep>(&ctx, &mut req));
        assert!(result.is_none(), "Unregistered type should resolve to None");
    }

    #[test]
    fn overrides_resolve_some_for_registered_type() {
        let overrides = Arc::new(DependencyOverrides::new());
        overrides.insert_value(OverrideDep { value: 77 });

        let ctx = test_context(Some(overrides.clone()));
        let mut req = empty_request();

        let result = futures_executor::block_on(overrides.resolve::<OverrideDep>(&ctx, &mut req));
        assert!(result.is_some());
        let dep = result.unwrap().expect("resolve should succeed");
        assert_eq!(dep.value, 77);
    }

    #[test]
    fn overrides_not_affect_unrelated_dependency() {
        let overrides = Arc::new(DependencyOverrides::new());
        // Override only NestedInnerDep
        overrides.insert_value(NestedInnerDep {
            value: "overridden".to_string(),
        });

        let ctx = test_context(Some(overrides));
        let mut req = empty_request();

        // OverrideDep should still use its real implementation (returns value: 1)
        let dep = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .expect("should resolve from real implementation");
        assert_eq!(
            dep.value, 1,
            "Unoverridden dep should use real implementation"
        );
    }

    #[test]
    fn overrides_take_precedence_over_cache() {
        let overrides = Arc::new(DependencyOverrides::new());
        overrides.insert_value(OverrideDep { value: 42 });
        let ctx = test_context(Some(overrides));

        // Pre-populate cache with a different value
        ctx.dependency_cache().insert(OverrideDep { value: 999 });

        let mut req = empty_request();
        let dep = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .expect("override should take precedence");

        // Override should win over cache
        assert_eq!(dep.value, 42, "Override should take precedence over cache");
    }

    #[test]
    fn overrides_dynamic_resolver_can_return_error() {
        let overrides = Arc::new(DependencyOverrides::new());

        overrides.insert::<OverrideDep, _, _>(|_ctx, _req| async move {
            Err(
                HttpError::new(crate::response::StatusCode::INTERNAL_SERVER_ERROR)
                    .with_detail("override error"),
            )
        });

        let ctx = test_context(Some(overrides));
        let mut req = empty_request();

        let err = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .expect_err("override should return error");
        assert_eq!(err.status.as_u16(), 500);
    }

    #[test]
    fn overrides_insert_value_works_for_multiple_resolves() {
        // insert_value uses Clone, so the value should work for multiple resolves
        let overrides = Arc::new(DependencyOverrides::new());
        overrides.insert_value(OverrideDep { value: 7 });

        let ctx = test_context(Some(overrides));

        for _ in 0..5 {
            let mut req = empty_request();
            let dep =
                futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
                    .expect("repeated resolve should work");
            assert_eq!(dep.value, 7);
        }
    }

    #[test]
    fn overrides_dynamic_resolver_accesses_request() {
        let overrides = Arc::new(DependencyOverrides::new());

        // Dynamic resolver that reads from request extensions
        overrides.insert::<OverrideDep, _, _>(|_ctx, req| {
            let value = req.get_extension::<usize>().copied().unwrap_or(0);
            async move { Ok(OverrideDep { value }) }
        });

        let ctx = test_context(Some(overrides));
        let mut req = empty_request();
        req.insert_extension(42usize);

        let dep = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .expect("dynamic resolver with request access failed");
        assert_eq!(dep.value, 42, "Dynamic resolver should read from request");
    }

    #[test]
    fn overrides_after_clear_fall_back_to_real_dependency() {
        let overrides = Arc::new(DependencyOverrides::new());
        overrides.insert_value(OverrideDep { value: 999 });

        // Verify override works
        let ctx = test_context(Some(overrides.clone()));
        let mut req = empty_request();
        let dep = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .unwrap();
        assert_eq!(dep.value, 999);

        // Clear and verify fallback to real dependency
        overrides.clear();
        let ctx2 = test_context(Some(overrides));
        let mut req2 = empty_request();
        let dep2 =
            futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx2, &mut req2))
                .unwrap();
        assert_eq!(dep2.value, 1, "After clear, real dependency should be used");
    }

    #[test]
    fn overrides_without_overrides_use_real_dependency() {
        // No overrides at all
        let ctx = test_context(None);
        let mut req = empty_request();

        let dep = futures_executor::block_on(Depends::<OverrideDep>::from_request(&ctx, &mut req))
            .unwrap();
        assert_eq!(
            dep.value, 1,
            "Without overrides, real dependency should be used"
        );
    }

    // Test: ResolutionGuard properly cleans up on drop
    #[test]
    fn resolution_guard_cleanup() {
        let stack = ResolutionStack::new();
        stack.push::<CounterDep>("CounterDep", DependencyScope::Request);
        assert_eq!(stack.depth(), 1);

        {
            // Create guard within a scope
            let _guard = ResolutionGuard::new(&stack);
            stack.push::<ErrorDep>("ErrorDep", DependencyScope::Request);
            assert_eq!(stack.depth(), 2);
            // _guard will pop when dropped
        }

        // After guard drops, one item should be popped
        // Note: guard pops, but we still have CounterDep
        assert_eq!(stack.depth(), 1);
    }

    // ========================================================================
    // CleanupStack Tests (fastapi_rust-9ps)
    // ========================================================================

    #[test]
    fn cleanup_stack_basic() {
        let stack = CleanupStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);

        // Register a cleanup
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);
        stack.push(Box::new(move || {
            Box::pin(async move {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        }));

        assert!(!stack.is_empty());
        assert_eq!(stack.len(), 1);

        // Run cleanups
        let completed = futures_executor::block_on(stack.run_cleanups());
        assert_eq!(completed, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Stack should be empty after running
        assert!(stack.is_empty());
    }

    #[test]
    fn cleanup_stack_lifo_order() {
        // Track cleanup execution order
        let order = Arc::new(parking_lot::Mutex::new(Vec::<i32>::new()));

        let stack = CleanupStack::new();

        // Register cleanups: 1, 2, 3
        for i in 1..=3 {
            let order_clone = Arc::clone(&order);
            stack.push(Box::new(move || {
                Box::pin(async move {
                    order_clone.lock().push(i);
                }) as Pin<Box<dyn Future<Output = ()> + Send>>
            }));
        }

        // Run cleanups - should execute in LIFO order: 3, 2, 1
        futures_executor::block_on(stack.run_cleanups());

        let executed_order = order.lock().clone();
        assert_eq!(
            executed_order,
            vec![3, 2, 1],
            "Cleanups should run in LIFO order"
        );
    }

    #[test]
    fn cleanup_stack_take_cleanups() {
        let stack = CleanupStack::new();

        // Register 3 cleanups
        for _ in 0..3 {
            stack.push(Box::new(|| {
                Box::pin(async {}) as Pin<Box<dyn Future<Output = ()> + Send>>
            }));
        }

        assert_eq!(stack.len(), 3);

        // Take cleanups
        let cleanups = stack.take_cleanups();
        assert_eq!(cleanups.len(), 3);

        // Stack should be empty
        assert!(stack.is_empty());
    }

    #[test]
    fn cleanup_stack_multiple_runs() {
        let counter = Arc::new(AtomicUsize::new(0));
        let stack = CleanupStack::new();

        // First batch
        let counter_clone = Arc::clone(&counter);
        stack.push(Box::new(move || {
            Box::pin(async move {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        }));
        futures_executor::block_on(stack.run_cleanups());

        // Second batch
        let counter_clone = Arc::clone(&counter);
        stack.push(Box::new(move || {
            Box::pin(async move {
                counter_clone.fetch_add(10, Ordering::SeqCst);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        }));
        futures_executor::block_on(stack.run_cleanups());

        assert_eq!(counter.load(Ordering::SeqCst), 11);
    }

    #[test]
    fn cleanup_stack_panic_continues() {
        // Test that if one cleanup panics, remaining cleanups still run (bd-35r4)
        let order = Arc::new(parking_lot::Mutex::new(Vec::<i32>::new()));

        let stack = CleanupStack::new();

        // First cleanup: runs normally
        let order_clone = Arc::clone(&order);
        stack.push(Box::new(move || {
            Box::pin(async move {
                order_clone.lock().push(1);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        }));

        // Second cleanup: panics during creation
        stack.push(Box::new(|| -> Pin<Box<dyn Future<Output = ()> + Send>> {
            panic!("cleanup 2 panics");
        }));

        // Third cleanup: runs normally
        let order_clone = Arc::clone(&order);
        stack.push(Box::new(move || {
            Box::pin(async move {
                order_clone.lock().push(3);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        }));

        // Run cleanups - LIFO order: 3, 2 (panics), 1
        let completed = futures_executor::block_on(stack.run_cleanups());

        // Only 2 should complete (cleanup 2 panicked)
        assert_eq!(completed, 2, "Should report 2 successful cleanups");

        let executed_order = order.lock().clone();
        // Should still run cleanup 3 and 1 despite cleanup 2 panicking
        assert_eq!(
            executed_order,
            vec![3, 1],
            "Cleanups should continue after panic"
        );
    }

    #[test]
    fn cleanup_runs_after_handler_error() {
        // Test that cleanups run even when handler returns an error (bd-35r4)
        // This simulates the server calling run_cleanups after any handler result

        let cleanup_ran = Arc::new(AtomicBool::new(false));
        let cleanup_ran_clone = Arc::clone(&cleanup_ran);

        let ctx = test_context(None);

        // Register a cleanup
        ctx.cleanup_stack().push(Box::new(move || {
            Box::pin(async move {
                cleanup_ran_clone.store(true, Ordering::SeqCst);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        }));

        // Simulate handler returning an error (we just don't use the result)
        let handler_result: Result<(), HttpError> =
            Err(HttpError::new(StatusCode::INTERNAL_SERVER_ERROR).with_detail("handler failed"));

        // Server always runs cleanups after handler, regardless of result
        futures_executor::block_on(ctx.cleanup_stack().run_cleanups());

        assert!(
            cleanup_ran.load(Ordering::SeqCst),
            "Cleanup should run even after handler error"
        );

        // The handler error is still propagated
        assert!(handler_result.is_err());
    }

    // ========================================================================
    // DependsCleanup Tests (fastapi_rust-9ps)
    // ========================================================================

    /// A dependency that tracks setup and cleanup
    #[derive(Clone)]
    struct TrackedResource {
        id: u32,
    }

    impl FromDependencyWithCleanup for TrackedResource {
        type Value = TrackedResource;
        type Error = HttpError;

        async fn setup(
            ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<(Self::Value, Option<CleanupFn>), Self::Error> {
            // Get or create a tracker in the dependency cache
            let tracker = ctx
                .dependency_cache()
                .get::<Arc<parking_lot::Mutex<Vec<String>>>>()
                .unwrap_or_else(|| {
                    let t = Arc::new(parking_lot::Mutex::new(Vec::new()));
                    ctx.dependency_cache().insert(Arc::clone(&t));
                    t
                });

            tracker.lock().push("setup:resource".to_string());

            let cleanup_tracker = Arc::clone(&tracker);
            let cleanup = Box::new(move || {
                Box::pin(async move {
                    cleanup_tracker.lock().push("cleanup:resource".to_string());
                }) as Pin<Box<dyn Future<Output = ()> + Send>>
            }) as CleanupFn;

            Ok((TrackedResource { id: 42 }, Some(cleanup)))
        }
    }

    #[test]
    fn depends_cleanup_registers_cleanup() {
        let ctx = test_context(None);
        let mut req = empty_request();

        // Resolve the dependency
        let dep = futures_executor::block_on(DependsCleanup::<TrackedResource>::from_request(
            &ctx, &mut req,
        ))
        .expect("cleanup dependency resolution failed");

        assert_eq!(dep.id, 42);

        // Cleanup should be registered
        assert_eq!(ctx.cleanup_stack().len(), 1);

        // Get tracker
        let tracker = ctx
            .dependency_cache()
            .get::<Arc<parking_lot::Mutex<Vec<String>>>>()
            .unwrap();

        // Only setup should have run
        let events = tracker.lock().clone();
        assert_eq!(events, vec!["setup:resource"]);

        // Run cleanups
        futures_executor::block_on(ctx.cleanup_stack().run_cleanups());

        // Now cleanup should have run too
        let events = tracker.lock().clone();
        assert_eq!(events, vec!["setup:resource", "cleanup:resource"]);
    }

    /// A dependency without cleanup
    #[derive(Clone)]
    struct NoCleanupResource {
        value: String,
    }

    impl FromDependencyWithCleanup for NoCleanupResource {
        type Value = NoCleanupResource;
        type Error = HttpError;

        async fn setup(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<(Self::Value, Option<CleanupFn>), Self::Error> {
            Ok((
                NoCleanupResource {
                    value: "no cleanup".to_string(),
                },
                None, // No cleanup needed
            ))
        }
    }

    #[test]
    fn depends_cleanup_no_cleanup_fn() {
        let ctx = test_context(None);
        let mut req = empty_request();

        let dep = futures_executor::block_on(DependsCleanup::<NoCleanupResource>::from_request(
            &ctx, &mut req,
        ))
        .expect("no cleanup dependency resolution failed");

        assert_eq!(dep.value, "no cleanup");

        // No cleanup should be registered
        assert!(ctx.cleanup_stack().is_empty());
    }

    /// Nested dependencies with cleanup
    #[derive(Clone)]
    struct OuterWithCleanup {
        inner_id: u32,
    }

    impl FromDependencyWithCleanup for OuterWithCleanup {
        type Value = OuterWithCleanup;
        type Error = HttpError;

        async fn setup(
            ctx: &RequestContext,
            req: &mut Request,
        ) -> Result<(Self::Value, Option<CleanupFn>), Self::Error> {
            // Resolve inner dependency first
            let inner = DependsCleanup::<TrackedResource>::from_request(ctx, req).await?;

            // Get tracker
            let tracker = ctx
                .dependency_cache()
                .get::<Arc<parking_lot::Mutex<Vec<String>>>>()
                .unwrap();

            tracker.lock().push("setup:outer".to_string());

            let cleanup_tracker = Arc::clone(&tracker);
            let cleanup = Box::new(move || {
                Box::pin(async move {
                    cleanup_tracker.lock().push("cleanup:outer".to_string());
                }) as Pin<Box<dyn Future<Output = ()> + Send>>
            }) as CleanupFn;

            Ok((OuterWithCleanup { inner_id: inner.id }, Some(cleanup)))
        }
    }

    #[test]
    fn depends_cleanup_nested_lifo() {
        let ctx = test_context(None);
        let mut req = empty_request();

        // Resolve outer (which resolves inner)
        let dep = futures_executor::block_on(DependsCleanup::<OuterWithCleanup>::from_request(
            &ctx, &mut req,
        ))
        .expect("nested cleanup dependency resolution failed");

        assert_eq!(dep.inner_id, 42);

        // Both cleanups should be registered
        assert_eq!(ctx.cleanup_stack().len(), 2);

        // Get tracker
        let tracker = ctx
            .dependency_cache()
            .get::<Arc<parking_lot::Mutex<Vec<String>>>>()
            .unwrap();

        // Setup order: inner, then outer
        let events_before = tracker.lock().clone();
        assert_eq!(events_before, vec!["setup:resource", "setup:outer"]);

        // Run cleanups - LIFO order: outer first, then inner
        futures_executor::block_on(ctx.cleanup_stack().run_cleanups());

        let events_after = tracker.lock().clone();
        assert_eq!(
            events_after,
            vec![
                "setup:resource",
                "setup:outer",
                "cleanup:outer",
                "cleanup:resource"
            ],
            "Cleanups should run in LIFO order"
        );
    }

    #[test]
    fn depends_cleanup_caching() {
        let ctx = test_context(None);
        let mut req = empty_request();

        // First resolution
        let _dep1 = futures_executor::block_on(DependsCleanup::<TrackedResource>::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        // Second resolution - should use cache
        let _dep2 = futures_executor::block_on(DependsCleanup::<TrackedResource>::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        // Only one cleanup should be registered (due to caching)
        assert_eq!(ctx.cleanup_stack().len(), 1);

        // Get tracker
        let tracker = ctx
            .dependency_cache()
            .get::<Arc<parking_lot::Mutex<Vec<String>>>>()
            .unwrap();

        // Setup should only run once
        let events = tracker.lock().clone();
        assert_eq!(events, vec!["setup:resource"]);
    }

    // ========================================================================
    // Scope Constraint Validation Tests (fastapi_rust-kpe)
    // ========================================================================

    // Test: DependencyScopeError formatting
    #[test]
    fn scope_error_formatting() {
        let err = DependencyScopeError::new("CachedUser".to_string(), "DbConnection".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Dependency scope violation"));
        assert!(msg.contains("request-scoped 'CachedUser'"));
        assert!(msg.contains("function-scoped 'DbConnection'"));
        assert!(msg.contains("cached value would become stale"));
    }

    // Test: DependencyScopeError into_response
    #[test]
    fn scope_error_into_response() {
        let err = DependencyScopeError::new("A".to_string(), "B".to_string());
        let response = err.into_response();
        assert_eq!(response.status().as_u16(), 500);
    }

    // Test: ResolutionStack detects request -> function scope violation
    #[test]
    fn resolution_stack_detects_scope_violation() {
        #[allow(dead_code)]
        struct RequestScoped;
        #[allow(dead_code)]
        struct FunctionScoped;

        let stack = ResolutionStack::new();

        // Push request-scoped dependency
        stack.push::<RequestScoped>("RequestScoped", DependencyScope::Request);

        // Now try to resolve function-scoped - should detect violation
        let violation = stack.check_scope_violation("FunctionScoped", DependencyScope::Function);
        assert!(
            violation.is_some(),
            "Should detect request -> function scope violation"
        );
        let err = violation.unwrap();
        assert_eq!(err.request_scoped_type, "RequestScoped");
        assert_eq!(err.function_scoped_type, "FunctionScoped");
    }

    // Test: ResolutionStack allows request -> request (valid)
    #[test]
    fn resolution_stack_allows_request_to_request() {
        #[allow(dead_code)]
        struct RequestA;
        #[allow(dead_code)]
        struct RequestB;

        let stack = ResolutionStack::new();

        // Push request-scoped dependency
        stack.push::<RequestA>("RequestA", DependencyScope::Request);

        // Another request-scoped is OK
        let violation = stack.check_scope_violation("RequestB", DependencyScope::Request);
        assert!(violation.is_none(), "Request -> Request should be allowed");
    }

    // Test: ResolutionStack allows function -> function (valid)
    #[test]
    fn resolution_stack_allows_function_to_function() {
        #[allow(dead_code)]
        struct FunctionA;
        #[allow(dead_code)]
        struct FunctionB;

        let stack = ResolutionStack::new();

        // Push function-scoped dependency
        stack.push::<FunctionA>("FunctionA", DependencyScope::Function);

        // Another function-scoped is OK
        let violation = stack.check_scope_violation("FunctionB", DependencyScope::Function);
        assert!(
            violation.is_none(),
            "Function -> Function should be allowed"
        );
    }

    // Test: ResolutionStack allows function -> request (valid)
    #[test]
    fn resolution_stack_allows_function_to_request() {
        #[allow(dead_code)]
        struct FunctionScoped;
        #[allow(dead_code)]
        struct RequestScoped;

        let stack = ResolutionStack::new();

        // Push function-scoped dependency
        stack.push::<FunctionScoped>("FunctionScoped", DependencyScope::Function);

        // Request-scoped inner is OK (cached inner is fine for fresh outer)
        let violation = stack.check_scope_violation("RequestScoped", DependencyScope::Request);
        assert!(violation.is_none(), "Function -> Request should be allowed");
    }

    // Test: Nested scope violation detection (A(request) -> B(request) -> C(function))
    #[test]
    fn resolution_stack_nested_scope_violation() {
        #[allow(dead_code)]
        struct OuterRequest;
        #[allow(dead_code)]
        struct MiddleRequest;
        #[allow(dead_code)]
        struct InnerFunction;

        let stack = ResolutionStack::new();

        // Push request -> request -> function
        stack.push::<OuterRequest>("OuterRequest", DependencyScope::Request);
        stack.push::<MiddleRequest>("MiddleRequest", DependencyScope::Request);

        // Inner function-scoped should fail because there's a request-scoped in the chain
        let violation = stack.check_scope_violation("InnerFunction", DependencyScope::Function);
        assert!(violation.is_some(), "Should detect nested scope violation");
        let err = violation.unwrap();
        // Should report the closest request-scoped (MiddleRequest)
        assert_eq!(err.request_scoped_type, "MiddleRequest");
        assert_eq!(err.function_scoped_type, "InnerFunction");
    }

    // Test: Empty stack has no scope violation
    #[test]
    fn resolution_stack_empty_no_scope_violation() {
        let stack = ResolutionStack::new();

        // Empty stack should allow any scope
        let violation_fn = stack.check_scope_violation("SomeDep", DependencyScope::Function);
        let violation_req = stack.check_scope_violation("SomeDep", DependencyScope::Request);

        assert!(
            violation_fn.is_none(),
            "Empty stack should allow function scope"
        );
        assert!(
            violation_req.is_none(),
            "Empty stack should allow request scope"
        );
    }

    // Test: Scope violation in runtime dependency resolution (integration test)
    // This test verifies the actual panic behavior when scope rules are violated.
    //
    // NOTE: We can't easily test the panic in normal unit tests because catch_unwind
    // doesn't work well with async. Instead, we test the check_scope_violation method
    // directly in the tests above. The actual panic behavior is tested via the
    // Depends::from_request implementation.

    // Test: Function-scoped dependency works correctly (NoCache config)
    #[test]
    fn function_scoped_resolves_fresh_each_time() {
        let ctx = test_context(None);
        let mut req = empty_request();

        // Use NoCache (function scope)
        let _ = futures_executor::block_on(Depends::<CountingDep, NoCache>::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        let _ = futures_executor::block_on(Depends::<CountingDep, NoCache>::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        let counter = ctx.dependency_cache().get::<Arc<AtomicUsize>>().unwrap();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "Function-scoped should resolve 2 times (not cached)"
        );
    }

    // Test: Request-scoped dependency is cached correctly
    #[test]
    fn request_scoped_cached_within_request() {
        let ctx = test_context(None);
        let mut req = empty_request();

        // Use default config (request scope)
        let _ = futures_executor::block_on(Depends::<CountingDep>::from_request(&ctx, &mut req))
            .unwrap();

        let _ = futures_executor::block_on(Depends::<CountingDep>::from_request(&ctx, &mut req))
            .unwrap();

        let counter = ctx.dependency_cache().get::<Arc<AtomicUsize>>().unwrap();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "Request-scoped should resolve only once (cached)"
        );
    }

    // ========================================================================
    // Scope and Cleanup Integration Tests (bd-2290)
    // ========================================================================

    /// Test that request-scoped dependencies with cleanup are only cleaned up once
    /// even when resolved multiple times
    #[test]
    fn request_scope_cleanup_only_once() {
        let ctx = test_context(None);
        let mut req = empty_request();

        // Resolve the same cleanup dependency multiple times
        let _ = futures_executor::block_on(DependsCleanup::<TrackedResource>::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        let _ = futures_executor::block_on(DependsCleanup::<TrackedResource>::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        // Only one cleanup should be registered due to caching
        assert_eq!(
            ctx.cleanup_stack().len(),
            1,
            "Request-scoped should only register cleanup once"
        );

        let tracker = ctx
            .dependency_cache()
            .get::<Arc<parking_lot::Mutex<Vec<String>>>>()
            .unwrap();

        // Setup only called once
        assert_eq!(tracker.lock().len(), 1);
        assert_eq!(tracker.lock()[0], "setup:resource");

        // Run cleanups
        futures_executor::block_on(ctx.cleanup_stack().run_cleanups());

        // Only one cleanup ran
        let events = tracker.lock().clone();
        assert_eq!(
            events,
            vec!["setup:resource", "cleanup:resource"],
            "Cleanup should run exactly once for cached dependency"
        );
    }

    /// Test that function-scoped cleanup dependencies register cleanup for each resolution
    #[test]
    fn function_scope_cleanup_each_time() {
        // Create a function-scoped cleanup dependency
        #[derive(Clone)]
        #[allow(dead_code)]
        struct FunctionScopedWithCleanup {
            id: u32,
        }

        impl FromDependencyWithCleanup for FunctionScopedWithCleanup {
            type Value = FunctionScopedWithCleanup;
            type Error = HttpError;

            async fn setup(
                ctx: &RequestContext,
                _req: &mut Request,
            ) -> Result<(Self::Value, Option<CleanupFn>), Self::Error> {
                // Get or create a cleanup counter
                let counter = ctx
                    .dependency_cache()
                    .get::<Arc<AtomicUsize>>()
                    .unwrap_or_else(|| {
                        let c = Arc::new(AtomicUsize::new(0));
                        ctx.dependency_cache().insert(Arc::clone(&c));
                        c
                    });

                let cleanup_counter = Arc::clone(&counter);
                let cleanup = Box::new(move || {
                    Box::pin(async move {
                        cleanup_counter.fetch_add(1, Ordering::SeqCst);
                    }) as Pin<Box<dyn Future<Output = ()> + Send>>
                }) as CleanupFn;

                Ok((FunctionScopedWithCleanup { id: 42 }, Some(cleanup)))
            }
        }

        let ctx = test_context(None);
        let mut req = empty_request();

        // Resolve 3 times with NoCache
        let _ = futures_executor::block_on(
            DependsCleanup::<FunctionScopedWithCleanup, NoCache>::from_request(&ctx, &mut req),
        )
        .unwrap();

        let _ = futures_executor::block_on(
            DependsCleanup::<FunctionScopedWithCleanup, NoCache>::from_request(&ctx, &mut req),
        )
        .unwrap();

        let _ = futures_executor::block_on(
            DependsCleanup::<FunctionScopedWithCleanup, NoCache>::from_request(&ctx, &mut req),
        )
        .unwrap();

        // Should have 3 cleanups registered
        assert_eq!(
            ctx.cleanup_stack().len(),
            3,
            "Function-scoped should register cleanup each time"
        );

        // Run all cleanups
        futures_executor::block_on(ctx.cleanup_stack().run_cleanups());

        // Verify all 3 cleanups ran
        let counter = ctx.dependency_cache().get::<Arc<AtomicUsize>>().unwrap();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "All 3 cleanups should have run"
        );
    }
}
