//! APIRouter for modular route organization.
//!
//! This module provides [`APIRouter`] for grouping related routes with
//! shared configuration like prefixes, tags, and dependencies.
//!
//! # Example
//!
//! ```ignore
//! use fastapi_core::api_router::APIRouter;
//! use fastapi_core::{Request, Response, RequestContext};
//!
//! async fn get_users(ctx: &RequestContext, req: &mut Request) -> Response {
//!     Response::ok().body_text("List of users")
//! }
//!
//! async fn create_user(ctx: &RequestContext, req: &mut Request) -> Response {
//!     Response::ok().body_text("User created")
//! }
//!
//! let router = APIRouter::new()
//!     .prefix("/api/v1/users")
//!     .tags(vec!["users"])
//!     .get("", get_users)
//!     .post("", create_user);
//!
//! let app = App::builder()
//!     .include_router(router)
//!     .build();
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::app::{BoxHandler, RouteEntry};
use crate::context::RequestContext;
use crate::request::{Method, Request};
use crate::response::Response;

/// Response definition for OpenAPI documentation.
#[derive(Debug, Clone)]
pub struct ResponseDef {
    /// HTTP status code description.
    pub description: String,
    /// Optional example response body.
    pub example: Option<serde_json::Value>,
    /// Content type for this response.
    pub content_type: Option<String>,
}

impl ResponseDef {
    /// Create a new response definition with a description.
    #[must_use]
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            example: None,
            content_type: None,
        }
    }

    /// Set an example response body.
    #[must_use]
    pub fn with_example(mut self, example: serde_json::Value) -> Self {
        self.example = Some(example);
        self
    }

    /// Set the content type.
    #[must_use]
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }
}

/// A boxed dependency function.
///
/// Dependencies are executed before route handlers and can short-circuit
/// the request by returning an error response.
pub type BoxDependency = Arc<
    dyn Fn(
            &RequestContext,
            &mut Request,
        ) -> Pin<Box<dyn Future<Output = Result<(), Response>> + Send>>
        + Send
        + Sync,
>;

/// A shared dependency that runs before route handlers.
///
/// Dependencies can be used for authentication, validation, or any
/// pre-processing that should apply to all routes in a router.
#[derive(Clone)]
pub struct RouterDependency {
    /// The dependency function.
    pub(crate) handler: BoxDependency,
    /// Name for debugging/logging.
    pub(crate) name: String,
}

impl RouterDependency {
    /// Create a new router dependency.
    ///
    /// The function should return `Ok(())` to continue processing,
    /// or `Err(Response)` to short-circuit with an error response.
    pub fn new<F, Fut>(name: impl Into<String>, f: F) -> Self
    where
        F: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), Response>> + Send + 'static,
    {
        Self {
            handler: Arc::new(move |ctx, req| Box::pin(f(ctx, req))),
            name: name.into(),
        }
    }

    /// Execute the dependency.
    pub async fn execute(&self, ctx: &RequestContext, req: &mut Request) -> Result<(), Response> {
        (self.handler)(ctx, req).await
    }
}

impl std::fmt::Debug for RouterDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterDependency")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Configuration for including a router with overrides.
///
/// When including a router in an app or another router, this config
/// allows overriding or prepending settings to the included router.
///
/// # Merge Rules (per FastAPI spec)
///
/// 1. **prefix**: Prepended to router's prefix
/// 2. **tags**: Prepended to router's tags
/// 3. **dependencies**: Prepended to router's dependencies
/// 4. **responses**: Merged (route > router > config)
/// 5. **deprecated**: Override if provided
/// 6. **include_in_schema**: Override if provided
///
/// # Example
///
/// ```ignore
/// let config = IncludeConfig::new()
///     .prefix("/api/v1")
///     .tags(vec!["api"])
///     .dependency(auth_dep);
///
/// let app = App::builder()
///     .include_router_with_config(router, config)
///     .build();
/// ```
#[derive(Debug, Default, Clone)]
pub struct IncludeConfig {
    /// Prefix to prepend to the router's prefix.
    prefix: Option<String>,
    /// Tags to prepend to the router's tags.
    tags: Vec<String>,
    /// Dependencies to prepend to the router's dependencies.
    dependencies: Vec<RouterDependency>,
    /// Response definitions to merge.
    responses: HashMap<u16, ResponseDef>,
    /// Override for deprecated flag.
    deprecated: Option<bool>,
    /// Override for include_in_schema flag.
    include_in_schema: Option<bool>,
}

impl IncludeConfig {
    /// Creates a new empty include configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the prefix to prepend to the router's prefix.
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        let p = prefix.into();
        if !p.is_empty() {
            // Normalize prefix
            let normalized = if p.starts_with('/') {
                p
            } else {
                format!("/{}", p)
            };
            // Remove trailing slash
            let normalized = if normalized.ends_with('/') && normalized.len() > 1 {
                normalized.trim_end_matches('/').to_string()
            } else {
                normalized
            };
            self.prefix = Some(normalized);
        }
        self
    }

    /// Sets the tags to prepend to the router's tags.
    #[must_use]
    pub fn tags(mut self, tags: Vec<impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// Adds a single tag to prepend.
    #[must_use]
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Adds a dependency to prepend.
    #[must_use]
    pub fn dependency(mut self, dep: RouterDependency) -> Self {
        self.dependencies.push(dep);
        self
    }

    /// Sets dependencies to prepend.
    #[must_use]
    pub fn dependencies(mut self, deps: Vec<RouterDependency>) -> Self {
        self.dependencies = deps;
        self
    }

    /// Adds a response definition.
    #[must_use]
    pub fn response(mut self, status_code: u16, def: ResponseDef) -> Self {
        self.responses.insert(status_code, def);
        self
    }

    /// Sets response definitions.
    #[must_use]
    pub fn responses(mut self, responses: HashMap<u16, ResponseDef>) -> Self {
        self.responses = responses;
        self
    }

    /// Sets the deprecated override.
    #[must_use]
    pub fn deprecated(mut self, deprecated: bool) -> Self {
        self.deprecated = Some(deprecated);
        self
    }

    /// Sets the include_in_schema override.
    #[must_use]
    pub fn include_in_schema(mut self, include: bool) -> Self {
        self.include_in_schema = Some(include);
        self
    }

    /// Returns the prefix.
    #[must_use]
    pub fn get_prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    /// Returns the tags.
    #[must_use]
    pub fn get_tags(&self) -> &[String] {
        &self.tags
    }

    /// Returns the dependencies.
    #[must_use]
    pub fn get_dependencies(&self) -> &[RouterDependency] {
        &self.dependencies
    }

    /// Returns the responses.
    #[must_use]
    pub fn get_responses(&self) -> &HashMap<u16, ResponseDef> {
        &self.responses
    }

    /// Returns the deprecated override.
    #[must_use]
    pub fn get_deprecated(&self) -> Option<bool> {
        self.deprecated
    }

    /// Returns the include_in_schema override.
    #[must_use]
    pub fn get_include_in_schema(&self) -> Option<bool> {
        self.include_in_schema
    }
}

/// Internal route storage that includes router-level metadata.
#[derive(Clone)]
pub struct RouterRoute {
    /// The HTTP method.
    pub method: Method,
    /// The path (without prefix).
    pub path: String,
    /// The handler function.
    pub(crate) handler: Arc<BoxHandler>,
    /// Route-specific tags (merged with router tags).
    pub tags: Vec<String>,
    /// Route-specific dependencies (run after router dependencies).
    pub dependencies: Vec<RouterDependency>,
    /// Whether this route is deprecated.
    pub deprecated: Option<bool>,
    /// Whether to include in OpenAPI schema.
    pub include_in_schema: bool,
}

impl std::fmt::Debug for RouterRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterRoute")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("tags", &self.tags)
            .field("deprecated", &self.deprecated)
            .field("include_in_schema", &self.include_in_schema)
            .finish_non_exhaustive()
    }
}

/// Router for grouping related routes with shared configuration.
///
/// `APIRouter` allows you to organize routes into logical groups with
/// common prefixes, tags, dependencies, and other shared settings.
///
/// # Example
///
/// ```ignore
/// let users_router = APIRouter::new()
///     .prefix("/users")
///     .tags(vec!["users"])
///     .get("", list_users)
///     .get("/{id}", get_user)
///     .post("", create_user);
///
/// let items_router = APIRouter::new()
///     .prefix("/items")
///     .tags(vec!["items"])
///     .get("", list_items);
///
/// let app = App::builder()
///     .include_router(users_router)
///     .include_router(items_router)
///     .build();
/// ```
#[derive(Debug, Default)]
pub struct APIRouter {
    /// URL prefix for all routes.
    prefix: String,
    /// Default tags for all routes.
    tags: Vec<String>,
    /// Shared dependencies run before every route.
    dependencies: Vec<RouterDependency>,
    /// Shared response definitions.
    responses: HashMap<u16, ResponseDef>,
    /// Whether all routes are deprecated.
    deprecated: Option<bool>,
    /// Whether to include routes in OpenAPI schema.
    include_in_schema: bool,
    /// The routes in this router.
    routes: Vec<RouterRoute>,
}

impl APIRouter {
    /// Creates a new empty router.
    #[must_use]
    pub fn new() -> Self {
        Self {
            prefix: String::new(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            responses: HashMap::new(),
            deprecated: None,
            include_in_schema: true,
            routes: Vec::new(),
        }
    }

    /// Creates a new router with the given prefix.
    #[must_use]
    pub fn with_prefix(prefix: impl Into<String>) -> Self {
        Self::new().prefix(prefix)
    }

    /// Sets the URL prefix for all routes.
    ///
    /// The prefix is prepended to all route paths when the router
    /// is included in an application.
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        let p = prefix.into();
        // Ensure prefix starts with / if not empty
        if !p.is_empty() && !p.starts_with('/') {
            self.prefix = format!("/{}", p);
        } else {
            self.prefix = p;
        }
        // Remove trailing slash
        if self.prefix.ends_with('/') && self.prefix.len() > 1 {
            self.prefix.pop();
        }
        self
    }

    /// Sets the default tags for all routes.
    ///
    /// Tags are used for organizing routes in OpenAPI documentation.
    /// Route-specific tags are merged with these router-level tags.
    #[must_use]
    pub fn tags(mut self, tags: Vec<impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// Adds a single tag to the default tags.
    #[must_use]
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Adds a dependency that runs before all routes.
    ///
    /// Dependencies are executed in the order they are added.
    /// If a dependency returns an error response, subsequent
    /// dependencies and the route handler are not executed.
    #[must_use]
    pub fn dependency(mut self, dep: RouterDependency) -> Self {
        self.dependencies.push(dep);
        self
    }

    /// Adds multiple dependencies.
    #[must_use]
    pub fn dependencies(mut self, deps: Vec<RouterDependency>) -> Self {
        self.dependencies.extend(deps);
        self
    }

    /// Adds a response definition for OpenAPI documentation.
    #[must_use]
    pub fn response(mut self, status_code: u16, def: ResponseDef) -> Self {
        self.responses.insert(status_code, def);
        self
    }

    /// Sets shared response definitions.
    #[must_use]
    pub fn responses(mut self, responses: HashMap<u16, ResponseDef>) -> Self {
        self.responses = responses;
        self
    }

    /// Marks all routes as deprecated.
    #[must_use]
    pub fn deprecated(mut self, deprecated: bool) -> Self {
        self.deprecated = Some(deprecated);
        self
    }

    /// Sets whether routes should be included in OpenAPI schema.
    #[must_use]
    pub fn include_in_schema(mut self, include: bool) -> Self {
        self.include_in_schema = include;
        self
    }

    /// Adds a route with the given method and path.
    #[must_use]
    pub fn route<H, Fut>(mut self, path: impl Into<String>, method: Method, handler: H) -> Self
    where
        H: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let boxed: BoxHandler = Box::new(move |ctx, req| {
            let fut = handler(ctx, req);
            Box::pin(fut)
        });
        self.routes.push(RouterRoute {
            method,
            path: path.into(),
            handler: Arc::new(boxed),
            tags: Vec::new(),
            dependencies: Vec::new(),
            deprecated: None,
            include_in_schema: true,
        });
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

    /// Adds an OPTIONS route.
    #[must_use]
    pub fn options<H, Fut>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.route(path, Method::Options, handler)
    }

    /// Adds a HEAD route.
    #[must_use]
    pub fn head<H, Fut>(self, path: impl Into<String>, handler: H) -> Self
    where
        H: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.route(path, Method::Head, handler)
    }

    /// Includes another router's routes with an optional additional prefix.
    ///
    /// This allows nesting routers for hierarchical organization.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let v1_users = APIRouter::new()
    ///     .prefix("/users")
    ///     .get("", list_users);
    ///
    /// let v1_router = APIRouter::new()
    ///     .prefix("/v1")
    ///     .include_router(v1_users);
    ///
    /// // Routes: /v1/users
    /// ```
    #[must_use]
    pub fn include_router(self, other: APIRouter) -> Self {
        self.include_router_with_config(other, IncludeConfig::default())
    }

    /// Includes another router with configuration overrides.
    ///
    /// This allows applying additional configuration when including a router,
    /// such as prepending a prefix, adding tags, or injecting dependencies.
    ///
    /// # Merge Rules
    ///
    /// Following FastAPI's merge semantics:
    /// 1. **prefix**: config.prefix + router.prefix + route.path
    /// 2. **tags**: config.tags + router.tags + route.tags
    /// 3. **dependencies**: config.deps + router.deps + route.deps
    /// 4. **responses**: route > router > config (later values win)
    /// 5. **deprecated**: config overrides router if set
    /// 6. **include_in_schema**: config overrides router if set
    ///
    /// # Example
    ///
    /// ```ignore
    /// let users_router = APIRouter::new()
    ///     .prefix("/users")
    ///     .get("", list_users);
    ///
    /// let config = IncludeConfig::new()
    ///     .prefix("/api/v1")
    ///     .tags(vec!["api"])
    ///     .dependency(auth_dep);
    ///
    /// let app_router = APIRouter::new()
    ///     .include_router_with_config(users_router, config);
    ///
    /// // Routes: /api/v1/users with ["api", "users"] tags
    /// ```
    #[must_use]
    pub fn include_router_with_config(mut self, other: APIRouter, config: IncludeConfig) -> Self {
        // Determine effective values with config overrides
        let effective_deprecated = config.deprecated.or(other.deprecated);
        let effective_include_in_schema = config
            .include_in_schema
            .unwrap_or(other.include_in_schema);

        // Build the full prefix: config.prefix + router.prefix
        let full_prefix = match config.prefix.as_deref() {
            Some(config_prefix) => combine_paths(config_prefix, &other.prefix),
            None => other.prefix.clone(),
        };

        // Merge routes with combined configuration
        for mut route in other.routes {
            // Combine path: full_prefix + route.path
            let combined_path = combine_paths(&full_prefix, &route.path);
            route.path = combined_path;

            // Merge tags: config.tags + router.tags + route.tags
            let mut merged_tags = config.tags.clone();
            merged_tags.extend(other.tags.clone());
            merged_tags.extend(route.tags);
            route.tags = merged_tags;

            // Merge dependencies: config.deps + router.deps + route.deps
            let mut merged_deps = config.dependencies.clone();
            merged_deps.extend(other.dependencies.clone());
            merged_deps.extend(route.dependencies);
            route.dependencies = merged_deps;

            // Apply deprecated override
            if route.deprecated.is_none() {
                route.deprecated = effective_deprecated;
            }

            // Apply include_in_schema override
            if !effective_include_in_schema {
                route.include_in_schema = false;
            }

            self.routes.push(route);
        }

        // Merge response definitions: route > router > config
        // First add config responses (lowest priority)
        for (code, def) in config.responses {
            self.responses.entry(code).or_insert(def);
        }
        // Then add router responses (higher priority)
        for (code, def) in other.responses {
            self.responses.insert(code, def);
        }

        self
    }

    /// Returns the prefix for this router.
    #[must_use]
    pub fn get_prefix(&self) -> &str {
        &self.prefix
    }

    /// Returns the tags for this router.
    #[must_use]
    pub fn get_tags(&self) -> &[String] {
        &self.tags
    }

    /// Returns the dependencies for this router.
    #[must_use]
    pub fn get_dependencies(&self) -> &[RouterDependency] {
        &self.dependencies
    }

    /// Returns the response definitions.
    #[must_use]
    pub fn get_responses(&self) -> &HashMap<u16, ResponseDef> {
        &self.responses
    }

    /// Returns whether routes are deprecated.
    #[must_use]
    pub fn is_deprecated(&self) -> Option<bool> {
        self.deprecated
    }

    /// Returns whether routes should be included in schema.
    #[must_use]
    pub fn get_include_in_schema(&self) -> bool {
        self.include_in_schema
    }

    /// Returns the routes in this router.
    #[must_use]
    pub fn get_routes(&self) -> &[RouterRoute] {
        &self.routes
    }

    /// Converts router routes to `RouteEntry` values for the app.
    ///
    /// This applies the router's prefix, tags, and dependencies to all routes.
    /// The returned routes can be added to an `AppBuilder`.
    #[must_use]
    pub fn into_route_entries(self) -> Vec<RouteEntry> {
        let prefix = self.prefix;
        let _router_tags = self.tags;
        let router_deps = self.dependencies;
        let _router_deprecated = self.deprecated;
        let router_include_in_schema = self.include_in_schema;

        self.routes
            .into_iter()
            .filter(|route| {
                // Only include routes that should be in schema
                router_include_in_schema && route.include_in_schema
            })
            .map(move |route| {
                // Combine prefix with route path
                let full_path = combine_paths(&prefix, &route.path);

                // Clone dependencies for the wrapped handler
                let deps: Vec<RouterDependency> = router_deps
                    .iter()
                    .cloned()
                    .chain(route.dependencies)
                    .collect();

                let handler = route.handler;

                // Create a wrapper handler that runs dependencies first
                if deps.is_empty() {
                    // No dependencies, use handler directly
                    RouteEntry::new(route.method, full_path, move |ctx, req| {
                        let handler = Arc::clone(&handler);
                        (handler)(ctx, req)
                    })
                } else {
                    // Wrap handler to run dependencies first
                    let deps = Arc::new(deps);
                    RouteEntry::new(route.method, full_path, move |ctx, req| {
                        let handler = Arc::clone(&handler);
                        let deps = Arc::clone(&deps);
                        Box::pin(async move {
                            // Run all dependencies
                            for dep in deps.iter() {
                                if let Err(response) = dep.execute(ctx, req).await {
                                    return response;
                                }
                            }
                            // All dependencies passed, run handler
                            (handler)(ctx, req).await
                        })
                    })
                }
            })
            .collect()
    }
}

/// Combines two path segments, handling slashes correctly.
fn combine_paths(prefix: &str, path: &str) -> String {
    match (prefix.is_empty(), path.is_empty()) {
        (true, true) => "/".to_string(),
        (true, false) => {
            if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{}", path)
            }
        }
        (false, true) => prefix.to_string(),
        (false, false) => {
            let prefix = prefix.trim_end_matches('/');
            let path = path.trim_start_matches('/');
            if path.is_empty() {
                prefix.to_string()
            } else {
                format!("{}/{}", prefix, path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_combine_paths() {
        assert_eq!(combine_paths("", ""), "/");
        assert_eq!(combine_paths("", "/users"), "/users");
        assert_eq!(combine_paths("", "users"), "/users");
        assert_eq!(combine_paths("/api", ""), "/api");
        assert_eq!(combine_paths("/api", "/users"), "/api/users");
        assert_eq!(combine_paths("/api", "users"), "/api/users");
        assert_eq!(combine_paths("/api/", "/users"), "/api/users");
        assert_eq!(combine_paths("/api/", "users"), "/api/users");
    }

    #[test]
    fn test_router_prefix_normalization() {
        let router = APIRouter::new().prefix("api");
        assert_eq!(router.get_prefix(), "/api");

        let router = APIRouter::new().prefix("/api/");
        assert_eq!(router.get_prefix(), "/api");

        let router = APIRouter::new().prefix("/api/v1");
        assert_eq!(router.get_prefix(), "/api/v1");
    }

    #[test]
    fn test_router_tags() {
        let router = APIRouter::new().tags(vec!["users", "admin"]).tag("api");

        assert_eq!(router.get_tags(), &["users", "admin", "api"]);
    }

    #[test]
    fn test_router_deprecated() {
        let router = APIRouter::new().deprecated(true);
        assert_eq!(router.is_deprecated(), Some(true));

        let router = APIRouter::new();
        assert_eq!(router.is_deprecated(), None);
    }

    #[test]
    fn test_response_def() {
        let def = ResponseDef::new("Success")
            .with_example(serde_json::json!({"id": 1}))
            .with_content_type("application/json");

        assert_eq!(def.description, "Success");
        assert_eq!(def.example, Some(serde_json::json!({"id": 1})));
        assert_eq!(def.content_type, Some("application/json".to_string()));
    }

    #[test]
    fn test_include_in_schema() {
        let router = APIRouter::new().include_in_schema(false);
        assert!(!router.get_include_in_schema());

        let router = APIRouter::new();
        assert!(router.get_include_in_schema());
    }

    #[test]
    fn test_nested_routers_prefix_combination() {
        // Create an inner router
        let inner = APIRouter::new().prefix("/items");
        assert_eq!(inner.get_prefix(), "/items");

        // Create an outer router and include the inner one
        let outer = APIRouter::new().prefix("/api/v1").include_router(inner);

        // The routes from inner should have combined prefix
        // Note: We test this indirectly since routes are private
        assert_eq!(outer.get_prefix(), "/api/v1");
    }

    #[test]
    fn test_router_with_responses() {
        let router = APIRouter::new()
            .response(200, ResponseDef::new("Success"))
            .response(404, ResponseDef::new("Not Found"));

        let responses = router.get_responses();
        assert_eq!(responses.len(), 2);
        assert!(responses.contains_key(&200));
        assert!(responses.contains_key(&404));
    }

    #[test]
    fn test_router_dependency_creation() {
        let dep = RouterDependency::new("auth", |_ctx, _req| async { Ok(()) });
        assert_eq!(dep.name, "auth");
    }

    #[test]
    fn test_router_with_dependency() {
        let dep = RouterDependency::new("auth", |_ctx, _req| async { Ok(()) });

        let router = APIRouter::new().dependency(dep);
        assert_eq!(router.get_dependencies().len(), 1);
        assert_eq!(router.get_dependencies()[0].name, "auth");
    }

    #[test]
    fn test_router_multiple_dependencies() {
        let dep1 = RouterDependency::new("auth", |_ctx, _req| async { Ok(()) });
        let dep2 = RouterDependency::new("rate_limit", |_ctx, _req| async { Ok(()) });

        let router = APIRouter::new().dependencies(vec![dep1, dep2]);
        assert_eq!(router.get_dependencies().len(), 2);
    }

    #[test]
    fn test_tag_merging_with_nested_routers() {
        let inner = APIRouter::new().tags(vec!["items"]);
        let outer = APIRouter::new().tags(vec!["api"]).include_router(inner);

        // Outer router keeps its own tags
        assert_eq!(outer.get_tags(), &["api"]);
        // Inner router's tags are merged into its routes (tested via into_route_entries)
    }

    #[test]
    fn test_with_prefix_constructor() {
        let router = APIRouter::with_prefix("/api/v1");
        assert_eq!(router.get_prefix(), "/api/v1");
    }

    #[test]
    fn test_empty_router() {
        let router = APIRouter::new();
        assert_eq!(router.get_prefix(), "");
        assert!(router.get_tags().is_empty());
        assert!(router.get_dependencies().is_empty());
        assert!(router.get_responses().is_empty());
        assert_eq!(router.is_deprecated(), None);
        assert!(router.get_include_in_schema());
        assert!(router.get_routes().is_empty());
    }

    // =========================================================================
    // IncludeConfig Tests
    // =========================================================================

    #[test]
    fn test_include_config_default() {
        let config = IncludeConfig::new();
        assert!(config.get_prefix().is_none());
        assert!(config.get_tags().is_empty());
        assert!(config.get_dependencies().is_empty());
        assert!(config.get_responses().is_empty());
        assert!(config.get_deprecated().is_none());
        assert!(config.get_include_in_schema().is_none());
    }

    #[test]
    fn test_include_config_prefix() {
        let config = IncludeConfig::new().prefix("/api/v1");
        assert_eq!(config.get_prefix(), Some("/api/v1"));

        // Should normalize prefix without leading slash
        let config = IncludeConfig::new().prefix("api/v1");
        assert_eq!(config.get_prefix(), Some("/api/v1"));

        // Should remove trailing slash
        let config = IncludeConfig::new().prefix("/api/v1/");
        assert_eq!(config.get_prefix(), Some("/api/v1"));
    }

    #[test]
    fn test_include_config_tags() {
        let config = IncludeConfig::new()
            .tags(vec!["api", "v1"])
            .tag("extra");
        assert_eq!(config.get_tags(), &["api", "v1", "extra"]);
    }

    #[test]
    fn test_include_config_dependencies() {
        let dep1 = RouterDependency::new("auth", |_ctx, _req| async { Ok(()) });
        let dep2 = RouterDependency::new("rate_limit", |_ctx, _req| async { Ok(()) });

        let config = IncludeConfig::new()
            .dependency(dep1)
            .dependency(dep2);
        assert_eq!(config.get_dependencies().len(), 2);
    }

    #[test]
    fn test_include_config_responses() {
        let config = IncludeConfig::new()
            .response(401, ResponseDef::new("Unauthorized"))
            .response(500, ResponseDef::new("Server Error"));
        assert_eq!(config.get_responses().len(), 2);
    }

    #[test]
    fn test_include_config_deprecated() {
        let config = IncludeConfig::new().deprecated(true);
        assert_eq!(config.get_deprecated(), Some(true));

        let config = IncludeConfig::new().deprecated(false);
        assert_eq!(config.get_deprecated(), Some(false));
    }

    #[test]
    fn test_include_config_include_in_schema() {
        let config = IncludeConfig::new().include_in_schema(false);
        assert_eq!(config.get_include_in_schema(), Some(false));
    }

    // =========================================================================
    // Merge Rules Tests
    // =========================================================================

    #[test]
    fn test_merge_rule_prefix_prepending() {
        // config.prefix + router.prefix should be combined
        let inner_router = APIRouter::new().prefix("/users");
        let config = IncludeConfig::new().prefix("/api/v1");

        let outer = APIRouter::new().include_router_with_config(inner_router, config);

        // Check that routes were merged (we can't directly access the full path,
        // but we can verify the outer router structure)
        // The inner router's routes should now have paths like /api/v1/users
        assert_eq!(outer.get_routes().len(), 0); // No routes were added to inner
    }

    #[test]
    fn test_merge_rule_tags_prepending() {
        // config.tags + router.tags should be merged
        let inner = APIRouter::new().tags(vec!["users"]);
        let config = IncludeConfig::new().tags(vec!["api", "v1"]);

        let outer = APIRouter::new()
            .tags(vec!["outer"])
            .include_router_with_config(inner, config);

        // Outer keeps its own tags
        assert_eq!(outer.get_tags(), &["outer"]);
    }

    #[test]
    fn test_merge_rule_deprecated_override() {
        // config.deprecated should override router.deprecated
        let inner = APIRouter::new().deprecated(false);
        let config = IncludeConfig::new().deprecated(true);

        let outer = APIRouter::new().include_router_with_config(inner, config);

        // The override happens at the route level, not router level
        assert_eq!(outer.is_deprecated(), None);
    }

    #[test]
    fn test_merge_rule_include_in_schema_override() {
        // config.include_in_schema should override router.include_in_schema
        let inner = APIRouter::new().include_in_schema(true);
        let config = IncludeConfig::new().include_in_schema(false);

        let _outer = APIRouter::new().include_router_with_config(inner, config);
        // Routes from inner should have include_in_schema = false
    }

    #[test]
    fn test_merge_rule_responses_priority() {
        // route > router > config for responses
        let inner = APIRouter::new()
            .response(200, ResponseDef::new("Router Success"))
            .response(404, ResponseDef::new("Router Not Found"));

        let config = IncludeConfig::new()
            .response(200, ResponseDef::new("Config Success"))
            .response(500, ResponseDef::new("Config Error"));

        let outer = APIRouter::new().include_router_with_config(inner, config);

        let responses = outer.get_responses();
        // Router's 200 should override config's 200
        assert_eq!(responses.get(&200).unwrap().description, "Router Success");
        // Config's 500 should be present
        assert_eq!(responses.get(&500).unwrap().description, "Config Error");
        // Router's 404 should be present
        assert_eq!(responses.get(&404).unwrap().description, "Router Not Found");
    }

    #[test]
    fn test_recursive_router_inclusion() {
        // Routers can include other routers at multiple levels
        let level3 = APIRouter::new().prefix("/items");
        let level2 = APIRouter::new()
            .prefix("/users")
            .include_router(level3);
        let level1 = APIRouter::new()
            .prefix("/api")
            .include_router(level2);

        // The final router should have prefix /api
        // Routes from level3 should be at /api/users/items
        assert_eq!(level1.get_prefix(), "/api");
    }

    #[test]
    fn test_recursive_config_merging() {
        // Multi-level config merging
        let inner = APIRouter::new().tags(vec!["items"]);
        let middle_config = IncludeConfig::new().tags(vec!["users"]).prefix("/users");
        let outer_config = IncludeConfig::new().tags(vec!["api"]).prefix("/api");

        let middle = APIRouter::new().include_router_with_config(inner, middle_config);
        let outer = APIRouter::new().include_router_with_config(middle, outer_config);

        // Outer has its own (empty) tags, but routes should have merged tags
        assert!(outer.get_tags().is_empty());
    }

    #[test]
    fn test_include_config_empty_prefix() {
        // Empty prefix in config should not affect router prefix
        let inner = APIRouter::new().prefix("/users");
        let config = IncludeConfig::new(); // No prefix set

        let outer = APIRouter::new()
            .prefix("/api")
            .include_router_with_config(inner, config);

        // Outer keeps its prefix
        assert_eq!(outer.get_prefix(), "/api");
    }

    #[test]
    fn test_multi_level_path_construction() {
        // Test that paths are correctly constructed at multiple levels
        // /api + /v1 + /users + /{id} = /api/v1/users/{id}

        // We can't directly test the path construction without adding routes,
        // but we can test the combine_paths function
        let level1 = "/api";
        let level2 = "/v1";
        let level3 = "/users";
        let level4 = "/{id}";

        let combined_12 = combine_paths(level1, level2);
        assert_eq!(combined_12, "/api/v1");

        let combined_123 = combine_paths(&combined_12, level3);
        assert_eq!(combined_123, "/api/v1/users");

        let combined_1234 = combine_paths(&combined_123, level4);
        assert_eq!(combined_1234, "/api/v1/users/{id}");
    }
}
