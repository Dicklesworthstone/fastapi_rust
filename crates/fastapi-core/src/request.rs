//! HTTP request types.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

/// HTTP version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HttpVersion {
    /// HTTP/1.0
    Http10,
    /// HTTP/1.1 (default)
    #[default]
    Http11,
}

impl HttpVersion {
    /// Parse HTTP version from string.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "HTTP/1.0" => Some(Self::Http10),
            "HTTP/1.1" => Some(Self::Http11),
            _ => None,
        }
    }

    /// Returns true if this is HTTP/1.1.
    #[must_use]
    pub fn is_http11(self) -> bool {
        matches!(self, Self::Http11)
    }

    /// Returns true if this is HTTP/1.0.
    #[must_use]
    pub fn is_http10(self) -> bool {
        matches!(self, Self::Http10)
    }

    /// Returns the version string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http10 => "HTTP/1.0",
            Self::Http11 => "HTTP/1.1",
        }
    }
}

impl std::str::FromStr for HttpVersion {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or(())
    }
}

impl fmt::Display for HttpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

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

    /// Return the canonical uppercase method name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
            Self::Options => "OPTIONS",
            Self::Head => "HEAD",
            Self::Trace => "TRACE",
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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

    /// Remove a header by name (case-insensitive).
    ///
    /// Returns the removed value, if any.
    pub fn remove(&mut self, name: &str) -> Option<Vec<u8>> {
        self.inner.remove(&name.to_ascii_lowercase())
    }

    /// Check if a header exists (case-insensitive).
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.inner.contains_key(&name.to_ascii_lowercase())
    }
}

/// Request body.
#[derive(Debug)]
pub enum Body {
    /// Empty body.
    Empty,
    /// Bytes body.
    Bytes(Vec<u8>),
    /// Streaming body for large uploads.
    ///
    /// This variant enables memory-efficient handling of large request bodies
    /// by yielding chunks incrementally rather than buffering the entire content.
    ///
    /// The stream yields `Result<Vec<u8>, RequestBodyStreamError>` chunks.
    Stream(RequestBodyStream),
}

/// Error type for streaming body operations.
#[derive(Debug)]
pub enum RequestBodyStreamError {
    /// Connection was closed before body was complete.
    ConnectionClosed,
    /// Timeout while waiting for body data.
    Timeout,
    /// Body exceeded configured size limit.
    TooLarge { received: usize, max: usize },
    /// I/O error during streaming.
    Io(String),
}

impl std::fmt::Display for RequestBodyStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionClosed => write!(f, "connection closed"),
            Self::Timeout => write!(f, "timeout waiting for body data"),
            Self::TooLarge { received, max } => {
                write!(f, "body too large: {received} bytes exceeds limit of {max}")
            }
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
        }
    }
}

impl std::error::Error for RequestBodyStreamError {}

/// A streaming request body.
///
/// This provides an async interface for reading request body chunks
/// without buffering the entire body in memory.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{Body, RequestBodyStream};
///
/// async fn handle_upload(body: Body) -> Vec<u8> {
///     match body {
///         Body::Stream(mut stream) => {
///             let mut buffer = Vec::new();
///             while let Some(chunk) = stream.next().await {
///                 buffer.extend_from_slice(&chunk?);
///             }
///             buffer
///         }
///         Body::Bytes(bytes) => bytes,
///         Body::Empty => Vec::new(),
///     }
/// }
/// ```
pub struct RequestBodyStream {
    /// The inner stream of chunks.
    inner: std::pin::Pin<
        Box<
            dyn asupersync::stream::Stream<Item = Result<Vec<u8>, RequestBodyStreamError>>
                + Send
                + Sync,
        >,
    >,
    /// Total bytes received so far.
    bytes_received: usize,
    /// Expected total size (from Content-Length), if known.
    expected_size: Option<usize>,
    /// Whether the stream is complete.
    complete: bool,
}

impl std::fmt::Debug for RequestBodyStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestBodyStream")
            .field("bytes_received", &self.bytes_received)
            .field("expected_size", &self.expected_size)
            .field("complete", &self.complete)
            .finish_non_exhaustive()
    }
}

impl RequestBodyStream {
    /// Create a new body stream from an async stream of chunks.
    pub fn new<S>(stream: S) -> Self
    where
        S: asupersync::stream::Stream<Item = Result<Vec<u8>, RequestBodyStreamError>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            inner: Box::pin(stream),
            bytes_received: 0,
            expected_size: None,
            complete: false,
        }
    }

    /// Create a body stream with a known expected size.
    pub fn with_expected_size<S>(stream: S, expected_size: usize) -> Self
    where
        S: asupersync::stream::Stream<Item = Result<Vec<u8>, RequestBodyStreamError>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            inner: Box::pin(stream),
            bytes_received: 0,
            expected_size: Some(expected_size),
            complete: false,
        }
    }

    /// Returns the number of bytes received so far.
    #[must_use]
    pub fn bytes_received(&self) -> usize {
        self.bytes_received
    }

    /// Returns the expected total size, if known.
    #[must_use]
    pub fn expected_size(&self) -> Option<usize> {
        self.expected_size
    }

    /// Returns true if the stream is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Collect all chunks into a single buffer.
    ///
    /// This consumes the stream and buffers the entire body in memory.
    /// Use this for small bodies or when the full content is needed.
    ///
    /// For large bodies, prefer processing chunks individually via `next()`.
    pub async fn collect(mut self) -> Result<Vec<u8>, RequestBodyStreamError> {
        use asupersync::stream::StreamExt;

        let capacity = self.expected_size.unwrap_or(4096);
        let mut buffer = Vec::with_capacity(capacity);

        while let Some(chunk) = self.inner.next().await {
            buffer.extend_from_slice(&chunk?);
            self.bytes_received = buffer.len();
        }

        self.complete = true;
        Ok(buffer)
    }
}

impl asupersync::stream::Stream for RequestBodyStream {
    type Item = Result<Vec<u8>, RequestBodyStreamError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.complete {
            return std::task::Poll::Ready(None);
        }

        match self.inner.as_mut().poll_next(cx) {
            std::task::Poll::Ready(Some(Ok(chunk))) => {
                self.bytes_received += chunk.len();
                std::task::Poll::Ready(Some(Ok(chunk)))
            }
            std::task::Poll::Ready(Some(Err(e))) => std::task::Poll::Ready(Some(Err(e))),
            std::task::Poll::Ready(None) => {
                self.complete = true;
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl Body {
    /// Get body as bytes, consuming it.
    ///
    /// For `Body::Stream`, this will panic. Use `into_bytes_async()` instead
    /// for streaming bodies, or check with `is_streaming()` first.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Empty => Vec::new(),
            Self::Bytes(b) => b,
            Self::Stream(_) => panic!(
                "cannot synchronously convert streaming body to bytes; use into_bytes_async()"
            ),
        }
    }

    /// Get body as bytes asynchronously, consuming it.
    ///
    /// This works for all body types:
    /// - `Empty` returns an empty Vec
    /// - `Bytes` returns the bytes
    /// - `Stream` collects all chunks into a Vec
    ///
    /// # Errors
    ///
    /// Returns an error if the stream encounters an error while reading.
    pub async fn into_bytes_async(self) -> Result<Vec<u8>, RequestBodyStreamError> {
        match self {
            Self::Empty => Ok(Vec::new()),
            Self::Bytes(b) => Ok(b),
            Self::Stream(stream) => stream.collect().await,
        }
    }

    /// Check if body is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Bytes(b) => b.is_empty(),
            Self::Stream(s) => s.is_complete() && s.bytes_received() == 0,
        }
    }

    /// Check if body is a streaming body.
    #[must_use]
    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Stream(_))
    }

    /// Take the body stream, if this is a streaming body.
    ///
    /// Returns `None` for `Empty` and `Bytes` variants.
    #[must_use]
    pub fn take_stream(self) -> Option<RequestBodyStream> {
        match self {
            Self::Stream(s) => Some(s),
            _ => None,
        }
    }

    /// Create a streaming body.
    pub fn streaming<S>(stream: S) -> Self
    where
        S: asupersync::stream::Stream<Item = Result<Vec<u8>, RequestBodyStreamError>>
            + Send
            + Sync
            + 'static,
    {
        Self::Stream(RequestBodyStream::new(stream))
    }

    /// Create a streaming body with a known size.
    pub fn streaming_with_size<S>(stream: S, size: usize) -> Self
    where
        S: asupersync::stream::Stream<Item = Result<Vec<u8>, RequestBodyStreamError>>
            + Send
            + Sync
            + 'static,
    {
        Self::Stream(RequestBodyStream::with_expected_size(stream, size))
    }
}

/// HTTP request.
#[derive(Debug)]
pub struct Request {
    method: Method,
    path: String,
    query: Option<String>,
    version: HttpVersion,
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
            version: HttpVersion::default(),
            headers: Headers::new(),
            body: Body::Empty,
            extensions: HashMap::new(),
        }
    }

    /// Create a new request with a specific HTTP version.
    #[must_use]
    pub fn with_version(method: Method, path: impl Into<String>, version: HttpVersion) -> Self {
        Self {
            method,
            path: path.into(),
            query: None,
            version,
            headers: Headers::new(),
            body: Body::Empty,
            extensions: HashMap::new(),
        }
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
}
