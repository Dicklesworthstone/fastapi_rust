//! Route matching result.

use crate::trie::Route;
use fastapi_core::Method;

/// A matched route with extracted parameters.
#[derive(Debug)]
pub struct RouteMatch<'a> {
    /// The matched route.
    pub route: &'a Route,
    /// Extracted path parameters.
    pub params: Vec<(&'a str, &'a str)>,
}

impl<'a> RouteMatch<'a> {
    /// Get a parameter value by name.
    #[must_use]
    pub fn get_param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
    }
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
