//! Path matching and routing utilities.
//!
//! This module provides path parameter extraction with type converters,
//! similar to FastAPI's path converters: `{id:int}`, `{value:float}`,
//! `{uuid:uuid}`, `{path:path}`.
//!
//! # Trailing Slash Handling
//!
//! This module also handles trailing slash normalization via [`TrailingSlashMode`]:
//! - `Strict`: Exact match required (default)
//! - `Redirect`: 308 redirect to canonical form (no trailing slash)
//! - `RedirectWithSlash`: 308 redirect to form with trailing slash
//! - `MatchBoth`: Accept both forms without redirect

use crate::request::Method;

/// Trailing slash handling mode.
///
/// Controls how the router handles trailing slashes in URLs.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::routing::TrailingSlashMode;
///
/// // Redirect /users/ to /users
/// let mode = TrailingSlashMode::Redirect;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrailingSlashMode {
    /// Exact match required - `/users` and `/users/` are different routes.
    #[default]
    Strict,
    /// Redirect trailing slash to no trailing slash (308 Permanent Redirect).
    /// `/users/` redirects to `/users`.
    Redirect,
    /// Redirect no trailing slash to with trailing slash (308 Permanent Redirect).
    /// `/users` redirects to `/users/`.
    RedirectWithSlash,
    /// Accept both forms without redirect.
    /// Both `/users` and `/users/` match the route.
    MatchBoth,
}

/// Path parameter type converter.
///
/// Converters validate and constrain path parameter values during matching.
///
/// # Supported Types
///
/// - `Str` (default): Any string value
/// - `Int`: Integer values (i64)
/// - `Float`: Floating-point values (f64)
/// - `Uuid`: UUID format (8-4-4-4-12 hex digits)
/// - `Path`: Captures remaining path including slashes
///
/// # Example Route Patterns
///
/// - `/users/{id}` - String parameter (default)
/// - `/items/{id:int}` - Integer parameter
/// - `/values/{val:float}` - Float parameter
/// - `/objects/{id:uuid}` - UUID parameter
/// - `/files/{path:path}` - Captures `/files/a/b/c.txt` as `path="a/b/c.txt"`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Converter {
    /// String (default).
    Str,
    /// Integer (i64).
    Int,
    /// Float (f64).
    Float,
    /// UUID format.
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

    /// Parse a converter type from a string.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "int" => Self::Int,
            "float" => Self::Float,
            "uuid" => Self::Uuid,
            "path" => Self::Path,
            _ => Self::Str,
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

/// A parsed path segment.
#[derive(Debug, Clone)]
pub enum PathSegment {
    /// A static path segment (e.g., "users").
    Static(String),
    /// A parameter segment with name and converter.
    Param(ParamInfo),
}

/// A parsed route pattern.
#[derive(Debug, Clone)]
pub struct RoutePattern {
    /// The original path pattern string.
    pub pattern: String,
    /// Parsed segments.
    pub segments: Vec<PathSegment>,
    /// Whether the last segment is a path converter (captures slashes).
    pub has_path_converter: bool,
}

impl RoutePattern {
    /// Parse a route pattern from a string.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let pattern = RoutePattern::parse("/users/{id:int}/posts/{post_id}");
    /// ```
    #[must_use]
    pub fn parse(pattern: &str) -> Self {
        let segments = parse_path_segments(pattern);
        let has_path_converter = matches!(
            segments.last(),
            Some(PathSegment::Param(ParamInfo {
                converter: Converter::Path,
                ..
            }))
        );

        Self {
            pattern: pattern.to_string(),
            segments,
            has_path_converter,
        }
    }

    /// Try to match this pattern against a path, extracting parameters.
    ///
    /// Returns `Some(params)` if the path matches, `None` otherwise.
    /// Parameter names are owned strings, values are borrowed from the path.
    #[must_use]
    pub fn match_path<'a>(&self, path: &'a str) -> Option<Vec<(String, &'a str)>> {
        let path_ranges = segment_ranges(path);
        let mut path_segments: Vec<&'a str> = Vec::with_capacity(path_ranges.len());
        for (start, end) in &path_ranges {
            path_segments.push(&path[*start..*end]);
        }

        let mut params = Vec::new();
        let mut path_idx = 0;
        let last_end = path_ranges.last().map_or(0, |(_, end)| *end);

        for segment in &self.segments {
            match segment {
                PathSegment::Static(expected) => {
                    if path_idx >= path_segments.len() || path_segments[path_idx] != expected {
                        return None;
                    }
                    path_idx += 1;
                }
                PathSegment::Param(info) => {
                    if path_idx >= path_segments.len() {
                        return None;
                    }

                    if info.converter == Converter::Path {
                        // Path converter captures everything remaining
                        let start = path_ranges[path_idx].0;
                        let value = &path[start..last_end];
                        params.push((info.name.clone(), value));
                        // Consume all remaining segments
                        path_idx = path_segments.len();
                    } else {
                        let value = path_segments[path_idx];
                        if !info.converter.matches(value) {
                            return None;
                        }
                        params.push((info.name.clone(), value));
                        path_idx += 1;
                    }
                }
            }
        }

        // All path segments must be consumed (unless we had a path converter)
        if path_idx != path_segments.len() && !self.has_path_converter {
            return None;
        }

        Some(params)
    }

    /// Check if this pattern could potentially match the given path (ignoring method).
    ///
    /// Used for 405 Method Not Allowed detection.
    #[must_use]
    pub fn could_match(&self, path: &str) -> bool {
        self.match_path(path).is_some()
    }
}

fn parse_path_segments(path: &str) -> Vec<PathSegment> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.starts_with('{') && s.ends_with('}') {
                let inner = &s[1..s.len() - 1];
                let (name, converter) = if let Some(pos) = inner.find(':') {
                    let conv = Converter::parse(&inner[pos + 1..]);
                    (inner[..pos].to_string(), conv)
                } else {
                    (inner.to_string(), Converter::Str)
                };
                PathSegment::Param(ParamInfo { name, converter })
            } else {
                PathSegment::Static(s.to_string())
            }
        })
        .collect()
}

fn segment_ranges(path: &str) -> Vec<(usize, usize)> {
    let bytes = path.as_bytes();
    let mut ranges = Vec::new();
    let mut idx = 0;
    while idx < bytes.len() {
        // Skip leading slashes
        while idx < bytes.len() && bytes[idx] == b'/' {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }
        let start = idx;
        // Find end of segment
        while idx < bytes.len() && bytes[idx] != b'/' {
            idx += 1;
        }
        ranges.push((start, idx));
    }
    ranges
}

/// Result of a route lookup.
#[derive(Debug)]
pub enum RouteLookup<'a, T> {
    /// A route matched.
    Match {
        /// The matched route data.
        route: &'a T,
        /// Extracted path parameters (name, value).
        params: Vec<(String, String)>,
    },
    /// Path matched but method not allowed.
    MethodNotAllowed {
        /// Methods that are allowed for this path.
        allowed: Vec<Method>,
    },
    /// Redirect to a different path (308 Permanent Redirect).
    ///
    /// Used for trailing slash normalization.
    Redirect {
        /// The path to redirect to.
        target: String,
    },
    /// No route matched.
    NotFound,
}

/// Simple route table for path matching.
///
/// This provides O(n) matching but with full converter support.
/// For larger applications, consider using fastapi-router's trie.
pub struct RouteTable<T> {
    routes: Vec<(Method, RoutePattern, T)>,
}

impl<T> RouteTable<T> {
    /// Create a new empty route table.
    #[must_use]
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Add a route to the table.
    pub fn add(&mut self, method: Method, pattern: &str, data: T) {
        let parsed = RoutePattern::parse(pattern);
        self.routes.push((method, parsed, data));
    }

    /// Look up a route by path and method.
    #[must_use]
    pub fn lookup(&self, path: &str, method: Method) -> RouteLookup<'_, T> {
        // First, try to find exact method + path match
        for (route_method, pattern, data) in &self.routes {
            if let Some(params) = pattern.match_path(path) {
                // Convert params to owned strings
                let owned_params: Vec<(String, String)> = params
                    .into_iter()
                    .map(|(name, value)| (name, value.to_string()))
                    .collect();

                if *route_method == method {
                    return RouteLookup::Match {
                        route: data,
                        params: owned_params,
                    };
                }
                // HEAD can match GET routes
                if method == Method::Head && *route_method == Method::Get {
                    return RouteLookup::Match {
                        route: data,
                        params: owned_params,
                    };
                }
            }
        }

        // Check if any route matches the path (for 405)
        let mut allowed_methods: Vec<Method> = Vec::new();
        for (route_method, pattern, _) in &self.routes {
            if pattern.could_match(path) && !allowed_methods.contains(route_method) {
                allowed_methods.push(*route_method);
            }
        }

        if !allowed_methods.is_empty() {
            // Add HEAD if GET is allowed
            if allowed_methods.contains(&Method::Get) && !allowed_methods.contains(&Method::Head) {
                allowed_methods.push(Method::Head);
            }
            // Sort methods for consistent output
            allowed_methods.sort_by_key(|m| method_order(*m));
            return RouteLookup::MethodNotAllowed {
                allowed: allowed_methods,
            };
        }

        RouteLookup::NotFound
    }

    /// Look up a route by path and method, with trailing slash handling.
    ///
    /// This extends `lookup` with trailing slash normalization based on the mode:
    /// - `Strict`: Exact match required
    /// - `Redirect`: Redirect trailing slash to no trailing slash
    /// - `RedirectWithSlash`: Redirect no trailing slash to with trailing slash
    /// - `MatchBoth`: Accept both forms without redirect
    #[must_use]
    pub fn lookup_with_trailing_slash(
        &self,
        path: &str,
        method: Method,
        mode: TrailingSlashMode,
    ) -> RouteLookup<'_, T> {
        // First, try exact match
        let result = self.lookup(path, method);
        if !matches!(result, RouteLookup::NotFound) {
            return result;
        }

        // If strict mode or no trailing slash handling, return the result
        if mode == TrailingSlashMode::Strict {
            return result;
        }

        // Try alternate path (toggle trailing slash)
        let has_trailing_slash = path.len() > 1 && path.ends_with('/');
        let alt_path = if has_trailing_slash {
            // Remove trailing slash
            &path[..path.len() - 1]
        } else {
            // Need to allocate for adding trailing slash
            return self.lookup_with_trailing_slash_add(path, method, mode);
        };

        let alt_result = self.lookup(alt_path, method);
        match (&alt_result, mode) {
            (RouteLookup::Match { .. }, TrailingSlashMode::Redirect) => {
                // Path has trailing slash but route matches without it - redirect
                RouteLookup::Redirect {
                    target: alt_path.to_string(),
                }
            }
            (RouteLookup::Match { route, params }, TrailingSlashMode::MatchBoth) => {
                // Match both - return the match directly
                RouteLookup::Match {
                    route,
                    params: params.clone(),
                }
            }
            (RouteLookup::MethodNotAllowed { allowed: _ }, TrailingSlashMode::Redirect) => {
                // Path has trailing slash, route exists without it - redirect
                RouteLookup::Redirect {
                    target: alt_path.to_string(),
                }
            }
            (RouteLookup::MethodNotAllowed { allowed }, TrailingSlashMode::MatchBoth) => {
                // Return method not allowed for the alt path
                RouteLookup::MethodNotAllowed {
                    allowed: allowed.clone(),
                }
            }
            _ => result, // NotFound or other modes
        }
    }

    /// Helper for lookup_with_trailing_slash when we need to add a trailing slash.
    fn lookup_with_trailing_slash_add(
        &self,
        path: &str,
        method: Method,
        mode: TrailingSlashMode,
    ) -> RouteLookup<'_, T> {
        // Path doesn't have trailing slash, try with it
        let with_slash = format!("{}/", path);
        let alt_result = self.lookup(&with_slash, method);

        match (&alt_result, mode) {
            (RouteLookup::Match { .. }, TrailingSlashMode::RedirectWithSlash) => {
                // Path doesn't have trailing slash but route matches with it - redirect
                RouteLookup::Redirect { target: with_slash }
            }
            (RouteLookup::Match { route, params }, TrailingSlashMode::MatchBoth) => {
                // Match both - return the match directly
                RouteLookup::Match {
                    route,
                    params: params.clone(),
                }
            }
            (
                RouteLookup::MethodNotAllowed { allowed: _ },
                TrailingSlashMode::RedirectWithSlash,
            ) => {
                // Route exists with trailing slash - redirect
                RouteLookup::Redirect { target: with_slash }
            }
            (RouteLookup::MethodNotAllowed { allowed }, TrailingSlashMode::MatchBoth) => {
                // Return method not allowed for the alt path
                RouteLookup::MethodNotAllowed {
                    allowed: allowed.clone(),
                }
            }
            _ => RouteLookup::NotFound,
        }
    }

    /// Get the number of routes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Check if the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

impl<T> Default for RouteTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the sort order for an HTTP method.
///
/// Used to produce consistent ordering in Allow headers:
/// GET, HEAD, POST, PUT, DELETE, PATCH, OPTIONS, TRACE
#[must_use]
pub fn method_order(method: Method) -> u8 {
    match method {
        Method::Get => 0,
        Method::Head => 1,
        Method::Post => 2,
        Method::Put => 3,
        Method::Delete => 4,
        Method::Patch => 5,
        Method::Options => 6,
        Method::Trace => 7,
    }
}

/// Format allowed methods as an HTTP Allow header value.
#[must_use]
pub fn format_allow_header(methods: &[Method]) -> String {
    methods
        .iter()
        .map(|m| m.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converter_str_matches_anything() {
        assert!(Converter::Str.matches("hello"));
        assert!(Converter::Str.matches("123"));
        assert!(Converter::Str.matches(""));
    }

    #[test]
    fn converter_int_matches_integers() {
        assert!(Converter::Int.matches("123"));
        assert!(Converter::Int.matches("-456"));
        assert!(Converter::Int.matches("0"));
        assert!(!Converter::Int.matches("12.34"));
        assert!(!Converter::Int.matches("abc"));
        assert!(!Converter::Int.matches(""));
    }

    #[test]
    fn converter_float_matches_floats() {
        assert!(Converter::Float.matches("3.14"));
        assert!(Converter::Float.matches("42"));
        assert!(Converter::Float.matches("-1.5"));
        assert!(Converter::Float.matches("1e10"));
        assert!(!Converter::Float.matches("abc"));
    }

    #[test]
    fn converter_uuid_matches_uuids() {
        assert!(Converter::Uuid.matches("550e8400-e29b-41d4-a716-446655440000"));
        assert!(Converter::Uuid.matches("550E8400-E29B-41D4-A716-446655440000"));
        assert!(!Converter::Uuid.matches("not-a-uuid"));
        assert!(!Converter::Uuid.matches("550e8400e29b41d4a716446655440000")); // No hyphens
    }

    #[test]
    fn parse_static_path() {
        let pattern = RoutePattern::parse("/users");
        assert_eq!(pattern.segments.len(), 1);
        assert!(matches!(&pattern.segments[0], PathSegment::Static(s) if s == "users"));
    }

    #[test]
    fn parse_path_with_param() {
        let pattern = RoutePattern::parse("/users/{id}");
        assert_eq!(pattern.segments.len(), 2);
        assert!(matches!(&pattern.segments[0], PathSegment::Static(s) if s == "users"));
        assert!(
            matches!(&pattern.segments[1], PathSegment::Param(info) if info.name == "id" && info.converter == Converter::Str)
        );
    }

    #[test]
    fn parse_path_with_typed_param() {
        let pattern = RoutePattern::parse("/items/{id:int}");
        assert_eq!(pattern.segments.len(), 2);
        assert!(
            matches!(&pattern.segments[1], PathSegment::Param(info) if info.name == "id" && info.converter == Converter::Int)
        );
    }

    #[test]
    fn parse_path_with_path_converter() {
        let pattern = RoutePattern::parse("/files/{path:path}");
        assert!(pattern.has_path_converter);
    }

    #[test]
    fn match_static_path() {
        let pattern = RoutePattern::parse("/users");
        assert!(pattern.match_path("/users").is_some());
        assert!(pattern.match_path("/items").is_none());
    }

    #[test]
    fn match_path_extracts_params() {
        let pattern = RoutePattern::parse("/users/{id}");
        let params = pattern.match_path("/users/42").unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "id");
        assert_eq!(params[0].1, "42");
    }

    #[test]
    fn match_path_validates_int_converter() {
        let pattern = RoutePattern::parse("/items/{id:int}");
        assert!(pattern.match_path("/items/123").is_some());
        assert!(pattern.match_path("/items/abc").is_none());
    }

    #[test]
    fn match_path_validates_uuid_converter() {
        let pattern = RoutePattern::parse("/objects/{id:uuid}");
        assert!(
            pattern
                .match_path("/objects/550e8400-e29b-41d4-a716-446655440000")
                .is_some()
        );
        assert!(pattern.match_path("/objects/not-a-uuid").is_none());
    }

    #[test]
    fn match_path_converter_captures_slashes() {
        let pattern = RoutePattern::parse("/files/{path:path}");
        let params = pattern.match_path("/files/a/b/c.txt").unwrap();
        assert_eq!(params[0].0, "path");
        assert_eq!(params[0].1, "a/b/c.txt");
    }

    #[test]
    fn match_multiple_params() {
        let pattern = RoutePattern::parse("/users/{user_id}/posts/{post_id}");
        let params = pattern.match_path("/users/42/posts/99").unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].0, "user_id");
        assert_eq!(params[0].1, "42");
        assert_eq!(params[1].0, "post_id");
        assert_eq!(params[1].1, "99");
    }

    #[test]
    fn route_table_lookup_match() {
        let mut table: RouteTable<&str> = RouteTable::new();
        table.add(Method::Get, "/users/{id}", "get_user");
        table.add(Method::Post, "/users", "create_user");

        match table.lookup("/users/42", Method::Get) {
            RouteLookup::Match { route, params } => {
                assert_eq!(*route, "get_user");
                assert_eq!(params[0].0, "id");
                assert_eq!(params[0].1, "42");
            }
            _ => panic!("Expected match"),
        }
    }

    #[test]
    fn route_table_lookup_method_not_allowed() {
        let mut table: RouteTable<&str> = RouteTable::new();
        table.add(Method::Get, "/users", "get_users");
        table.add(Method::Post, "/users", "create_user");

        match table.lookup("/users", Method::Delete) {
            RouteLookup::MethodNotAllowed { allowed } => {
                assert!(allowed.contains(&Method::Get));
                assert!(allowed.contains(&Method::Head));
                assert!(allowed.contains(&Method::Post));
            }
            _ => panic!("Expected MethodNotAllowed"),
        }
    }

    #[test]
    fn route_table_lookup_not_found() {
        let mut table: RouteTable<&str> = RouteTable::new();
        table.add(Method::Get, "/users", "get_users");

        assert!(matches!(
            table.lookup("/items", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn route_table_head_matches_get() {
        let mut table: RouteTable<&str> = RouteTable::new();
        table.add(Method::Get, "/users", "get_users");

        match table.lookup("/users", Method::Head) {
            RouteLookup::Match { route, .. } => {
                assert_eq!(*route, "get_users");
            }
            _ => panic!("Expected match for HEAD on GET route"),
        }
    }

    #[test]
    fn format_allow_header_formats_methods() {
        let methods = vec![Method::Get, Method::Head, Method::Post];
        assert_eq!(format_allow_header(&methods), "GET, HEAD, POST");
    }

    #[test]
    fn options_request_returns_method_not_allowed_with_allowed_methods() {
        let mut table: RouteTable<&str> = RouteTable::new();
        table.add(Method::Get, "/users", "get_users");
        table.add(Method::Post, "/users", "create_user");

        // OPTIONS should return MethodNotAllowed with the allowed methods
        // (The app layer handles converting this to a 204 response)
        match table.lookup("/users", Method::Options) {
            RouteLookup::MethodNotAllowed { allowed } => {
                assert!(allowed.contains(&Method::Get));
                assert!(allowed.contains(&Method::Head));
                assert!(allowed.contains(&Method::Post));
            }
            _ => panic!("Expected MethodNotAllowed for OPTIONS request"),
        }
    }

    #[test]
    fn options_request_on_nonexistent_path_returns_not_found() {
        let mut table: RouteTable<&str> = RouteTable::new();
        table.add(Method::Get, "/users", "get_users");

        match table.lookup("/items", Method::Options) {
            RouteLookup::NotFound => {}
            _ => panic!("Expected NotFound for OPTIONS on non-existent path"),
        }
    }

    #[test]
    fn explicit_options_handler_matches() {
        let mut table: RouteTable<&str> = RouteTable::new();
        table.add(Method::Get, "/api/resource", "get_resource");
        table.add(Method::Options, "/api/resource", "options_resource");

        match table.lookup("/api/resource", Method::Options) {
            RouteLookup::Match { route, .. } => {
                assert_eq!(*route, "options_resource");
            }
            _ => panic!("Expected match for explicit OPTIONS handler"),
        }
    }

    #[test]
    fn method_order_returns_expected_ordering() {
        assert!(method_order(Method::Get) < method_order(Method::Post));
        assert!(method_order(Method::Head) < method_order(Method::Post));
        assert!(method_order(Method::Options) < method_order(Method::Trace));
        assert!(method_order(Method::Delete) < method_order(Method::Options));
    }
}
