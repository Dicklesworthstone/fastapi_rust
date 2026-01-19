//! Dependency injection support.
//!
//! This module provides the `Depends` extractor and supporting types for
//! request-scoped dependency resolution with optional caching and overrides.

use crate::context::RequestContext;
use crate::extract::FromRequest;
use crate::request::Request;
use crate::response::IntoResponse;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

/// Dependency resolution scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyScope {
    /// No request-level caching; resolve on each call.
    Function,
    /// Cache for the lifetime of the request.
    Request,
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
        if let Some(result) = ctx.dependency_overrides().resolve::<T>(ctx, req).await {
            return result.map(Depends::new);
        }

        let scope = C::SCOPE.unwrap_or(DependencyScope::Request);
        let use_cache = C::USE_CACHE && scope == DependencyScope::Request;

        if use_cache {
            if let Some(cached) = ctx.dependency_cache().get::<T>() {
                return Ok(Depends::new(cached));
            }
        }

        let value = T::from_dependency(ctx, req).await?;

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
        let guard = self.inner.read().expect("dependency cache poisoned");
        guard
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
            .cloned()
    }

    /// Insert a dependency into the cache.
    pub fn insert<T: Clone + Send + Sync + 'static>(&self, value: T) {
        let mut guard = self.inner.write().expect("dependency cache poisoned");
        guard.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Clear all cached dependencies.
    pub fn clear(&self) {
        let mut guard = self.inner.write().expect("dependency cache poisoned");
        guard.clear();
    }

    /// Return the number of cached dependencies.
    #[must_use]
    pub fn len(&self) -> usize {
        let guard = self.inner.read().expect("dependency cache poisoned");
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

        let mut guard = self.inner.write().expect("dependency overrides poisoned");
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
        let mut guard = self.inner.write().expect("dependency overrides poisoned");
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
            let guard = self.inner.read().expect("dependency overrides poisoned");
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
        let guard = self.inner.read().expect("dependency overrides poisoned");
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[derive(Clone)]
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
}
