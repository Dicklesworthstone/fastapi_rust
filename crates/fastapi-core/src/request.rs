//! HTTP request types.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::sync::Mutex;

// Re-export Method from fastapi-types
pub use fastapi_types::Method;

use asupersync::stream::Stream;

/// Error yielded by streaming request bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestBodyStreamError {
    /// The body exceeded the configured maximum size.
    TooLarge { received: usize, max: usize },
    /// The connection closed before the full body was read.
    ConnectionClosed,
    /// An I/O error occurred while reading the body.
    Io(String),
}

impl fmt::Display for RequestBodyStreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { received, max } => write!(
                f,
                "request body too large: received {received} bytes (max {max})"
            ),
            Self::ConnectionClosed => write!(f, "connection closed while reading request body"),
            Self::Io(e) => write!(f, "I/O error while reading request body: {e}"),
        }
    }
}

impl std::error::Error for RequestBodyStreamError {}

/// Streamed request body type (yields chunks or a streaming error).
pub type RequestBodyStream =
    Pin<Box<dyn Stream<Item = Result<Vec<u8>, RequestBodyStreamError>> + Send>>;

/// HTTP protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpVersion {
    /// HTTP/1.0
    Http10,
    /// HTTP/1.1
    Http11,
    /// HTTP/2
    Http2,
}

impl HttpVersion {
    /// Parse an HTTP version string like "HTTP/1.1".
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "HTTP/1.0" => Some(Self::Http10),
            "HTTP/1.1" => Some(Self::Http11),
            "HTTP/2" | "HTTP/2.0" => Some(Self::Http2),
            _ => None,
        }
    }
}

/// Connection metadata supplied by the server (or test harness).
///
/// The core request type does not inherently know whether it arrived over TLS;
/// servers should insert this as an extension when that information is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// True if the connection is using TLS (HTTPS).
    pub is_tls: bool,
}

impl ConnectionInfo {
    /// Plain HTTP connection.
    #[allow(dead_code)]
    pub const HTTP: Self = Self { is_tls: false };
    /// HTTPS connection (TLS).
    #[allow(dead_code)]
    pub const HTTPS: Self = Self { is_tls: true };
}

/// A syntactically validated DNS, IPv4, or bracketed IPv6 request authority.
///
/// Parsing does not authorize a host or establish proxy trust. The server applies
/// its host policy before storing an admitted authority on [`Request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAuthority {
    host: String,
    port: Option<u16>,
    ipv6: bool,
}

impl RequestAuthority {
    /// Parse an authority without userinfo, a path, or an invalid supplied port.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim_matches([' ', '\t']);
        if value.is_empty() || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return None;
        }

        if let Some(bracketed) = value.strip_prefix('[') {
            let (host, rest) = bracketed.split_once(']')?;
            let address = host.parse::<Ipv6Addr>().ok()?;
            let port = if rest.is_empty() {
                None
            } else {
                Some(Self::parse_port(rest.strip_prefix(':')?)?)
            };
            return Some(Self {
                host: address.to_string(),
                port,
                ipv6: true,
            });
        }

        let (host, port) = match value.split_once(':') {
            Some((host, port)) => (host, Some(Self::parse_port(port)?)),
            None => (value, None),
        };
        if host.parse::<Ipv4Addr>().is_err() && !Self::is_valid_hostname(host) {
            return None;
        }
        Some(Self {
            host: host.to_ascii_lowercase(),
            port,
            ipv6: false,
        })
    }

    fn parse_port(port: &str) -> Option<u16> {
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let port = port.parse::<u16>().ok()?;
        (port != 0).then_some(port)
    }

    fn is_valid_hostname(host: &str) -> bool {
        !host.is_empty()
            && host.len() <= 253
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
    }

    /// Return the normalized host without an HTTP port or IPv6 brackets.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Return the explicitly supplied HTTP port, if present.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// Format the host for a URI authority, retaining brackets around IPv6.
    #[must_use]
    pub fn host_for_uri(&self) -> String {
        if self.ipv6 {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
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

    /// Returns true if a header is present (case-insensitive).
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.inner.contains_key(&name.to_ascii_lowercase())
    }

    /// Insert a header.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.inner
            .insert(name.into().to_ascii_lowercase(), value.into());
    }

    /// Insert a header from borrowed name/value slices.
    ///
    /// This is a convenience for parsers that already have `&str`/`&[u8]` and want
    /// to avoid constructing intermediate owned buffers.
    pub fn insert_from_slice(&mut self, name: &str, value: &[u8]) {
        self.inner.insert(name.to_ascii_lowercase(), value.to_vec());
    }

    /// Remove a header value by name (case-insensitive).
    pub fn remove(&mut self, name: &str) -> Option<Vec<u8>> {
        self.inner.remove(&name.to_ascii_lowercase())
    }

    /// Iterate over all headers as (name, value) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.inner
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_slice()))
    }

    /// Returns the number of headers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if there are no headers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Request body.
pub enum Body {
    /// Empty body.
    Empty,
    /// Bytes body.
    Bytes(Vec<u8>),
    /// Streaming body, optionally with a known content length.
    Stream {
        /// Streamed chunks.
        stream: Mutex<RequestBodyStream>,
        /// Known content length, if available.
        content_length: Option<usize>,
    },
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.debug_tuple("Empty").finish(),
            Self::Bytes(b) => f.debug_tuple("Bytes").field(b).finish(),
            Self::Stream { content_length, .. } => f
                .debug_struct("Stream")
                .field("content_length", content_length)
                .finish(),
        }
    }
}

impl Body {
    /// Create a streaming body.
    #[must_use]
    pub fn streaming<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<Vec<u8>, RequestBodyStreamError>> + Send + 'static,
    {
        Self::Stream {
            stream: Mutex::new(Box::pin(stream)),
            content_length: None,
        }
    }

    /// Create a streaming body with a known content length.
    #[must_use]
    pub fn streaming_with_size<S>(stream: S, content_length: usize) -> Self
    where
        S: Stream<Item = Result<Vec<u8>, RequestBodyStreamError>> + Send + 'static,
    {
        Self::Stream {
            stream: Mutex::new(Box::pin(stream)),
            content_length: Some(content_length),
        }
    }

    /// Get body as bytes, consuming it.
    ///
    /// Note: streaming bodies cannot be synchronously collected; this returns
    /// an empty vector for `Body::Stream`.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Empty => Vec::new(),
            Self::Bytes(b) => b,
            Self::Stream { .. } => Vec::new(),
        }
    }

    /// Check if body is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
            || matches!(self, Self::Bytes(b) if b.is_empty())
            || matches!(
                self,
                Self::Stream {
                    content_length: Some(0),
                    ..
                }
            )
    }

    /// Take ownership of the inner stream, if this is a streaming body.
    pub fn into_stream(self) -> Option<(RequestBodyStream, Option<usize>)> {
        match self {
            Self::Stream {
                stream,
                content_length,
            } => Some((
                stream.into_inner().unwrap_or_else(|e| e.into_inner()),
                content_length,
            )),
            _ => None,
        }
    }
}

/// Request-scoped background tasks to execute after the response is sent.
///
/// This is inspired by FastAPI's `BackgroundTasks`. Handlers can enqueue work
/// that is executed by the server after the main response completes.
pub type BackgroundTasksInner = Mutex<Vec<Pin<Box<dyn Future<Output = ()> + Send>>>>;

pub struct BackgroundTasks {
    tasks: BackgroundTasksInner,
}

impl fmt::Debug for BackgroundTasks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackgroundTasks").finish_non_exhaustive()
    }
}

impl BackgroundTasks {
    /// Create an empty background task set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(Vec::new()),
        }
    }

    /// Add a synchronous task to run after the response is written.
    ///
    /// This matches FastAPI's `BackgroundTasks.add_task(...)` UX: you enqueue work
    /// and the server runs it after the response is sent.
    pub fn add<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.add_async(async move { f() });
    }

    /// Add an async task (future) to run after the response is written.
    pub fn add_async<Fut>(&self, fut: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut guard = self
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.push(Box::pin(fut));
    }

    /// Execute all background tasks sequentially.
    pub async fn execute_all(self) {
        let tasks = self
            .tasks
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for t in tasks {
            t.await;
        }
    }
}

impl Default for BackgroundTasks {
    fn default() -> Self {
        Self::new()
    }
}

/// HTTP request.
#[derive(Debug)]
pub struct Request {
    method: Method,
    version: HttpVersion,
    path: String,
    query: Option<String>,
    raw_target: Option<String>,
    admitted_authority: Option<RequestAuthority>,
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
            version: HttpVersion::Http11,
            path: path.into(),
            query: None,
            raw_target: None,
            admitted_authority: None,
            headers: Headers::new(),
            body: Body::Empty,
            extensions: HashMap::new(),
        }
    }

    /// Create a new request with an explicit HTTP version.
    #[must_use]
    pub fn with_version(method: Method, path: impl Into<String>, version: HttpVersion) -> Self {
        let mut req = Self::new(method, path);
        req.version = version;
        req
    }

    /// Get the HTTP method.
    #[must_use]
    pub fn method(&self) -> Method {
        self.method
    }

    /// Get the HTTP version.
    #[must_use]
    pub fn version(&self) -> HttpVersion {
        self.version
    }

    /// Set the HTTP version.
    pub fn set_version(&mut self, version: HttpVersion) {
        self.version = version;
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

    /// Return the original encoded request target supplied by the HTTP parser.
    ///
    /// This is separate from the decoded routing path and unmodified by query
    /// extraction. Its presence does not imply that it is an origin-form URI.
    #[must_use]
    pub fn raw_target(&self) -> Option<&str> {
        self.raw_target.as_deref()
    }

    /// Preserve an original encoded request target without changing routing.
    pub fn set_raw_target(&mut self, target: impl Into<String>) {
        self.raw_target = Some(target.into());
    }

    /// Return the effective authority admitted by the server's host policy.
    #[must_use]
    pub fn admitted_authority(&self) -> Option<&RequestAuthority> {
        self.admitted_authority.as_ref()
    }

    /// Store an effective authority after applying the server's host/proxy policy.
    ///
    /// Applications supplying requests directly must also enforce that policy;
    /// syntactic parsing alone does not authorize a forwarded host.
    pub fn set_admitted_authority(&mut self, authority: RequestAuthority) {
        self.admitted_authority = Some(authority);
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

    /// Insert a typed extension value.
    pub fn insert_extension<T: Any + Send + Sync>(&mut self, value: T) {
        self.extensions.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Get a typed extension value.
    #[must_use]
    pub fn get_extension<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.extensions
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    /// Get a mutable typed extension value.
    pub fn get_extension_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.extensions
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    /// Remove and return a typed extension value.
    pub fn take_extension<T: Any + Send + Sync>(&mut self) -> Option<T> {
        self.extensions
            .remove(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast::<T>().ok())
            .map(|boxed| *boxed)
    }

    /// Access (and lazily create) the request-scoped background tasks container.
    pub fn background_tasks(&mut self) -> &BackgroundTasks {
        if !self
            .extensions
            .contains_key(&TypeId::of::<BackgroundTasks>())
        {
            self.insert_extension(BackgroundTasks::new());
        }
        self.get_extension::<BackgroundTasks>()
            .expect("BackgroundTasks extension should exist")
    }
}

#[cfg(test)]
mod authority_tests {
    use super::*;

    #[test]
    fn authorities_preserve_valid_host_kind_and_explicit_port() {
        for (input, host, port, uri_host) in [
            ("Example.COM", "example.com", None, "example.com"),
            (
                "api.example.com:8080",
                "api.example.com",
                Some(8080),
                "api.example.com",
            ),
            ("127.0.0.1:80", "127.0.0.1", Some(80), "127.0.0.1"),
            (
                "[2001:db8::1]:8080",
                "2001:db8::1",
                Some(8080),
                "[2001:db8::1]",
            ),
            ("[::1]", "::1", None, "[::1]"),
        ] {
            let authority = RequestAuthority::parse(input).expect("valid authority");
            assert_eq!(authority.host(), host, "{input}");
            assert_eq!(authority.port(), port, "{input}");
            assert_eq!(authority.host_for_uri(), uri_host, "{input}");
        }
    }

    #[test]
    fn authority_rejects_userinfo_malformed_hosts_and_supplied_ports() {
        for input in [
            "",
            "user@example.com",
            "good.example@evil.example",
            "example.com/path",
            "example.com\\evil",
            "example.com?x=1",
            "example.com#fragment",
            "bad host",
            "example.com\r\n",
            "example.com:",
            "example.com:bad",
            "example.com:0",
            "example.com:65536",
            "example.com:80:90",
            "[::1",
            "[::1]suffix",
            "[::1]:",
            "[::1]:bad",
            "[::1]:0",
            "[::1]:65536",
            "::1",
            "-bad.example",
        ] {
            assert!(
                RequestAuthority::parse(input).is_none(),
                "accepted {input:?}"
            );
        }
    }

    #[test]
    fn original_target_does_not_replace_decoded_routing_path_or_query() {
        let mut request = Request::new(Method::Get, "/a?part#fragment/end");
        request.set_query(Some("tag=a&tag=b".to_string()));
        request.set_raw_target("/a%3Fpart%23fragment%2Fend?tag=a&tag=b");
        assert_eq!(request.path(), "/a?part#fragment/end");
        assert_eq!(request.query(), Some("tag=a&tag=b"));
        assert_eq!(
            request.raw_target(),
            Some("/a%3Fpart%23fragment%2Fend?tag=a&tag=b")
        );
    }
}
