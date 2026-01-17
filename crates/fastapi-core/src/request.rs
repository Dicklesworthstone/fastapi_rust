//! HTTP request types.

use std::collections::HashMap;

/// HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    /// GET method.
    Get,
    /// POST method.
    Post,
    /// PUT method.
    Put,
    /// DELETE method.
    Delete,
    /// PATCH method.
    Patch,
    /// OPTIONS method.
    Options,
    /// HEAD method.
    Head,
    /// TRACE method.
    Trace,
}

impl Method {
    /// Parse method from bytes.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b"GET" => Some(Self::Get),
            b"POST" => Some(Self::Post),
            b"PUT" => Some(Self::Put),
            b"DELETE" => Some(Self::Delete),
            b"PATCH" => Some(Self::Patch),
            b"OPTIONS" => Some(Self::Options),
            b"HEAD" => Some(Self::Head),
            b"TRACE" => Some(Self::Trace),
            _ => None,
        }
    }
}

/// HTTP headers collection.
#[derive(Debug, Default)]
pub struct Headers {
    inner: HashMap<String, Vec<u8>>,
}

impl Headers {
    /// Create empty headers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a header value by name (case-insensitive).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.inner
            .get(&name.to_ascii_lowercase())
            .map(Vec::as_slice)
    }

    /// Insert a header.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.inner
            .insert(name.into().to_ascii_lowercase(), value.into());
    }
}

/// Request body.
#[derive(Debug)]
pub enum Body {
    /// Empty body.
    Empty,
    /// Bytes body.
    Bytes(Vec<u8>),
    // TODO: Stream variant for large bodies
}

impl Body {
    /// Get body as bytes, consuming it.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Empty => Vec::new(),
            Self::Bytes(b) => b,
        }
    }

    /// Check if body is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty) || matches!(self, Self::Bytes(b) if b.is_empty())
    }
}

/// HTTP request.
#[derive(Debug)]
pub struct Request {
    method: Method,
    path: String,
    query: Option<String>,
    headers: Headers,
    body: Body,
    // Extensions for middleware/extractors
    #[allow(dead_code)] // Used in future implementation
    extensions: HashMap<std::any::TypeId, Box<dyn std::any::Any + Send + Sync>>,
}

impl Request {
    /// Create a new request.
    #[must_use]
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: None,
            headers: Headers::new(),
            body: Body::Empty,
            extensions: HashMap::new(),
        }
    }

    /// Get the HTTP method.
    #[must_use]
    pub fn method(&self) -> Method {
        self.method
    }

    /// Get the request path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Get the query string.
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// Get the headers.
    #[must_use]
    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    /// Get mutable headers.
    pub fn headers_mut(&mut self) -> &mut Headers {
        &mut self.headers
    }

    /// Get the body.
    #[must_use]
    pub fn body(&self) -> &Body {
        &self.body
    }

    /// Take the body, replacing with Empty.
    pub fn take_body(&mut self) -> Body {
        std::mem::replace(&mut self.body, Body::Empty)
    }

    /// Set the body.
    pub fn set_body(&mut self, body: Body) {
        self.body = body;
    }

    /// Set the query string.
    pub fn set_query(&mut self, query: Option<String>) {
        self.query = query;
    }
}
