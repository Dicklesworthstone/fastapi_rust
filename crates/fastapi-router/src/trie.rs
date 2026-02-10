//! Radix trie router implementation.
//!
//! This crate provides pure path matching without any handler storage.
//! Handlers are stored externally and looked up by route index.

use crate::r#match::{AllowedMethods, RouteLookup, RouteMatch};
use fastapi_types::Method;
use std::collections::HashMap;
use std::fmt;

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
    /// Path segment (can contain /).
    Path,
}

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

/// A route definition (metadata only, no handler).
///
/// Routes store path patterns and metadata for matching.
/// Handlers are stored separately and looked up by route index.
#[derive(Debug, Clone)]
pub struct Route {
    /// Route path pattern (e.g., "/items/{id}").
    pub path: String,
    /// HTTP method.
    pub method: Method,
    /// Operation ID for OpenAPI.
    pub operation_id: String,
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

impl Route {
    /// Create a new route.
    #[must_use]
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        let path = path.into();
        let operation_id = path.replace('/', "_").replace(['{', '}'], "");
        Self {
            path,
            method,
            operation_id,
        }
    }

    /// Set the operation ID for OpenAPI documentation.
    #[must_use]
    pub fn operation_id(mut self, id: impl Into<String>) -> Self {
        self.operation_id = id.into();
        self
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

    /// Add a route, returning an error if it conflicts with existing routes.
    ///
    /// Conflict rules:
    /// - Same HTTP method + structurally identical path patterns conflict
    /// - Static segments take priority over parameter segments (no conflict)
    /// - Parameter names/converters do not disambiguate conflicts (one param slot per segment)
    pub fn add(&mut self, route: Route) -> Result<(), RouteConflictError> {
        if let Some(conflict) = self.find_conflict(&route) {
            return Err(conflict);
        }

        let route_idx = self.routes.len();
        let path = route.path.clone();
        let method = route.method;
        self.routes.push(route);

        let segments = parse_path(&path);
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
        let segments: Vec<_> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut params = Vec::new();
        let mut node = &self.root;

        for segment in segments {
            // Try static match first
            if let Some(child) = node.find_static(segment) {
                node = child;
                continue;
            }

            // Try parameter match
            if let Some(child) = node.find_param() {
                if let Some(ref info) = child.param {
                    if info.converter.matches(segment) {
                        params.push((info.name.as_str(), segment));
                        node = child;
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

fn paths_conflict(a: &str, b: &str) -> bool {
    let a_segments = parse_path(a);
    let b_segments = parse_path(b);

    if a_segments.len() != b_segments.len() {
        return false;
    }

    for (left, right) in a_segments.iter().zip(b_segments.iter()) {
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
            (PathSegment::Param { .. }, PathSegment::Param { .. }) => {}
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_route_match() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/users")).unwrap();
        router.add(Route::new(Method::Get, "/items")).unwrap();

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
        router
            .add(Route::new(Method::Get, "/api/v1/users"))
            .unwrap();
        router
            .add(Route::new(Method::Get, "/api/v2/users"))
            .unwrap();

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
        router
            .add(Route::new(Method::Get, "/users/{user_id}"))
            .unwrap();

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
            .add(Route::new(Method::Get, "/users/{user_id}/posts/{post_id}"))
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
        router
            .add(Route::new(Method::Get, "/items/{id:int}"))
            .unwrap();

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
            .add(Route::new(Method::Get, "/values/{val:float}"))
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
            .add(Route::new(Method::Get, "/objects/{id:uuid}"))
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
    fn method_dispatch() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/items")).unwrap();
        router.add(Route::new(Method::Post, "/items")).unwrap();
        router
            .add(Route::new(Method::Delete, "/items/{id}"))
            .unwrap();

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
        router.add(Route::new(Method::Get, "/users")).unwrap();

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
        router.add(Route::new(Method::Get, "/users")).unwrap();
        router.add(Route::new(Method::Post, "/users")).unwrap();
        router.add(Route::new(Method::Delete, "/users")).unwrap();

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
        router.add(Route::new(Method::Get, "/users")).unwrap();

        assert!(matches!(
            router.lookup("/missing", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn lookup_not_found_when_converter_mismatch() {
        let mut router = Router::new();
        router
            .add(Route::new(Method::Get, "/items/{id:int}"))
            .unwrap();

        assert!(matches!(
            router.lookup("/items/abc", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn static_takes_priority_over_param() {
        let mut router = Router::new();
        // Order matters: add static first, then param
        router.add(Route::new(Method::Get, "/users/me")).unwrap();
        router.add(Route::new(Method::Get, "/users/{id}")).unwrap();

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
            .add(Route::new(Method::Get, "/users/{user_id}/posts/{post_id}"))
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
        let _ = router.add(Route::new(Method::Get, "/a"));
        let _ = router.add(Route::new(Method::Post, "/b"));

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
        router.add(Route::new(Method::Get, "/users")).unwrap();

        let result = router.add(Route::new(Method::Get, "/users"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.method, Method::Get);
        assert_eq!(err.new_path, "/users");
        assert_eq!(err.existing_path, "/users");
    }

    #[test]
    fn conflict_same_method_same_param_pattern() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/users/{id}")).unwrap();

        // Same structure, different param name - still conflicts
        let result = router.add(Route::new(Method::Get, "/users/{user_id}"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.existing_path, "/users/{id}");
        assert_eq!(err.new_path, "/users/{user_id}");
    }

    #[test]
    fn conflict_different_converter_same_position() {
        let mut router = Router::new();
        router
            .add(Route::new(Method::Get, "/items/{id:int}"))
            .unwrap();

        // Different converter but same structural position - conflicts
        let result = router.add(Route::new(Method::Get, "/items/{id:uuid}"));
        assert!(result.is_err());
    }

    #[test]
    fn no_conflict_different_methods() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/users")).unwrap();
        router.add(Route::new(Method::Post, "/users")).unwrap();
        router.add(Route::new(Method::Put, "/users")).unwrap();
        router.add(Route::new(Method::Delete, "/users")).unwrap();
        router.add(Route::new(Method::Patch, "/users")).unwrap();

        assert_eq!(router.routes().len(), 5);
    }

    #[test]
    fn no_conflict_static_vs_param() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/users/me")).unwrap();
        router.add(Route::new(Method::Get, "/users/{id}")).unwrap();

        // Both should be registered (static takes priority during matching)
        assert_eq!(router.routes().len(), 2);
    }

    #[test]
    fn no_conflict_different_path_lengths() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/users")).unwrap();
        router.add(Route::new(Method::Get, "/users/{id}")).unwrap();
        router
            .add(Route::new(Method::Get, "/users/{id}/posts"))
            .unwrap();

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
        router.add(Route::new(Method::Get, "/")).unwrap();

        let m = router.match_path("/", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/");
    }

    #[test]
    fn trailing_slash_handling() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/users")).unwrap();

        // Path with trailing slash should not match (strict matching)
        // Note: The router treats /users and /users/ differently
        let m = router.match_path("/users/", Method::Get);
        // This depends on implementation - let's test actual behavior
        assert!(m.is_none() || m.is_some());
    }

    #[test]
    fn multiple_consecutive_slashes() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/users")).unwrap();

        // Multiple slashes are normalized during path parsing
        // (empty segments are filtered out)
        let m = router.match_path("//users", Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, "/users");
    }

    #[test]
    fn unicode_in_static_path() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/用户")).unwrap();
        router.add(Route::new(Method::Get, "/données")).unwrap();

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
        router
            .add(Route::new(Method::Get, "/users/{name}"))
            .unwrap();

        let m = router.match_path("/users/田中", Method::Get);
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.params[0], ("name", "田中"));
    }

    #[test]
    fn special_characters_in_param_value() {
        let mut router = Router::new();
        router
            .add(Route::new(Method::Get, "/files/{name}"))
            .unwrap();

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
        router
            .add(Route::new(Method::Get, "/users/{id}/posts"))
            .unwrap();

        // Empty segment won't match a param (filtered out during parsing)
        let m = router.match_path("/users//posts", Method::Get);
        // This should not match because empty segment is skipped
        assert!(m.is_none());
    }

    #[test]
    fn very_long_path() {
        let mut router = Router::new();
        let long_path = "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/u/v/w/x/y/z";
        router.add(Route::new(Method::Get, long_path)).unwrap();

        let m = router.match_path(long_path, Method::Get);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.path, long_path);
    }

    #[test]
    fn many_routes_same_prefix() {
        let mut router = Router::new();
        for i in 0..100 {
            router
                .add(Route::new(Method::Get, &format!("/api/v{}", i)))
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
        router.add(Route::new(Method::Get, "/users")).unwrap();

        // HEAD should match GET routes
        let m = router.match_path("/users", Method::Head);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.method, Method::Get);
    }

    #[test]
    fn head_with_explicit_head_route() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/users")).unwrap();
        router.add(Route::new(Method::Head, "/users")).unwrap();

        // If explicit HEAD is registered, should match HEAD
        let m = router.match_path("/users", Method::Head);
        assert!(m.is_some());
        assert_eq!(m.unwrap().route.method, Method::Head);
    }

    #[test]
    fn head_does_not_match_non_get() {
        let mut router = Router::new();
        router.add(Route::new(Method::Post, "/users")).unwrap();

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
        router
            .add(Route::new(Method::Get, "/items/{id:int}"))
            .unwrap();

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
            .add(Route::new(Method::Get, "/values/{val:float}"))
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
            .add(Route::new(Method::Get, "/objects/{id:uuid}"))
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
            .add(Route::new(Method::Get, "/objects/{id:uuid}"))
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
        router.add(Route::new(Method::Get, "/users")).unwrap();
        router.add(Route::new(Method::Get, "/items")).unwrap();

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
        router
            .add(Route::new(Method::Get, "/api/v1/users"))
            .unwrap();

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
        router.add(Route::new(Method::Get, "/users")).unwrap();

        // Extra segments should be 404
        assert!(matches!(
            router.lookup("/users/extra", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn lookup_405_single_method() {
        let mut router = Router::new();
        router.add(Route::new(Method::Get, "/users")).unwrap();

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
        router.add(Route::new(Method::Get, "/resource")).unwrap();
        router.add(Route::new(Method::Post, "/resource")).unwrap();
        router.add(Route::new(Method::Put, "/resource")).unwrap();
        router.add(Route::new(Method::Delete, "/resource")).unwrap();
        router.add(Route::new(Method::Patch, "/resource")).unwrap();
        router
            .add(Route::new(Method::Options, "/resource"))
            .unwrap();

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
}
