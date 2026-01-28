//! Route matching result.

use crate::trie::Route;
use fastapi_core::Method;
use std::num::{ParseFloatError, ParseIntError};

/// A matched route with extracted parameters.
#[derive(Debug)]
pub struct RouteMatch<'a> {
    /// The matched route.
    pub route: &'a Route,
    /// Extracted path parameters.
    pub params: Vec<(&'a str, &'a str)>,
}

impl<'a> RouteMatch<'a> {
    /// Get a parameter value by name as a string slice.
    #[must_use]
    pub fn get_param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
    }

    /// Get a parameter value parsed as an i64 integer.
    ///
    /// Returns `None` if the parameter doesn't exist.
    /// Returns `Some(Err(_))` if the parameter exists but can't be parsed as i64.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Route: /users/{id:int}
    /// if let Some(Ok(id)) = route_match.get_param_int("id") {
    ///     println!("User ID: {id}");
    /// }
    /// ```
    #[must_use]
    pub fn get_param_int(&self, name: &str) -> Option<Result<i64, ParseIntError>> {
        self.get_param(name).map(str::parse)
    }

    /// Get a parameter value parsed as an i32 integer.
    ///
    /// Returns `None` if the parameter doesn't exist.
    /// Returns `Some(Err(_))` if the parameter exists but can't be parsed as i32.
    #[must_use]
    pub fn get_param_i32(&self, name: &str) -> Option<Result<i32, ParseIntError>> {
        self.get_param(name).map(str::parse)
    }

    /// Get a parameter value parsed as a u64 unsigned integer.
    ///
    /// Returns `None` if the parameter doesn't exist.
    /// Returns `Some(Err(_))` if the parameter exists but can't be parsed as u64.
    #[must_use]
    pub fn get_param_u64(&self, name: &str) -> Option<Result<u64, ParseIntError>> {
        self.get_param(name).map(str::parse)
    }

    /// Get a parameter value parsed as a u32 unsigned integer.
    ///
    /// Returns `None` if the parameter doesn't exist.
    /// Returns `Some(Err(_))` if the parameter exists but can't be parsed as u32.
    #[must_use]
    pub fn get_param_u32(&self, name: &str) -> Option<Result<u32, ParseIntError>> {
        self.get_param(name).map(str::parse)
    }

    /// Get a parameter value parsed as an f64 float.
    ///
    /// Returns `None` if the parameter doesn't exist.
    /// Returns `Some(Err(_))` if the parameter exists but can't be parsed as f64.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Route: /values/{val:float}
    /// if let Some(Ok(val)) = route_match.get_param_float("val") {
    ///     println!("Value: {val}");
    /// }
    /// ```
    #[must_use]
    pub fn get_param_float(&self, name: &str) -> Option<Result<f64, ParseFloatError>> {
        self.get_param(name).map(str::parse)
    }

    /// Get a parameter value parsed as an f32 float.
    ///
    /// Returns `None` if the parameter doesn't exist.
    /// Returns `Some(Err(_))` if the parameter exists but can't be parsed as f32.
    #[must_use]
    pub fn get_param_f32(&self, name: &str) -> Option<Result<f32, ParseFloatError>> {
        self.get_param(name).map(str::parse)
    }

    /// Check if a parameter value is a valid UUID format.
    ///
    /// Returns `None` if the parameter doesn't exist.
    /// Returns `Some(true)` if the parameter is a valid UUID.
    /// Returns `Some(false)` if the parameter exists but isn't a valid UUID.
    #[must_use]
    pub fn is_param_uuid(&self, name: &str) -> Option<bool> {
        self.get_param(name).map(is_valid_uuid)
    }

    /// Get parameter count.
    #[must_use]
    pub fn param_count(&self) -> usize {
        self.params.len()
    }

    /// Check if there are no parameters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }

    /// Iterate over all parameters as (name, value) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.params.iter().map(|(n, v)| (*n, *v))
    }
}

/// Check if a string is a valid UUID format (8-4-4-4-12 hex digits).
fn is_valid_uuid(s: &str) -> bool {
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

/// Result of attempting to locate a route by path and method.
#[derive(Debug)]
pub enum RouteLookup<'a> {
    /// A route matched by path and method.
    Match(RouteMatch<'a>),
    /// Path matched, but method is not allowed.
    MethodNotAllowed { allowed: AllowedMethods },
    /// No route matched the path.
    NotFound,
}

/// Allowed methods for a matched path.
#[derive(Debug, Clone)]
pub struct AllowedMethods {
    methods: Vec<Method>,
}

impl AllowedMethods {
    /// Create a normalized allow list.
    ///
    /// - Adds `HEAD` if `GET` is present.
    /// - Sorts and de-duplicates for stable output.
    #[must_use]
    pub fn new(mut methods: Vec<Method>) -> Self {
        if methods.contains(&Method::Get) && !methods.contains(&Method::Head) {
            methods.push(Method::Head);
        }
        methods.sort_by_key(method_order);
        methods.dedup();
        Self { methods }
    }

    /// Access the normalized methods.
    #[must_use]
    pub fn methods(&self) -> &[Method] {
        &self.methods
    }

    /// Check whether a method is allowed.
    #[must_use]
    pub fn contains(&self, method: Method) -> bool {
        self.methods.contains(&method)
    }

    /// Format as an HTTP Allow header value.
    #[must_use]
    pub fn header_value(&self) -> String {
        let mut out = String::new();
        for (idx, method) in self.methods.iter().enumerate() {
            if idx > 0 {
                out.push_str(", ");
            }
            out.push_str(method.as_str());
        }
        out
    }
}

fn method_order(method: &Method) -> u8 {
    match *method {
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
