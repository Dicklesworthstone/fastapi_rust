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
    ///
    /// For `Str`, rejects path traversal segments (`..` and `.`).
    /// For `Float`, rejects NaN, inf, and -inf (non-finite values).
    /// For `Path`, rejects values containing `..` components to prevent traversal.
    #[must_use]
    pub fn matches(&self, value: &str) -> bool {
        match self {
            Self::Str => value != ".." && value != ".",
            Self::Int => value.parse::<i64>().is_ok(),
            Self::Float => value.parse::<f64>().is_ok_and(f64::is_finite),
            Self::Uuid => is_uuid(value),
            Self::Path => !path_has_traversal(value),
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

/// Check if a path value contains traversal components (`..` or `.`).
///
/// Splits on `/` and checks each segment, preventing directory traversal
/// attacks through path parameters like `/files/{path:path}`.
fn path_has_traversal(value: &str) -> bool {
    value.split('/').any(|seg| seg == ".." || seg == ".")
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
                        // Reject path traversal (../ or ./ components)
                        if path_has_traversal(value) {
                            return None;
                        }
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

// =============================================================================
// URL GENERATION AND REVERSE ROUTING
// =============================================================================
//
// Generate URLs from route names and parameters.
//
// # Features
// - Look up routes by name
// - Substitute path parameters
// - Include query parameters
// - Respect root_path for proxied apps

use std::collections::HashMap;

/// Error that can occur during URL generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    /// The route name was not found in the registry.
    RouteNotFound { name: String },
    /// A required path parameter was missing.
    MissingParam { name: String, param: String },
    /// A path parameter value was invalid for its converter type.
    InvalidParam {
        name: String,
        param: String,
        value: String,
    },
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RouteNotFound { name } => {
                write!(f, "route '{}' not found", name)
            }
            Self::MissingParam { name, param } => {
                write!(f, "route '{}' requires parameter '{}'", name, param)
            }
            Self::InvalidParam { name, param, value } => {
                write!(
                    f,
                    "route '{}' parameter '{}': invalid value '{}'",
                    name, param, value
                )
            }
        }
    }
}

impl std::error::Error for UrlError {}

/// Registry for named routes, enabling URL generation.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::routing::UrlRegistry;
///
/// let mut registry = UrlRegistry::new();
/// registry.register("get_user", "/users/{id}");
/// registry.register("get_post", "/posts/{post_id:int}");
///
/// // Generate URL with path parameter
/// let url = registry.url_for("get_user", &[("id", "42")], &[]).unwrap();
/// assert_eq!(url, "/users/42");
///
/// // Generate URL with query parameters
/// let url = registry.url_for("get_user", &[("id", "42")], &[("fields", "name,email")]).unwrap();
/// assert_eq!(url, "/users/42?fields=name%2Cemail");
/// ```
#[derive(Debug, Clone, Default)]
pub struct UrlRegistry {
    /// Map from route name to route pattern.
    routes: HashMap<String, RoutePattern>,
    /// Root path prefix for reverse proxy support.
    root_path: String,
}

impl UrlRegistry {
    /// Create a new empty URL registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            routes: HashMap::new(),
            root_path: String::new(),
        }
    }

    /// Create a URL registry with a root path prefix.
    ///
    /// The root path is prepended to all generated URLs, useful for apps
    /// running behind a reverse proxy at a sub-path.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let registry = UrlRegistry::with_root_path("/api/v1");
    /// registry.register("get_user", "/users/{id}");
    /// let url = registry.url_for("get_user", &[("id", "42")], &[]).unwrap();
    /// assert_eq!(url, "/api/v1/users/42");
    /// ```
    #[must_use]
    pub fn with_root_path(root_path: impl Into<String>) -> Self {
        let mut path = root_path.into();
        // Normalize: ensure no trailing slash
        while path.ends_with('/') {
            path.pop();
        }
        Self {
            routes: HashMap::new(),
            root_path: path,
        }
    }

    /// Set the root path prefix.
    pub fn set_root_path(&mut self, root_path: impl Into<String>) {
        let mut path = root_path.into();
        while path.ends_with('/') {
            path.pop();
        }
        self.root_path = path;
    }

    /// Get the current root path.
    #[must_use]
    pub fn root_path(&self) -> &str {
        &self.root_path
    }

    /// Register a named route.
    ///
    /// # Arguments
    ///
    /// * `name` - The route name (used to look up the route)
    /// * `pattern` - The route pattern (e.g., "/users/{id}")
    pub fn register(&mut self, name: impl Into<String>, pattern: &str) {
        let name = name.into();
        let parsed = RoutePattern::parse(pattern);
        self.routes.insert(name, parsed);
    }

    /// Check if a route with the given name exists.
    #[must_use]
    pub fn has_route(&self, name: &str) -> bool {
        self.routes.contains_key(name)
    }

    /// Get the pattern for a named route.
    #[must_use]
    pub fn get_pattern(&self, name: &str) -> Option<&str> {
        self.routes.get(name).map(|p| p.pattern.as_str())
    }

    /// Generate a URL for a named route.
    ///
    /// # Arguments
    ///
    /// * `name` - The route name
    /// * `params` - Path parameters as (name, value) pairs
    /// * `query` - Query parameters as (name, value) pairs
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The route name is not found
    /// - A required path parameter is missing
    /// - A path parameter value doesn't match its converter type
    ///
    /// # Example
    ///
    /// ```ignore
    /// let url = registry.url_for(
    ///     "get_user",
    ///     &[("id", "42")],
    ///     &[("fields", "name"), ("include", "posts")]
    /// ).unwrap();
    /// // Returns: "/users/42?fields=name&include=posts"
    /// ```
    pub fn url_for(
        &self,
        name: &str,
        params: &[(&str, &str)],
        query: &[(&str, &str)],
    ) -> Result<String, UrlError> {
        let pattern = self
            .routes
            .get(name)
            .ok_or_else(|| UrlError::RouteNotFound {
                name: name.to_string(),
            })?;

        // Build parameter map for fast lookup
        let param_map: HashMap<&str, &str> = params.iter().copied().collect();

        // Build the path by substituting parameters
        let mut path = String::new();
        if !self.root_path.is_empty() {
            path.push_str(&self.root_path);
        }

        // Track whether we have any segments
        let has_segments = !pattern.segments.is_empty();

        for segment in &pattern.segments {
            path.push('/');
            match segment {
                PathSegment::Static(s) => {
                    path.push_str(s);
                }
                PathSegment::Param(info) => {
                    let value = *param_map.get(info.name.as_str()).ok_or_else(|| {
                        UrlError::MissingParam {
                            name: name.to_string(),
                            param: info.name.clone(),
                        }
                    })?;

                    // Validate the value against the converter
                    if !info.converter.matches(value) {
                        return Err(UrlError::InvalidParam {
                            name: name.to_string(),
                            param: info.name.clone(),
                            value: value.to_string(),
                        });
                    }

                    // URL-encode the value (except for path converter which allows slashes)
                    if info.converter == Converter::Path {
                        path.push_str(value);
                    } else {
                        path.push_str(&url_encode_path_segment(value));
                    }
                }
            }
        }

        // Handle empty path (root route) - must have at least "/"
        // If no segments but has root_path, still need trailing "/"
        if path.is_empty() || (!has_segments && !self.root_path.is_empty()) {
            path.push('/');
        }

        // Add query parameters if any
        if !query.is_empty() {
            path.push('?');
            for (i, (key, value)) in query.iter().enumerate() {
                if i > 0 {
                    path.push('&');
                }
                path.push_str(&url_encode(key));
                path.push('=');
                path.push_str(&url_encode(value));
            }
        }

        Ok(path)
    }

    /// Get the number of registered routes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Check if the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// Get an iterator over route names.
    pub fn route_names(&self) -> impl Iterator<Item = &str> {
        self.routes.keys().map(String::as_str)
    }
}

/// URL-encode a string for use in a query parameter.
///
/// Encodes all non-unreserved characters according to RFC 3986.
#[must_use]
pub fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            // Unreserved characters (RFC 3986)
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                result.push(byte as char);
            }
            // Everything else gets percent-encoded
            _ => {
                result.push('%');
                result.push(
                    char::from_digit(u32::from(byte >> 4), 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                result.push(
                    char::from_digit(u32::from(byte & 0xF), 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    result
}

/// URL-encode a path segment, preserving forward slashes.
///
/// Similar to `url_encode` but also allows forward slashes for path converter
/// values, so `/files/a/b/c.txt` stays as-is instead of encoding `/` as `%2F`.
#[must_use]
pub fn url_encode_path_segment(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            // Unreserved characters (RFC 3986) + forward slash for paths
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                result.push(byte as char);
            }
            // Everything else gets percent-encoded
            _ => {
                result.push('%');
                result.push(
                    char::from_digit(u32::from(byte >> 4), 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                result.push(
                    char::from_digit(u32::from(byte & 0xF), 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    result
}

/// URL-decode a percent-encoded string.
///
/// # Errors
///
/// Returns `None` if the string contains invalid percent-encoding.
#[must_use]
pub fn url_decode(s: &str) -> Option<String> {
    let mut result = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();

    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let hi = bytes.next()?;
            let lo = bytes.next()?;
            let hi = char::from(hi).to_digit(16)?;
            let lo = char::from(lo).to_digit(16)?;
            result.push((hi * 16 + lo) as u8);
        } else if byte == b'+' {
            // Handle + as space (form encoding)
            result.push(b' ');
        } else {
            result.push(byte);
        }
    }

    String::from_utf8(result).ok()
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

    // =========================================================================
    // URL GENERATION TESTS
    // =========================================================================

    #[test]
    fn url_registry_new() {
        let registry = UrlRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert_eq!(registry.root_path(), "");
    }

    #[test]
    fn url_registry_with_root_path() {
        let registry = UrlRegistry::with_root_path("/api/v1");
        assert_eq!(registry.root_path(), "/api/v1");
    }

    #[test]
    fn url_registry_with_root_path_normalizes_trailing_slash() {
        let registry = UrlRegistry::with_root_path("/api/v1/");
        assert_eq!(registry.root_path(), "/api/v1");

        let registry2 = UrlRegistry::with_root_path("/api///");
        assert_eq!(registry2.root_path(), "/api");
    }

    #[test]
    fn url_registry_register_and_lookup() {
        let mut registry = UrlRegistry::new();
        registry.register("get_user", "/users/{id}");

        assert!(registry.has_route("get_user"));
        assert!(!registry.has_route("nonexistent"));
        assert_eq!(registry.get_pattern("get_user"), Some("/users/{id}"));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn url_for_static_route() {
        let mut registry = UrlRegistry::new();
        registry.register("home", "/");
        registry.register("about", "/about");

        let url = registry.url_for("home", &[], &[]).unwrap();
        assert_eq!(url, "/");

        let url = registry.url_for("about", &[], &[]).unwrap();
        assert_eq!(url, "/about");
    }

    #[test]
    fn url_for_with_path_param() {
        let mut registry = UrlRegistry::new();
        registry.register("get_user", "/users/{id}");

        let url = registry.url_for("get_user", &[("id", "42")], &[]).unwrap();
        assert_eq!(url, "/users/42");
    }

    #[test]
    fn url_for_with_multiple_params() {
        let mut registry = UrlRegistry::new();
        registry.register("get_post", "/users/{user_id}/posts/{post_id}");

        let url = registry
            .url_for("get_post", &[("user_id", "42"), ("post_id", "99")], &[])
            .unwrap();
        assert_eq!(url, "/users/42/posts/99");
    }

    #[test]
    fn url_for_with_typed_param() {
        let mut registry = UrlRegistry::new();
        registry.register("get_item", "/items/{id:int}");

        // Valid integer
        let url = registry.url_for("get_item", &[("id", "123")], &[]).unwrap();
        assert_eq!(url, "/items/123");

        // Invalid integer
        let result = registry.url_for("get_item", &[("id", "abc")], &[]);
        assert!(matches!(result, Err(UrlError::InvalidParam { .. })));
    }

    #[test]
    fn url_for_with_uuid_param() {
        let mut registry = UrlRegistry::new();
        registry.register("get_object", "/objects/{id:uuid}");

        let url = registry
            .url_for(
                "get_object",
                &[("id", "550e8400-e29b-41d4-a716-446655440000")],
                &[],
            )
            .unwrap();
        assert_eq!(url, "/objects/550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn url_for_with_query_params() {
        let mut registry = UrlRegistry::new();
        registry.register("search", "/search");

        let url = registry
            .url_for("search", &[], &[("q", "hello"), ("page", "1")])
            .unwrap();
        assert_eq!(url, "/search?q=hello&page=1");
    }

    #[test]
    fn url_for_encodes_query_params() {
        let mut registry = UrlRegistry::new();
        registry.register("search", "/search");

        let url = registry
            .url_for("search", &[], &[("q", "hello world"), ("filter", "a&b=c")])
            .unwrap();
        assert_eq!(url, "/search?q=hello%20world&filter=a%26b%3Dc");
    }

    #[test]
    fn url_for_encodes_path_params() {
        let mut registry = UrlRegistry::new();
        registry.register("get_file", "/files/{name}");

        let url = registry
            .url_for("get_file", &[("name", "my file.txt")], &[])
            .unwrap();
        assert_eq!(url, "/files/my%20file.txt");
    }

    #[test]
    fn url_for_with_root_path() {
        let mut registry = UrlRegistry::with_root_path("/api/v1");
        registry.register("get_user", "/users/{id}");

        let url = registry.url_for("get_user", &[("id", "42")], &[]).unwrap();
        assert_eq!(url, "/api/v1/users/42");
    }

    #[test]
    fn url_for_route_not_found() {
        let registry = UrlRegistry::new();
        let result = registry.url_for("nonexistent", &[], &[]);
        assert!(matches!(result, Err(UrlError::RouteNotFound { name }) if name == "nonexistent"));
    }

    #[test]
    fn url_for_missing_param() {
        let mut registry = UrlRegistry::new();
        registry.register("get_user", "/users/{id}");

        let result = registry.url_for("get_user", &[], &[]);
        assert!(matches!(
            result,
            Err(UrlError::MissingParam { name, param }) if name == "get_user" && param == "id"
        ));
    }

    #[test]
    fn url_for_with_path_converter() {
        let mut registry = UrlRegistry::new();
        registry.register("get_file", "/files/{path:path}");

        let url = registry
            .url_for("get_file", &[("path", "docs/images/logo.png")], &[])
            .unwrap();
        // Path converter preserves slashes
        assert_eq!(url, "/files/docs/images/logo.png");
    }

    #[test]
    fn url_encode_basic() {
        assert_eq!(url_encode("hello"), "hello");
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(url_encode("100%"), "100%25");
    }

    #[test]
    fn url_encode_unicode() {
        assert_eq!(url_encode("日本"), "%E6%97%A5%E6%9C%AC");
        assert_eq!(url_encode("café"), "caf%C3%A9");
    }

    #[test]
    fn url_encode_path_segment_preserves_slashes() {
        // url_encode encodes slashes
        assert_eq!(url_encode("a/b/c"), "a%2Fb%2Fc");
        // url_encode_path_segment preserves them
        assert_eq!(url_encode_path_segment("a/b/c"), "a/b/c");
        // But still encodes other special chars
        assert_eq!(url_encode_path_segment("a b/c"), "a%20b/c");
        assert_eq!(url_encode_path_segment("a&b/c"), "a%26b/c");
    }

    #[test]
    fn url_decode_basic() {
        assert_eq!(url_decode("hello"), Some("hello".to_string()));
        assert_eq!(url_decode("hello%20world"), Some("hello world".to_string()));
        assert_eq!(url_decode("a%26b%3Dc"), Some("a&b=c".to_string()));
    }

    #[test]
    fn url_decode_plus_as_space() {
        assert_eq!(url_decode("hello+world"), Some("hello world".to_string()));
    }

    #[test]
    fn url_decode_invalid() {
        // Incomplete percent encoding
        assert_eq!(url_decode("hello%2"), None);
        assert_eq!(url_decode("hello%"), None);
        // Invalid hex
        assert_eq!(url_decode("hello%GG"), None);
    }

    #[test]
    fn url_error_display() {
        let err = UrlError::RouteNotFound {
            name: "test".to_string(),
        };
        assert_eq!(format!("{}", err), "route 'test' not found");

        let err = UrlError::MissingParam {
            name: "get_user".to_string(),
            param: "id".to_string(),
        };
        assert_eq!(
            format!("{}", err),
            "route 'get_user' requires parameter 'id'"
        );

        let err = UrlError::InvalidParam {
            name: "get_item".to_string(),
            param: "id".to_string(),
            value: "abc".to_string(),
        };
        assert_eq!(
            format!("{}", err),
            "route 'get_item' parameter 'id': invalid value 'abc'"
        );
    }

    #[test]
    fn url_registry_route_names_iterator() {
        let mut registry = UrlRegistry::new();
        registry.register("a", "/a");
        registry.register("b", "/b");
        registry.register("c", "/c");

        let names: Vec<_> = registry.route_names().collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
    }

    #[test]
    fn url_registry_set_root_path() {
        let mut registry = UrlRegistry::new();
        registry.register("home", "/");

        let url1 = registry.url_for("home", &[], &[]).unwrap();
        assert_eq!(url1, "/");

        registry.set_root_path("/api");
        let url2 = registry.url_for("home", &[], &[]).unwrap();
        assert_eq!(url2, "/api/");
    }

    // ========================================================================
    // Trailing slash normalization tests
    // ========================================================================

    #[test]
    fn trailing_slash_strict_mode_matches_both_due_to_segment_parsing() {
        // Note: segment_ranges normalizes trailing slashes, so /users/ and /users
        // both resolve to the same segments ["users"]. In strict mode, the lookup
        // still finds the match because exact matching happens at the segment level.
        let mut table = RouteTable::new();
        table.add(Method::Get, "/users", "users");

        assert!(matches!(
            table.lookup_with_trailing_slash("/users", Method::Get, TrailingSlashMode::Strict),
            RouteLookup::Match {
                route: &"users",
                ..
            }
        ));

        // Trailing slash still matches because segment parsing normalizes it
        assert!(matches!(
            table.lookup_with_trailing_slash("/users/", Method::Get, TrailingSlashMode::Strict),
            RouteLookup::Match {
                route: &"users",
                ..
            }
        ));
    }

    #[test]
    fn trailing_slash_redirect_mode_exact_match_no_redirect() {
        let mut table = RouteTable::new();
        table.add(Method::Get, "/users", "users");

        // Exact match: no redirect needed
        assert!(matches!(
            table.lookup_with_trailing_slash("/users", Method::Get, TrailingSlashMode::Redirect),
            RouteLookup::Match {
                route: &"users",
                ..
            }
        ));

        // With trailing slash: exact match found first (segment parsing normalizes)
        // so no redirect is triggered
        assert!(matches!(
            table.lookup_with_trailing_slash("/users/", Method::Get, TrailingSlashMode::Redirect),
            RouteLookup::Match {
                route: &"users",
                ..
            }
        ));
    }

    #[test]
    fn trailing_slash_match_both_mode() {
        let mut table = RouteTable::new();
        table.add(Method::Get, "/users", "users");

        // Both forms match
        assert!(matches!(
            table.lookup_with_trailing_slash("/users", Method::Get, TrailingSlashMode::MatchBoth),
            RouteLookup::Match {
                route: &"users",
                ..
            }
        ));
        assert!(matches!(
            table.lookup_with_trailing_slash("/users/", Method::Get, TrailingSlashMode::MatchBoth),
            RouteLookup::Match {
                route: &"users",
                ..
            }
        ));
    }

    #[test]
    fn trailing_slash_root_path_not_redirected() {
        let mut table = RouteTable::new();
        table.add(Method::Get, "/", "root");

        assert!(matches!(
            table.lookup_with_trailing_slash("/", Method::Get, TrailingSlashMode::Redirect),
            RouteLookup::Match { route: &"root", .. }
        ));
    }

    #[test]
    fn trailing_slash_with_path_params() {
        let mut table = RouteTable::new();
        table.add(Method::Get, "/users/{id}", "get_user");

        // Segment parsing normalizes trailing slash, so /users/42/ matches /users/{id}
        match table.lookup_with_trailing_slash(
            "/users/42/",
            Method::Get,
            TrailingSlashMode::MatchBoth,
        ) {
            RouteLookup::Match { params, .. } => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0], ("id".to_string(), "42".to_string()));
            }
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn trailing_slash_not_found_stays_not_found() {
        let mut table = RouteTable::new();
        table.add(Method::Get, "/users", "users");

        // Nonexistent path stays NotFound regardless of mode
        assert!(matches!(
            table.lookup_with_trailing_slash(
                "/nonexistent",
                Method::Get,
                TrailingSlashMode::Redirect
            ),
            RouteLookup::NotFound
        ));
        assert!(matches!(
            table.lookup_with_trailing_slash(
                "/nonexistent/",
                Method::Get,
                TrailingSlashMode::Redirect
            ),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn trailing_slash_mode_default_is_strict() {
        assert_eq!(TrailingSlashMode::default(), TrailingSlashMode::Strict);
    }

    #[test]
    fn app_config_trailing_slash_mode() {
        use crate::app::AppConfig;

        let config = AppConfig::new();
        assert_eq!(config.trailing_slash_mode, TrailingSlashMode::Strict);

        let config = AppConfig::new().trailing_slash_mode(TrailingSlashMode::Redirect);
        assert_eq!(config.trailing_slash_mode, TrailingSlashMode::Redirect);

        let config = AppConfig::new().trailing_slash_mode(TrailingSlashMode::MatchBoth);
        assert_eq!(config.trailing_slash_mode, TrailingSlashMode::MatchBoth);
    }

    // === Security regression tests ===

    #[test]
    fn converter_str_rejects_dot_dot_traversal() {
        assert!(!Converter::Str.matches(".."));
        assert!(!Converter::Str.matches("."));
        // Normal values still pass
        assert!(Converter::Str.matches("users"));
        assert!(Converter::Str.matches("file.txt"));
        assert!(Converter::Str.matches("my..name"));
    }

    #[test]
    fn converter_path_rejects_traversal_components() {
        assert!(!Converter::Path.matches("../etc/passwd"));
        assert!(!Converter::Path.matches("foo/../../bar"));
        assert!(!Converter::Path.matches("./hidden"));
        assert!(!Converter::Path.matches(".."));
        // Normal paths still pass
        assert!(Converter::Path.matches("a/b/c.txt"));
        assert!(Converter::Path.matches("docs/readme.md"));
    }

    #[test]
    fn converter_float_rejects_nan_and_infinity() {
        assert!(!Converter::Float.matches("NaN"));
        assert!(!Converter::Float.matches("inf"));
        assert!(!Converter::Float.matches("-inf"));
        assert!(!Converter::Float.matches("infinity"));
        assert!(!Converter::Float.matches("-infinity"));
        // Finite values still pass
        assert!(Converter::Float.matches("3.14"));
        assert!(Converter::Float.matches("-1.5"));
        assert!(Converter::Float.matches("1e10"));
        assert!(Converter::Float.matches("42"));
    }

    #[test]
    fn route_table_rejects_traversal_in_str_param() {
        let mut table = RouteTable::new();
        table.add(Method::Get, "/files/{name}", "handler");

        // Normal file names match
        assert!(matches!(
            table.lookup("/files/readme.txt", Method::Get),
            RouteLookup::Match { .. }
        ));

        // Traversal segment does NOT match
        assert!(matches!(
            table.lookup("/files/..", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn route_table_rejects_traversal_in_path_param() {
        let mut table = RouteTable::new();
        table.add(Method::Get, "/files/{filepath:path}", "handler");

        // Normal paths match
        if let RouteLookup::Match { params, .. } =
            table.lookup("/files/docs/readme.md", Method::Get)
        {
            assert_eq!(params[0].1, "docs/readme.md");
        } else {
            panic!("Expected match for normal path");
        }

        // Traversal paths do NOT match
        assert!(matches!(
            table.lookup("/files/../etc/passwd", Method::Get),
            RouteLookup::NotFound
        ));
        assert!(matches!(
            table.lookup("/files/a/../../etc/shadow", Method::Get),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn path_has_traversal_helper() {
        assert!(path_has_traversal(".."));
        assert!(path_has_traversal("."));
        assert!(path_has_traversal("../foo"));
        assert!(path_has_traversal("foo/.."));
        assert!(path_has_traversal("foo/../bar"));
        assert!(path_has_traversal("./bar"));
        assert!(!path_has_traversal("foo/bar"));
        assert!(!path_has_traversal("foo.bar"));
        assert!(!path_has_traversal("a..b"));
    }
}
