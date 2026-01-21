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
    pub fn include_router(mut self, other: APIRouter) -> Self {
        // Merge routes with combined prefix
        for mut route in other.routes {
            // Combine path with other router's prefix
            let combined_path = combine_paths(&other.prefix, &route.path);
            route.path = combined_path;

            // Merge tags (other router's tags + route's tags)
            let mut merged_tags = other.tags.clone();
            merged_tags.extend(route.tags);
            route.tags = merged_tags;

            // Prepend other router's dependencies
            let mut merged_deps = other.dependencies.clone();
            merged_deps.extend(route.dependencies);
            route.dependencies = merged_deps;

            // Inherit deprecated if not set
            if route.deprecated.is_none() {
                route.deprecated = other.deprecated;
            }

            // Inherit include_in_schema
            if !other.include_in_schema {
                route.include_in_schema = false;
            }

            self.routes.push(route);
        }

        // Merge response definitions
        for (code, def) in other.responses {
            self.responses.entry(code).or_insert(def);
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
        let router_tags = self.tags;
        let router_deps = self.dependencies;
        let router_deprecated = self.deprecated;
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
                    .chain(route.dependencies.into_iter())
                    .collect();

                let handler = route.handler;

                // Create a wrapper handler that runs dependencies first
                if deps.is_empty() {
                    // No dependencies, use handler directly
                    RouteEntry::new(route.method, full_path, move |ctx, req| {
                        let handler = Arc::clone(&handler);
                        async move { (handler)(ctx, req).await }
                    })
                } else {
                    // Wrap handler to run dependencies first
                    let deps = Arc::new(deps);
                    RouteEntry::new(route.method, full_path, move |ctx, req| {
                        let handler = Arc::clone(&handler);
                        let deps = Arc::clone(&deps);
                        async move {
                            // Run all dependencies
                            for dep in deps.iter() {
                                if let Err(response) = dep.execute(ctx, req).await {
                                    return response;
                                }
                            }
                            // All dependencies passed, run handler
                            (handler)(ctx, req).await
                        }
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
        let router = APIRouter::new()
            .tags(vec!["users", "admin"])
            .tag("api");

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
}
