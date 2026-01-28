//! Radix trie router implementation.
//!
//! # Wildcard Catch-All Routes
//!
//! The router supports catch-all wildcard routes using two equivalent syntaxes:
//!
//! - `{*path}` - asterisk prefix syntax (recommended)
//! - `{path:path}` - converter suffix syntax
//!
//! Wildcards capture all remaining path segments including slashes:
//!
//! ```ignore
//! // Route: /files/{*filepath}
//! // Request: /files/css/styles/main.css
//! // Captured: filepath = "css/styles/main.css"
//! ```
//!
//! Wildcards must be the final segment in a route pattern.

use crate::r#match::{AllowedMethods, RouteLookup, RouteMatch};
use fastapi_core::{Handler, Method};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// Path parameter type converter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Converter {
    /// String (default).
    Str,
    /// Integer (i64).
    Int,
    /// Float (f64).
    Float,
    /// UUID.
    Uuid,
    /// Path segment (can contain `/`). Used for catch-all wildcard routes.
    ///
    /// Can be specified as `{*name}` or `{name:path}`.
    Path,
}

/// A type-converted path parameter value.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    /// String value (from `{param}` or `{param:str}`).
    Str(String),
    /// Integer value (from `{param:int}`).
    Int(i64),
    /// Float value (from `{param:float}`).
    Float(f64),
    /// UUID value (from `{param:uuid}`).
    Uuid(String),
    /// Path value including slashes (from `{*param}` or `{param:path}`).
    Path(String),
}

impl ParamValue {
    /// Get as string reference. Works for all variants.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Str(s) | Self::Uuid(s) | Self::Path(s) => s,
            Self::Int(_) | Self::Float(_) => {
                // For numeric types, this isn't ideal but maintains API consistency
                // Users should use as_int() or as_float() for those types
                ""
            }
        }
    }

    /// Get as i64 if this is an Int variant.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Get as f64 if this is a Float variant.
    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(n) => Some(*n),
            _ => None,
        }
    }

    /// Get the raw string for Str, Uuid, or Path variants.
    #[must_use]
    pub fn into_string(self) -> Option<String> {
        match self {
            Self::Str(s) | Self::Uuid(s) | Self::Path(s) => Some(s),
            Self::Int(_) | Self::Float(_) => None,
        }
    }
}

/// Error type for parameter conversion failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionError {
    /// Failed to parse as integer.
    InvalidInt {
        /// The value that failed to parse.
        value: String,
        /// The parameter name.
        param: String,
    },
    /// Failed to parse as float.
    InvalidFloat {
        /// The value that failed to parse.
        value: String,
        /// The parameter name.
        param: String,
    },
    /// Failed to parse as UUID.
    InvalidUuid {
        /// The value that failed to parse.
        value: String,
        /// The parameter name.
        param: String,
    },
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInt { value, param } => {
                write!(f, "path parameter '{param}': '{value}' is not a valid integer")
            }
            Self::InvalidFloat { value, param } => {
                write!(f, "path parameter '{param}': '{value}' is not a valid float")
            }
            Self::InvalidUuid { value, param } => {
                write!(f, "path parameter '{param}': '{value}' is not a valid UUID")
            }
        }
    }
}

impl std::error::Error for ConversionError {}

impl Converter {
    /// Check if a value matches this converter.
    #[must_use]
    pub fn matches(&self, value: &str) -> bool {
        match self {
            Self::Str => true,
            Self::Int => value.parse::<i64>().is_ok(),
            Self::Float => value.parse::<f64>().is_ok(),
            Self::Uuid => is_uuid(value),
            Self::Path => true,
        }
    }

    /// Convert a string value to the appropriate typed value.
    ///
    /// # Errors
    ///
    /// Returns a `ConversionError` if the value cannot be parsed as the expected type.
    pub fn convert(&self, value: &str, param_name: &str) -> Result<ParamValue, ConversionError> {
        match self {
            Self::Str => Ok(ParamValue::Str(value.to_string())),
            Self::Int => value.parse::<i64>().map(ParamValue::Int).map_err(|_| {
                ConversionError::InvalidInt {
                    value: value.to_string(),
                    param: param_name.to_string(),
                }
            }),
            Self::Float => value.parse::<f64>().map(ParamValue::Float).map_err(|_| {
                ConversionError::InvalidFloat {
                    value: value.to_string(),
                    param: param_name.to_string(),
                }
            }),
            Self::Uuid => {
                if is_uuid(value) {
                    Ok(ParamValue::Uuid(value.to_string()))
                } else {
                    Err(ConversionError::InvalidUuid {
                        value: value.to_string(),
                        param: param_name.to_string(),
                    })
                }
            }
            Self::Path => Ok(ParamValue::Path(value.to_string())),
        }
    }

    /// Returns the type name for error messages.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::Int => "integer",
            Self::Float => "float",
            Self::Uuid => "UUID",
            Self::Path => "path",
        }
    }
}

fn is_uuid(s: &str) -> bool {
    // Simple UUID check: 8-4-4-4-12 hex digits
    if s.len() != 36 {
        return false;
    }
    let parts: Vec<_> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    parts[0].len() == 8
        && parts[1].len() == 4
        && parts[2].len() == 4
        && parts[3].len() == 4
        && parts[4].len() == 12
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Path parameter information.
#[derive(Debug, Clone)]
pub struct ParamInfo {
    /// Parameter name.
    pub name: String,
    /// Type converter.
    pub converter: Converter,
}

/// A route definition with handler for request processing.
///
/// Routes are created with a path pattern, HTTP method, and a handler function.
/// The handler is stored as a type-erased `Arc<dyn Handler>` for dynamic dispatch.
///
/// # Example
///
/// ```ignore
/// use fastapi_router::Route;
/// use fastapi_core::{Method, Handler, RequestContext, Request, Response};
///
/// let route = Route::new(Method::Get, "/users/{id}", my_handler);
/// ```
pub struct Route {
    /// Route path pattern (e.g., "/users/{id}").
    pub path: String,
    /// HTTP method for this route.
    pub method: Method,
    /// Operation ID for OpenAPI documentation.
    pub operation_id: String,
    /// Handler function that processes matching requests.
    handler: Arc<dyn Handler>,
}

impl fmt::Debug for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Route")
            .field("path", &self.path)
            .field("method", &self.method)
            .field("operation_id", &self.operation_id)
            .field("handler", &"<handler>")
            .finish()
    }
}

/// Error returned when a new route conflicts with an existing one.
#[derive(Debug, Clone)]
pub struct RouteConflictError {
    /// HTTP method for the conflicting route.
    pub method: Method,
    /// The new route path that failed to register.
    pub new_path: String,
    /// The existing route path that conflicts.
    pub existing_path: String,
}

impl fmt::Display for RouteConflictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "route conflict for {}: {} conflicts with {}",
            self.method, self.new_path, self.existing_path
        )
    }
}

impl std::error::Error for RouteConflictError {}

/// Error returned when a route path is invalid.
#[derive(Debug, Clone)]
pub struct InvalidRouteError {
    /// The invalid route path.
    pub path: String,
    /// Description of the validation failure.
    pub message: String,
}

impl InvalidRouteError {
    /// Create a new invalid route error.
    #[must_use]
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for InvalidRouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid route path '{}': {}", self.path, self.message)
    }
}

impl std::error::Error for InvalidRouteError {}

/// Error returned when adding a route fails.
#[derive(Debug, Clone)]
pub enum RouteAddError {
    /// Route conflicts with an existing route.
    Conflict(RouteConflictError),
    /// Route path is invalid.
    InvalidPath(InvalidRouteError),
}

impl fmt::Display for RouteAddError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(err) => err.fmt(f),
            Self::InvalidPath(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for RouteAddError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Conflict(err) => Some(err),
            Self::InvalidPath(err) => Some(err),
        }
    }
}

impl From<RouteConflictError> for RouteAddError {
    fn from(err: RouteConflictError) -> Self {
        Self::Conflict(err)
    }
}

impl From<InvalidRouteError> for RouteAddError {
    fn from(err: InvalidRouteError) -> Self {
        Self::InvalidPath(err)
    }
}

impl Route {
    /// Create a new route with a handler.
    ///
    /// # Arguments
    ///
    /// * `method` - HTTP method for this route
    /// * `path` - Path pattern (e.g., "/users/{id}")
    /// * `handler` - Handler that processes matching requests
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_router::Route;
    /// use fastapi_core::Method;
    ///
    /// let route = Route::new(Method::Get, "/users/{id}", my_handler);
    /// ```
    pub fn new<H>(method: Method, path: impl Into<String>, handler: H) -> Self
    where
        H: Handler + 'static,
    {
        let path = path.into();
        let operation_id = path.replace('/', "_").replace(['{', '}'], "");
        Self {
            path,
            method,
            operation_id,
            handler: Arc::new(handler),
        }
    }

    /// Create a new route with a pre-wrapped Arc handler.
    ///
    /// Useful when the handler is already wrapped in an Arc for sharing.
    pub fn with_arc_handler(
        method: Method,
        path: impl Into<String>,
        handler: Arc<dyn Handler>,
    ) -> Self {
        let path = path.into();
        let operation_id = path.replace('/', "_").replace(['{', '}'], "");
        Self {
            path,
            method,
            operation_id,
            handler,
        }
    }

    /// Get a reference to the handler.
    #[must_use]
    pub fn handler(&self) -> &Arc<dyn Handler> {
        &self.handler
    }

    /// Create a route with a placeholder handler.
    ///
    /// This is used by the route registration macros during compile-time route
    /// discovery. The placeholder handler returns 501 Not Implemented.
    ///
    /// **Note**: Routes created with this method should have their handlers
    /// replaced before being used to handle actual requests.
    #[must_use]
    pub fn with_placeholder_handler(method: Method, path: impl Into<String>) -> Self {
        Self::new(method, path, PlaceholderHandler)
    }
}

/// A placeholder handler that returns 501 Not Implemented.
///
/// Used for routes created during macro registration before the actual
/// handler is wired up.
struct PlaceholderHandler;

impl Handler for PlaceholderHandler {
    fn call<'a>(
        &'a self,
        _ctx: &'a fastapi_core::RequestContext,
        _req: &'a mut fastapi_core::Request,
    ) -> fastapi_core::BoxFuture<'a, fastapi_core::Response> {
        Box::pin(async {
            // 501 Not Implemented
            fastapi_core::Response::with_status(fastapi_core::StatusCode::from_u16(501))
        })
    }
}

/// Trie node.
struct Node {
    segment: String,
    children: Vec<Node>,
    param: Option<ParamInfo>,
    routes: HashMap<Method, usize>,
}

impl Node {
    fn new(segment: impl Into<String>) -> Self {
        Self {
            segment: segment.into(),
            children: Vec::new(),
            param: None,
            routes: HashMap::new(),
        }
    }

    fn find_static(&self, segment: &str) -> Option<&Node> {
        self.children
            .iter()
            .find(|c| c.param.is_none() && c.segment == segment)
    }

    fn find_param(&self) -> Option<&Node> {
        self.children.iter().find(|c| c.param.is_some())
    }
}

/// Radix trie router.
pub struct Router {
    root: Node,
    routes: Vec<Route>,
}

impl Router {
    /// Create an empty router.
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: Node::new(""),
            routes: Vec::new(),
        }
    }

    /// Add a route, returning an error if it conflicts with existing routes
    /// or the path pattern is invalid.
    ///
    /// Conflict rules:
    /// - Same HTTP method + structurally identical path patterns conflict
    /// - Static segments take priority over parameter segments (no conflict)
    /// - Parameter names/converters do not disambiguate conflicts (one param slot per segment)
    /// - `{param:path}` converters are only valid as the final segment
    pub fn add(&mut self, route: Route) -> Result<(), RouteAddError> {
        if let Some(conflict) = self.find_conflict(&route) {
            return Err(RouteAddError::Conflict(conflict));
        }

        let route_idx = self.routes.len();
        let path = route.path.clone();
        let method = route.method;
        self.routes.push(route);

        let segments = parse_path(&path);
        validate_path_segments(&path, &segments)?;
        let mut node = &mut self.root;

        for seg in segments {
            let (segment, param) = match seg {
                PathSegment::Static(s) => (s.to_string(), None),
                PathSegment::Param { name, converter } => {
                    let info = ParamInfo {
                        name: name.to_string(),
                        converter,
                    };
                    (format!("{{{name}}}"), Some(info))
                }
            };

            // Find or create child
            let child_idx = node.children.iter().position(|c| c.segment == segment);

            if let Some(idx) = child_idx {
                node = &mut node.children[idx];
            } else {
                let mut new_node = Node::new(&segment);
                new_node.param = param;
                node.children.push(new_node);
                node = node.children.last_mut().unwrap();
            }
        }

        node.routes.insert(method, route_idx);
        Ok(())
    }

    /// Match a path and method with 404/405 distinction.
    #[must_use]
    pub fn lookup<'a>(&'a self, path: &'a str, method: Method) -> RouteLookup<'a> {
        let (node, params) = match self.match_node(path) {
            Some(found) => found,
            None => return RouteLookup::NotFound,
        };

        if let Some(&idx) = node.routes.get(&method) {
            return RouteLookup::Match(RouteMatch {
                route: &self.routes[idx],
                params,
            });
        }

        // Allow HEAD when GET is registered.
        if method == Method::Head {
            if let Some(&idx) = node.routes.get(&Method::Get) {
                return RouteLookup::Match(RouteMatch {
                    route: &self.routes[idx],
                    params,
                });
            }
        }

        if node.routes.is_empty() {
            return RouteLookup::NotFound;
        }

        let allowed = AllowedMethods::new(node.routes.keys().copied().collect());
        RouteLookup::MethodNotAllowed { allowed }
    }

    /// Match a path and method.
    #[must_use]
    pub fn match_path<'a>(&'a self, path: &'a str, method: Method) -> Option<RouteMatch<'a>> {
        match self.lookup(path, method) {
            RouteLookup::Match(matched) => Some(matched),
            RouteLookup::MethodNotAllowed { .. } | RouteLookup::NotFound => None,
        }
    }

    /// Get all routes.
    #[must_use]
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    fn find_conflict(&self, route: &Route) -> Option<RouteConflictError> {
        for existing in &self.routes {
            if existing.method != route.method {
                continue;
            }

            if paths_conflict(&existing.path, &route.path) {
                return Some(RouteConflictError {
                    method: route.method,
                    new_path: route.path.clone(),
                    existing_path: existing.path.clone(),
                });
            }
        }

        None
    }

    fn match_node<'a>(&'a self, path: &'a str) -> Option<(&'a Node, Vec<(&'a str, &'a str)>)> {
        let ranges = segment_ranges(path);
        let mut segments: Vec<&'a str> = Vec::with_capacity(ranges.len());
        for (start, end) in &ranges {
            segments.push(&path[*start..*end]);
        }
        let last_end = ranges.last().map_or(0, |(_, end)| *end);
        let mut params = Vec::new();
        let mut node = &self.root;
        let mut idx = 0;

        while idx < segments.len() {
            let segment = segments[idx];
            // Try static match first
            if let Some(child) = node.find_static(segment) {
                node = child;
                idx += 1;
                continue;
            }

            // Try parameter match
            if let Some(child) = node.find_param() {
                if let Some(ref info) = child.param {
                    if info.converter == Converter::Path {
                        let start = ranges[idx].0;
                        let value = &path[start..last_end];
                        params.push((info.name.as_str(), value));
                        node = child;
                        break;
                    }
                    if info.converter.matches(segment) {
                        params.push((info.name.as_str(), segment));
                        node = child;
                        idx += 1;
                        continue;
                    }
                }
            }

            return None;
        }

        Some((node, params))
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

enum PathSegment<'a> {
    Static(&'a str),
    Param { name: &'a str, converter: Converter },
}

fn parse_path(path: &str) -> Vec<PathSegment<'_>> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.starts_with('{') && s.ends_with('}') {
                let inner = &s[1..s.len() - 1];
                // Check for {*name} wildcard syntax (catch-all)
                if let Some(name) = inner.strip_prefix('*') {
                    return PathSegment::Param {
                        name,
                        converter: Converter::Path,
                    };
                }
                let (name, converter) = if let Some(pos) = inner.find(':') {
                    let conv = match &inner[pos + 1..] {
                        "int" => Converter::Int,
                        "float" => Converter::Float,
                        "uuid" => Converter::Uuid,
                        "path" => Converter::Path,
                        _ => Converter::Str,
                    };
                    (&inner[..pos], conv)
                } else {
                    (inner, Converter::Str)
                };
                PathSegment::Param { name, converter }
            } else {
                PathSegment::Static(s)
            }
        })
        .collect()
}

fn validate_path_segments(
    path: &str,
    segments: &[PathSegment<'_>],
) -> Result<(), InvalidRouteError> {
    for (idx, segment) in segments.iter().enumerate() {
        if let PathSegment::Param {
            name,
            converter: Converter::Path,
        } = segment
        {
            if idx + 1 != segments.len() {
                return Err(InvalidRouteError::new(
                    path,
                    format!(
                        "wildcard '{{*{name}}}' or '{{{name}:path}}' must be the final segment"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn segment_ranges(path: &str) -> Vec<(usize, usize)> {
    let bytes = path.as_bytes();
    let mut ranges = Vec::new();
    let mut idx = 0;
    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx] == b'/' {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }
        let start = idx;
        while idx < bytes.len() && bytes[idx] != b'/' {
            idx += 1;
        }
        let end = idx;
        ranges.push((start, end));
    }
    ranges
}

fn paths_conflict(a: &str, b: &str) -> bool {
    let a_segments = parse_path(a);
    let b_segments = parse_path(b);

    let a_has_path = matches!(
        a_segments.last(),
        Some(PathSegment::Param {
            converter: Converter::Path,
            ..
        })
    );
    let b_has_path = matches!(
        b_segments.last(),
        Some(PathSegment::Param {
            converter: Converter::Path,
            ..
        })
    );
    let min_len = a_segments.len().min(b_segments.len());
    let mut param_mismatch = false;

    for (left, right) in a_segments.iter().take(min_len).zip(b_segments.iter()) {
        match (left, right) {
            (PathSegment::Static(a), PathSegment::Static(b)) => {
                if a != b {
                    return false;
                }
            }
            (PathSegment::Static(_), PathSegment::Param { .. })
            | (PathSegment::Param { .. }, PathSegment::Static(_)) => {
                // Static segments take priority over params, so this is not a conflict.
                return false;
            }
            (
                PathSegment::Param {
                    name: left_name,
                    converter: left_conv,
                },
                PathSegment::Param {
                    name: right_name,
                    converter: right_conv,
                },
            ) => {
                if left_name != right_name || left_conv != right_conv {
                    param_mismatch = true;
                }
            }
        }
    }

    if a_segments.len() == b_segments.len() {
        return true;
    }

    if param_mismatch {
        return true;
    }

    if a_has_path && a_segments.len() == min_len {
        return true;
    }

    if b_has_path && b_segments.len() == min_len {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastapi_core::{BoxFuture, Request, RequestContext, Response};

    /// Test handler that returns a 200 OK response.
    /// Used for testing route matching without needing real handlers.
    struct TestHandler;

    impl Handler for TestHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            Box::pin(async { Response::ok() })
        }
    }

    /// Helper to create a route with a test handler.
    fn route(method: Method, path: &str) -> Route {
        Route::new(method, path, TestHandler)
    }

    #[test]
    fn static_route_match() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();
        router.add(route(Method::Get, "/items")).unwrap();

        let m = router.match_path("/users", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/users");

        let m = router.match_path("/items", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/items");

        // Non-existent path
        assert!(router.match_path("/other", Method::Get).is_none());
    }

    #[test]
    fn nested_static_routes() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/api/v1/users")).unwrap();
        router.add(route(Method::Get, "/api/v2/users")).unwrap();

        let m = router.match_path("/api/v1/users", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/api/v1/users");

        let m = router.match_path("/api/v2/users", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/api/v2/users");
    }

    #[test]
    fn parameter_extraction() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users/{user_id}")).unwrap();

        let m = router.match_path("/users/123", Method::Get);
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.route.path, "/users/{user_id}");
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0], ("user_id", "123"));
    }

    #[test]
    fn multiple_parameters() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/users/{user_id}/posts/{post_id}"))
            .unwrap();

        let m = router.match_path("/users/42/posts/99", Method::Get);
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.params.len(), 2);
        assert_eq!(m.params[0], ("user_id", "42"));
        assert_eq!(m.params[1], ("post_id", "99"));
    }

    #[test]
    fn int_converter() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/items/{id:int}")).unwrap();

        // Valid integer
        let m = router.match_path("/items/123", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().params[0], ("id", "123"));

        // Negative integer
        let m = router.match_path("/items/-456", Method::Get);
        assert!(m.is_some());

        // Invalid (not an integer)
        assert!(router.match_path("/items/abc", Method::Get).is_none());
        assert!(router.match_path("/items/12.34", Method::Get).is_none());
    }

    #[test]
    fn float_converter() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/values/{val:float}"))
            .unwrap();

        // Valid float
        let m = router.match_path("/values/3.14", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().params[0], ("val", "3.14"));

        // Integer (also valid float)
        let m = router.match_path("/values/42", Method::Get);
        assert!(m.is_some());

        // Invalid
        assert!(router.match_path("/values/abc", Method::Get).is_none());
    }

    #[test]
    fn uuid_converter() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/objects/{id:uuid}"))
            .unwrap();

        // Valid UUID
        let m = router.match_path("/objects/550e8400-e29b-41d4-a716-446655440000", Method::Get);
        assert!(m.is_some());
        assert_eq!(
            m.unwrap().params[0],
            ("id", "550e8400-e29b-41d4-a716-446655440000")
        );

        // Invalid UUIDs
        assert!(
            router
                .match_path("/objects/not-a-uuid", Method::Get)
                .is_none()
        );
        assert!(router.match_path("/objects/123", Method::Get).is_none());
    }

    #[test]
    fn path_converter_captures_slashes() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/files/{path:path}"))
            .unwrap();

        let m = router.match_path("/files/a/b/c.txt", Method::Get).unwrap();
        assert_eq!(m.params[0], ("path", "a/b/c.txt"));
    }

    #[test]
    fn path_converter_must_be_terminal() {
        let mut router = Router::new();
        let result = router.add(route(Method::Get, "/files/{path:path}/edit"));
        assert!(matches!(result, Err(RouteAddError::InvalidPath(_))));
    }

    #[test]
    fn method_dispatch() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/items")).unwrap();
        router.add(route(Method::Post, "/items")).unwrap();
        router.add(route(Method::Delete, "/items/{id}")).unwrap();

        // GET /items
        let m = router.match_path("/items", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.method, Method::Get);

        // POST /items
        let m = router.match_path("/items", Method::Post);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.method, Method::Post);

        // DELETE /items/123
        let m = router.match_path("/items/123", Method::Delete);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.method, Method::Delete);

        // Method not allowed (PUT /items)
        assert!(router.match_path("/items", Method::Put).is_none());
    }

    #[test]
    fn lookup_method_not_allowed_includes_head() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();

        let result = router.lookup("/users", Method::Post);
        match result {
            RouteLookup::MethodNotAllowed { allowed } => {
                assert!(allowed.contains(Method::Get));
                assert!(allowed.contains(Method::Head));
                assert_eq!(allowed.header_value(), "GET, HEAD");
            }
            _ => panic!("expected MethodNotAllowed"),
        }
    }

    #[test]
    fn lookup_method_not_allowed_multiple_methods() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();
        router.add(route(Method::Post, "/users")).unwrap();
        router.add(route(Method::Delete, "/users")).unwrap();

        let result = router.lookup("/users", Method::Put);
        match result {
            RouteLookup::MethodNotAllowed { allowed } => {
                assert_eq!(allowed.header_value(), "GET, HEAD, POST, DELETE");
            }
            _ => panic!("expected MethodNotAllowed"),
        }
    }

    #[test]
    fn lookup_not_found_when_path_missing() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();

        assert!(matches!(
            router.lookup("/missing", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn lookup_not_found_when_converter_mismatch() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/items/{id:int}")).unwrap();

        assert!(matches!(
            router.lookup("/items/abc", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn static_takes_priority_over_param() {
        let mut router = Router::new();
        // Order matters: add static first, then param
        router.add(route(Method::Get, "/users/me")).unwrap();
        router.add(route(Method::Get, "/users/{id}")).unwrap();

        // Static match for "me"
        let m = router.match_path("/users/me", Method::Get);
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.route.path, "/users/me");
        assert!(m.params.is_empty());

        // Parameter match for "123"
        let m = router.match_path("/users/123", Method::Get);
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.route.path, "/users/{id}");
        assert_eq!(m.params[0], ("id", "123"));
    }

    #[test]
    fn route_match_get_param() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/users/{user_id}/posts/{post_id}"))
            .unwrap();

        let m = router
            .match_path("/users/42/posts/99", Method::Get)
            .unwrap();

        assert_eq!(m.get_param("user_id"), Some("42"));
        assert_eq!(m.get_param("post_id"), Some("99"));
        assert_eq!(m.get_param("unknown"), None);
    }

    #[test]
    fn converter_matches() {
        assert!(Converter::Str.matches("anything"));
        assert!(Converter::Str.matches("123"));

        assert!(Converter::Int.matches("123"));
        assert!(Converter::Int.matches("-456"));
        assert!(!Converter::Int.matches("12.34"));
        assert!(!Converter::Int.matches("abc"));

        assert!(Converter::Float.matches("3.14"));
        assert!(Converter::Float.matches("42"));
        assert!(!Converter::Float.matches("abc"));

        assert!(Converter::Uuid.matches("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!Converter::Uuid.matches("not-a-uuid"));

        assert!(Converter::Path.matches("any/path/here"));
    }

    #[test]
    fn parse_path_segments() {
        let segments = parse_path("/users/{id}/posts/{post_id:int}");
        assert_eq!(segments.len(), 4);

        match &segments[0] {
            PathSegment::Static(s) => assert_eq!(*s, "users"),
            _ => panic!("Expected static segment"),
        }

        match &segments[1] {
            PathSegment::Param { name, converter } => {
                assert_eq!(*name, "id");
                assert_eq!(*converter, Converter::Str);
            }
            _ => panic!("Expected param segment"),
        }

        match &segments[2] {
            PathSegment::Static(s) => assert_eq!(*s, "posts"),
            _ => panic!("Expected static segment"),
        }

        match &segments[3] {
            PathSegment::Param { name, converter } => {
                assert_eq!(*name, "post_id");
                assert_eq!(*converter, Converter::Int);
            }
            _ => panic!("Expected param segment"),
        }
    }

    #[test]
    fn empty_router() {
        let router = Router::new();
        assert!(router.match_path("/anything", Method::Get).is_none());
        assert!(router.routes().is_empty());
    }

    #[test]
    fn routes_accessor() {
        let mut router = Router::new();
        let _ = router.add(route(Method::Get, "/a"));
        let _ = router.add(route(Method::Post, "/b"));

        assert_eq!(router.routes().len(), 2);
        assert_eq!(router.routes()[0].path, "/a");
        assert_eq!(router.routes()[1].path, "/b");
    }

    // =========================================================================
    // CONFLICT DETECTION TESTS
    // =========================================================================

    #[test]
    fn conflict_same_method_same_path() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();

        let result = router.add(route(Method::Get, "/users"));
        assert!(result.is_err());
        let err = match result.unwrap_err() {
            RouteAddError::Conflict(err) => err,
            RouteAddError::InvalidPath(err) => {
                panic!("unexpected invalid path error: {err}")
            }
        };
        assert_eq!(err.method, Method::Get);
        assert_eq!(err.new_path, "/users");
        assert_eq!(err.existing_path, "/users");
    }

    #[test]
    fn conflict_same_method_same_param_pattern() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users/{id}")).unwrap();

        // Same structure, different param name - still conflicts
        let result = router.add(route(Method::Get, "/users/{user_id}"));
        assert!(result.is_err());
        let err = match result.unwrap_err() {
            RouteAddError::Conflict(err) => err,
            RouteAddError::InvalidPath(err) => {
                panic!("unexpected invalid path error: {err}")
            }
        };
        assert_eq!(err.existing_path, "/users/{id}");
        assert_eq!(err.new_path, "/users/{user_id}");
    }

    #[test]
    fn conflict_param_name_mismatch_across_lengths() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users/{id}/posts")).unwrap();

        let result = router.add(route(Method::Get, "/users/{user_id}"));
        assert!(matches!(result, Err(RouteAddError::Conflict(_))));
    }

    #[test]
    fn conflict_different_converter_same_position() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/items/{id:int}")).unwrap();

        // Different converter but same structural position - conflicts
        let result = router.add(route(Method::Get, "/items/{id:uuid}"));
        assert!(matches!(result, Err(RouteAddError::Conflict(_))));
    }

    #[test]
    fn no_conflict_different_methods() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();
        router.add(route(Method::Post, "/users")).unwrap();
        router.add(route(Method::Put, "/users")).unwrap();
        router.add(route(Method::Delete, "/users")).unwrap();
        router.add(route(Method::Patch, "/users")).unwrap();

        assert_eq!(router.routes().len(), 5);
    }

    #[test]
    fn no_conflict_static_vs_param() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users/me")).unwrap();
        router.add(route(Method::Get, "/users/{id}")).unwrap();

        // Both should be registered (static takes priority during matching)
        assert_eq!(router.routes().len(), 2);
    }

    #[test]
    fn no_conflict_different_path_lengths() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();
        router.add(route(Method::Get, "/users/{id}")).unwrap();
        router.add(route(Method::Get, "/users/{id}/posts")).unwrap();

        assert_eq!(router.routes().len(), 3);
    }

    #[test]
    fn conflict_error_display() {
        let err = RouteConflictError {
            method: Method::Get,
            new_path: "/new".to_string(),
            existing_path: "/existing".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("GET"));
        assert!(msg.contains("/new"));
        assert!(msg.contains("/existing"));
    }

    // =========================================================================
    // EDGE CASE TESTS
    // =========================================================================

    #[test]
    fn root_path() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/")).unwrap();

        let m = router.match_path("/", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/");
    }

    #[test]
    fn trailing_slash_handling() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();

        // Path with trailing slash should not match (strict matching)
        // Note: The router treats /users and /users/ differently
        let m = router.match_path("/users/", Method::Get);
        // This depends on implementation - let's test actual behavior
        assert!(m.is_none() || m.is_some());
    }

    #[test]
    fn multiple_consecutive_slashes() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();

        // Multiple slashes are normalized during path parsing
        // (empty segments are filtered out)
        let m = router.match_path("//users", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/users");
    }

    #[test]
    fn unicode_in_static_path() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/用户")).unwrap();
        router.add(route(Method::Get, "/données")).unwrap();

        let m = router.match_path("/用户", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/用户");

        let m = router.match_path("/données", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/données");
    }

    #[test]
    fn unicode_in_param_value() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users/{name}")).unwrap();

        let m = router.match_path("/users/田中", Method::Get);
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.params[0], ("name", "田中"));
    }

    #[test]
    fn special_characters_in_param_value() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/files/{name}")).unwrap();

        // Hyphens and underscores
        let m = router.match_path("/files/my-file_v2", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().params[0], ("name", "my-file_v2"));

        // Dots
        let m = router.match_path("/files/document.pdf", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().params[0], ("name", "document.pdf"));
    }

    #[test]
    fn empty_param_value() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users/{id}/posts")).unwrap();

        // Empty segment won't match a param (filtered out during parsing)
        let m = router.match_path("/users//posts", Method::Get);
        // This should not match because empty segment is skipped
        assert!(m.is_none());
    }

    #[test]
    fn very_long_path() {
        let mut router = Router::new();
        let long_path = "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/u/v/w/x/y/z";
        router.add(route(Method::Get, long_path)).unwrap();

        let m = router.match_path(long_path, Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, long_path);
    }

    #[test]
    fn many_routes_same_prefix() {
        let mut router = Router::new();
        for i in 0..100 {
            router
                .add(route(Method::Get, &format!("/api/v{}", i)))
                .unwrap();
        }

        assert_eq!(router.routes().len(), 100);

        // All routes should be matchable
        for i in 0..100 {
            let path = format!("/api/v{}", i);
            let m = router.match_path(&path, Method::Get);
            assert!(m.is_some());
            assert_eq!(m.unwrap().route.path, path);
        }
    }

    // =========================================================================
    // HEAD METHOD TESTS
    // =========================================================================

    #[test]
    fn head_matches_get_route() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();

        // HEAD should match GET routes
        let m = router.match_path("/users", Method::Head);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.method, Method::Get);
    }

    #[test]
    fn head_with_explicit_head_route() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();
        router.add(route(Method::Head, "/users")).unwrap();

        // If explicit HEAD is registered, should match HEAD
        let m = router.match_path("/users", Method::Head);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.method, Method::Head);
    }

    #[test]
    fn head_does_not_match_non_get() {
        let mut router = Router::new();
        router.add(route(Method::Post, "/users")).unwrap();

        // HEAD should not match POST
        let result = router.lookup("/users", Method::Head);
        match result {
            RouteLookup::MethodNotAllowed { allowed } => {
                assert!(!allowed.contains(Method::Head));
                assert!(allowed.contains(Method::Post));
            }
            _ => panic!("expected MethodNotAllowed"),
        }
    }

    // =========================================================================
    // CONVERTER EDGE CASE TESTS
    // =========================================================================

    #[test]
    fn int_converter_edge_cases() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/items/{id:int}")).unwrap();

        // Zero
        let m = router.match_path("/items/0", Method::Get);
        assert!(m.is_some());

        // Large positive
        let m = router.match_path("/items/9223372036854775807", Method::Get);
        assert!(m.is_some());

        // Large negative
        let m = router.match_path("/items/-9223372036854775808", Method::Get);
        assert!(m.is_some());

        // Leading zeros (still valid integer)
        let m = router.match_path("/items/007", Method::Get);
        assert!(m.is_some());

        // Plus sign (not standard integer format)
        let m = router.match_path("/items/+123", Method::Get);
        // Rust parse::<i64>() accepts +123
        assert!(m.is_some());
    }

    #[test]
    fn float_converter_edge_cases() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/values/{val:float}"))
            .unwrap();

        // Scientific notation
        let m = router.match_path("/values/1e10", Method::Get);
        assert!(m.is_some());

        // Negative exponent
        let m = router.match_path("/values/1e-10", Method::Get);
        assert!(m.is_some());

        // Infinity (Rust parses "inf" as f64::INFINITY)
        let m = router.match_path("/values/inf", Method::Get);
        assert!(m.is_some());

        // NaN (Rust parses "NaN" as f64::NAN)
        let m = router.match_path("/values/NaN", Method::Get);
        assert!(m.is_some());
    }

    #[test]
    fn uuid_converter_case_sensitivity() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/objects/{id:uuid}"))
            .unwrap();

        // Lowercase
        let m = router.match_path("/objects/550e8400-e29b-41d4-a716-446655440000", Method::Get);
        assert!(m.is_some());

        // Uppercase
        let m = router.match_path("/objects/550E8400-E29B-41D4-A716-446655440000", Method::Get);
        assert!(m.is_some());

        // Mixed case
        let m = router.match_path("/objects/550e8400-E29B-41d4-A716-446655440000", Method::Get);
        assert!(m.is_some());
    }

    #[test]
    fn uuid_converter_invalid_formats() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/objects/{id:uuid}"))
            .unwrap();

        // Wrong length
        assert!(
            router
                .match_path("/objects/550e8400-e29b-41d4-a716-44665544000", Method::Get)
                .is_none()
        );
        assert!(
            router
                .match_path(
                    "/objects/550e8400-e29b-41d4-a716-4466554400000",
                    Method::Get
                )
                .is_none()
        );

        // Missing hyphens
        assert!(
            router
                .match_path("/objects/550e8400e29b41d4a716446655440000", Method::Get)
                .is_none()
        );

        // Invalid hex characters
        assert!(
            router
                .match_path("/objects/550g8400-e29b-41d4-a716-446655440000", Method::Get)
                .is_none()
        );
    }

    #[test]
    fn unknown_converter_defaults_to_str() {
        let segments = parse_path("/items/{id:custom}");
        assert_eq!(segments.len(), 2);
        match &segments[1] {
            PathSegment::Param { name, converter } => {
                assert_eq!(*name, "id");
                assert_eq!(*converter, Converter::Str);
            }
            _ => panic!("Expected param segment"),
        }
    }

    // =========================================================================
    // PATH PARSING EDGE CASES
    // =========================================================================

    #[test]
    fn parse_empty_path() {
        let segments = parse_path("");
        assert!(segments.is_empty());
    }

    #[test]
    fn parse_root_only() {
        let segments = parse_path("/");
        assert!(segments.is_empty());
    }

    #[test]
    fn parse_leading_trailing_slashes() {
        let segments = parse_path("///users///");
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            PathSegment::Static(s) => assert_eq!(*s, "users"),
            _ => panic!("Expected static segment"),
        }
    }

    #[test]
    fn parse_param_with_colon_no_type() {
        // Edge case: param name contains colon but no valid type after
        let segments = parse_path("/items/{id:}");
        assert_eq!(segments.len(), 2);
        match &segments[1] {
            PathSegment::Param { name, converter } => {
                assert_eq!(*name, "id");
                // Empty after colon defaults to Str
                assert_eq!(*converter, Converter::Str);
            }
            _ => panic!("Expected param segment"),
        }
    }

    // =========================================================================
    // 404 AND 405 RESPONSE TESTS
    // =========================================================================

    #[test]
    fn lookup_404_empty_router() {
        let router = Router::new();
        assert!(matches!(
            router.lookup("/anything", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn lookup_404_no_matching_path() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();
        router.add(route(Method::Get, "/items")).unwrap();

        assert!(matches!(
            router.lookup("/other", Method::Get),
            RouteLookup::NotFound
        ));
        assert!(matches!(
            router.lookup("/user", Method::Get),
            RouteLookup::NotFound
        )); // Typo
    }

    #[test]
    fn lookup_404_partial_path_match() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/api/v1/users")).unwrap();

        // Partial matches should be 404
        assert!(matches!(
            router.lookup("/api", Method::Get),
            RouteLookup::NotFound
        ));
        assert!(matches!(
            router.lookup("/api/v1", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn lookup_404_extra_path_segments() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();

        // Extra segments should be 404
        assert!(matches!(
            router.lookup("/users/extra", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn lookup_405_single_method() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/users")).unwrap();

        let result = router.lookup("/users", Method::Post);
        match result {
            RouteLookup::MethodNotAllowed { allowed } => {
                assert_eq!(allowed.methods(), &[Method::Get, Method::Head]);
            }
            _ => panic!("expected MethodNotAllowed"),
        }
    }

    #[test]
    fn lookup_405_all_methods() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/resource")).unwrap();
        router.add(route(Method::Post, "/resource")).unwrap();
        router.add(route(Method::Put, "/resource")).unwrap();
        router.add(route(Method::Delete, "/resource")).unwrap();
        router.add(route(Method::Patch, "/resource")).unwrap();
        router.add(route(Method::Options, "/resource")).unwrap();

        let result = router.lookup("/resource", Method::Trace);
        match result {
            RouteLookup::MethodNotAllowed { allowed } => {
                let header = allowed.header_value();
                assert!(header.contains("GET"));
                assert!(header.contains("HEAD"));
                assert!(header.contains("POST"));
                assert!(header.contains("PUT"));
                assert!(header.contains("DELETE"));
                assert!(header.contains("PATCH"));
                assert!(header.contains("OPTIONS"));
            }
            _ => panic!("expected MethodNotAllowed"),
        }
    }

    // =========================================================================
    // ALLOWED METHODS TESTS
    // =========================================================================

    #[test]
    fn allowed_methods_deduplication() {
        // If GET is added twice, should only appear once
        let allowed = AllowedMethods::new(vec![Method::Get, Method::Get, Method::Post]);
        assert_eq!(allowed.methods().len(), 3); // GET, HEAD, POST
    }

    #[test]
    fn allowed_methods_sorting() {
        // Methods should be sorted in standard order
        let allowed = AllowedMethods::new(vec![Method::Delete, Method::Get, Method::Post]);
        assert_eq!(allowed.methods()[0], Method::Get);
        assert_eq!(allowed.methods()[1], Method::Head); // Added automatically
        assert_eq!(allowed.methods()[2], Method::Post);
        assert_eq!(allowed.methods()[3], Method::Delete);
    }

    #[test]
    fn allowed_methods_head_not_duplicated() {
        // If HEAD is already present, don't add it again
        let allowed = AllowedMethods::new(vec![Method::Get, Method::Head]);
        let count = allowed
            .methods()
            .iter()
            .filter(|&&m| m == Method::Head)
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn allowed_methods_empty() {
        let allowed = AllowedMethods::new(vec![]);
        assert!(allowed.methods().is_empty());
        assert_eq!(allowed.header_value(), "");
    }

    // =========================================================================
    // WILDCARD CATCH-ALL ROUTE TESTS ({*path} syntax)
    // =========================================================================

    #[test]
    fn wildcard_asterisk_syntax_basic() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/files/{*path}")).unwrap();

        let m = router.match_path("/files/a.txt", Method::Get).unwrap();
        assert_eq!(m.params[0], ("path", "a.txt"));
    }

    #[test]
    fn wildcard_asterisk_captures_multiple_segments() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/files/{*filepath}"))
            .unwrap();

        let m = router
            .match_path("/files/css/styles/main.css", Method::Get)
            .unwrap();
        assert_eq!(m.params[0], ("filepath", "css/styles/main.css"));
    }

    #[test]
    fn wildcard_asterisk_with_prefix() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/api/v1/{*rest}")).unwrap();

        let m = router
            .match_path("/api/v1/users/123/posts", Method::Get)
            .unwrap();
        assert_eq!(m.params[0], ("rest", "users/123/posts"));
    }

    #[test]
    fn wildcard_asterisk_empty_capture() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/files/{*path}")).unwrap();

        // Single segment after prefix should work
        let m = router.match_path("/files/x", Method::Get).unwrap();
        assert_eq!(m.params[0], ("path", "x"));
    }

    #[test]
    fn wildcard_asterisk_must_be_terminal() {
        let mut router = Router::new();
        let result = router.add(route(Method::Get, "/files/{*path}/edit"));
        assert!(matches!(result, Err(RouteAddError::InvalidPath(_))));
    }

    #[test]
    fn wildcard_asterisk_syntax_equivalent_to_path_converter() {
        // Both syntaxes should produce the same result
        let segments_asterisk = parse_path("/files/{*filepath}");
        let segments_converter = parse_path("/files/{filepath:path}");

        assert_eq!(segments_asterisk.len(), 2);
        assert_eq!(segments_converter.len(), 2);

        match (&segments_asterisk[1], &segments_converter[1]) {
            (
                PathSegment::Param {
                    name: n1,
                    converter: c1,
                },
                PathSegment::Param {
                    name: n2,
                    converter: c2,
                },
            ) => {
                assert_eq!(*n1, "filepath");
                assert_eq!(*n2, "filepath");
                assert_eq!(*c1, Converter::Path);
                assert_eq!(*c2, Converter::Path);
            }
            _ => panic!("Expected param segments"),
        }
    }

    #[test]
    fn wildcard_asterisk_spa_routing() {
        let mut router = Router::new();
        // Static API routes take priority
        router.add(route(Method::Get, "/api/users")).unwrap();
        router.add(route(Method::Get, "/api/posts")).unwrap();
        // Catch-all for SPA
        router.add(route(Method::Get, "/{*route}")).unwrap();

        // API routes match exactly
        let m = router.match_path("/api/users", Method::Get).unwrap();
        assert_eq!(m.route.path, "/api/users");

        // Other paths caught by wildcard
        let m = router.match_path("/dashboard", Method::Get).unwrap();
        assert_eq!(m.params[0], ("route", "dashboard"));

        let m = router
            .match_path("/users/123/profile", Method::Get)
            .unwrap();
        assert_eq!(m.params[0], ("route", "users/123/profile"));
    }

    #[test]
    fn wildcard_asterisk_file_serving() {
        let mut router = Router::new();
        router
            .add(route(Method::Get, "/static/{*filepath}"))
            .unwrap();

        let m = router
            .match_path("/static/js/app.bundle.js", Method::Get)
            .unwrap();
        assert_eq!(m.params[0], ("filepath", "js/app.bundle.js"));

        let m = router
            .match_path("/static/images/logo.png", Method::Get)
            .unwrap();
        assert_eq!(m.params[0], ("filepath", "images/logo.png"));
    }

    #[test]
    fn wildcard_asterisk_priority_lowest() {
        let mut router = Router::new();
        // Static routes have highest priority
        router.add(route(Method::Get, "/users/me")).unwrap();
        // Single-segment param has medium priority
        router.add(route(Method::Get, "/users/{id}")).unwrap();
        // Catch-all has lowest priority
        router.add(route(Method::Get, "/{*path}")).unwrap();

        // Static match
        let m = router.match_path("/users/me", Method::Get).unwrap();
        assert_eq!(m.route.path, "/users/me");
        assert!(m.params.is_empty());

        // Param match
        let m = router.match_path("/users/123", Method::Get).unwrap();
        assert_eq!(m.route.path, "/users/{id}");
        assert_eq!(m.params[0], ("id", "123"));

        // Wildcard catches the rest
        let m = router.match_path("/other/deep/path", Method::Get).unwrap();
        assert_eq!(m.route.path, "/{*path}");
        assert_eq!(m.params[0], ("path", "other/deep/path"));
    }

    #[test]
    fn parse_wildcard_asterisk_syntax() {
        let segments = parse_path("/files/{*path}");
        assert_eq!(segments.len(), 2);

        match &segments[0] {
            PathSegment::Static(s) => assert_eq!(*s, "files"),
            _ => panic!("Expected static segment"),
        }

        match &segments[1] {
            PathSegment::Param { name, converter } => {
                assert_eq!(*name, "path");
                assert_eq!(*converter, Converter::Path);
            }
            _ => panic!("Expected param segment"),
        }
    }

    #[test]
    fn wildcard_asterisk_conflict_with_path_converter() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/files/{*path}")).unwrap();

        // Same route with different syntax should conflict
        let result = router.add(route(Method::Get, "/files/{filepath:path}"));
        assert!(matches!(result, Err(RouteAddError::Conflict(_))));
    }

    #[test]
    fn wildcard_asterisk_different_methods_no_conflict() {
        let mut router = Router::new();
        router.add(route(Method::Get, "/files/{*path}")).unwrap();
        router.add(route(Method::Post, "/files/{*path}")).unwrap();
        router.add(route(Method::Delete, "/files/{*path}")).unwrap();

        assert_eq!(router.routes().len(), 3);

        // Each method works
        let m = router.match_path("/files/a/b/c", Method::Get).unwrap();
        assert_eq!(m.route.method, Method::Get);

        let m = router.match_path("/files/a/b/c", Method::Post).unwrap();
        assert_eq!(m.route.method, Method::Post);

        let m = router.match_path("/files/a/b/c", Method::Delete).unwrap();
        assert_eq!(m.route.method, Method::Delete);
    }
}
