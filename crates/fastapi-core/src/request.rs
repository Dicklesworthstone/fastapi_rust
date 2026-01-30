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
///
/// Header names are normalized to lowercase at insertion time for case-insensitive
/// matching. Lookups avoid allocation when the lookup key is already lowercase.
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
    ///
    /// Avoids heap allocation when the lookup key is already lowercase.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.inner
            .get(lowercase_header_key(name).as_ref())
            .map(Vec::as_slice)
    }

    /// Insert a header.
    ///
    /// The header name is normalized to lowercase.
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
        self.inner.remove(lowercase_header_key(name).as_ref())
    }

    /// Check if a header exists (case-insensitive).
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.inner.contains_key(lowercase_header_key(name).as_ref())
    }
}

/// Lowercase a header name for lookup.
///
/// Returns a `Cow<str>` that is:
/// - **Borrowed** if the name is already lowercase (zero allocation)
/// - **Owned** if uppercase characters need conversion
///
/// Since programmatic code typically uses lowercase header names like
/// `"content-type"` rather than `"Content-Type"`, most lookups are zero-alloc.
#[inline]
fn lowercase_header_key(name: &str) -> std::borrow::Cow<'_, str> {
    // Fast path: check if name is already ASCII lowercase.
    // This covers the common case of programmatic access with lowercase literals.
    let needs_lowercase = name.as_bytes().iter().any(|&b| b.is_ascii_uppercase());

    if needs_lowercase {
        std::borrow::Cow::Owned(name.to_ascii_lowercase())
    } else {
        std::borrow::Cow::Borrowed(name)
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

    /// Set the request path.
    ///
    /// This is used internally for mounted sub-applications, where the
    /// mount prefix is stripped from the path before forwarding.
    pub fn set_path(&mut self, path: String) {
        self.path = path;
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

#[cfg(test)]
mod tests {
    use super::*;
    use asupersync::stream::Stream;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct NoopWaker;

    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    fn noop_waker() -> Waker {
        Waker::from(Arc::new(NoopWaker))
    }

    // ============================================================
    // bd-isux: RequestBodyStream tests for large uploads
    // ============================================================

    #[test]
    fn stream_10mb_body_in_64kb_chunks() {
        // Test streaming a 10MB request body in chunks (bd-isux)
        const TARGET_SIZE: usize = 10 * 1024 * 1024; // 10MB
        const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks

        // Create chunks that total 10MB
        let num_chunks = TARGET_SIZE.div_ceil(CHUNK_SIZE);
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> = (0..num_chunks)
            .map(|i| {
                let start = i * CHUNK_SIZE;
                let end = std::cmp::min(start + CHUNK_SIZE, TARGET_SIZE);
                let chunk: Vec<u8> = (start..end).map(|j| (j % 256) as u8).collect();
                Ok(chunk)
            })
            .collect();

        let stream = asupersync::stream::iter(chunks);
        let mut body_stream = RequestBodyStream::with_expected_size(stream, TARGET_SIZE);

        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        let mut total_received = 0usize;
        let mut chunk_count = 0usize;

        loop {
            match Pin::new(&mut body_stream).poll_next(&mut ctx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    total_received += chunk.len();
                    chunk_count += 1;
                }
                Poll::Ready(Some(Err(e))) => panic!("Unexpected error: {e}"),
                Poll::Ready(None) => break,
                Poll::Pending => panic!("Mock stream should never return Pending"),
            }
        }

        assert_eq!(total_received, TARGET_SIZE, "Should receive all 10MB");
        assert_eq!(
            chunk_count, num_chunks,
            "Should have correct number of chunks"
        );
        assert!(
            body_stream.is_complete(),
            "Stream should be marked complete"
        );
        assert_eq!(
            body_stream.bytes_received(),
            TARGET_SIZE,
            "bytes_received should match"
        );
    }

    #[test]
    fn stream_memory_bounded_during_processing() {
        // Test that streaming doesn't buffer entire body (bd-isux)
        // Verify incremental processing with memory < 1MB at any point
        const TARGET_SIZE: usize = 5 * 1024 * 1024; // 5MB total
        const CHUNK_SIZE: usize = 64 * 1024; // 64KB chunks
        const MAX_MEMORY: usize = 1024 * 1024; // 1MB max

        let num_chunks = TARGET_SIZE / CHUNK_SIZE;
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> =
            (0..num_chunks).map(|_| Ok(vec![0u8; CHUNK_SIZE])).collect();

        let stream = asupersync::stream::iter(chunks);
        let mut body_stream = RequestBodyStream::new(stream);

        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        // Process chunks incrementally, simulating bounded memory usage
        let mut processed_total = 0usize;
        let mut max_held = 0usize;

        loop {
            match Pin::new(&mut body_stream).poll_next(&mut ctx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    // Simulate processing each chunk without accumulating
                    let chunk_size = chunk.len();
                    max_held = std::cmp::max(max_held, chunk_size);
                    processed_total += chunk_size;
                    // chunk goes out of scope here, releasing memory
                }
                Poll::Ready(Some(Err(e))) => panic!("Unexpected error: {e}"),
                Poll::Ready(None) => break,
                Poll::Pending => panic!("Mock stream should never return Pending"),
            }
        }

        assert_eq!(processed_total, TARGET_SIZE, "Should process all data");
        assert!(
            max_held <= MAX_MEMORY,
            "Max memory held per chunk ({max_held}) should be < {MAX_MEMORY}"
        );
    }

    #[test]
    fn stream_error_connection_closed() {
        // Test RequestBodyStreamError::ConnectionClosed (bd-isux)
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> = vec![
            Ok(vec![1, 2, 3]),
            Err(RequestBodyStreamError::ConnectionClosed),
            Ok(vec![4, 5, 6]), // Should not reach this
        ];

        let stream = asupersync::stream::iter(chunks);
        let mut body_stream = RequestBodyStream::new(stream);

        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        // First chunk succeeds
        match Pin::new(&mut body_stream).poll_next(&mut ctx) {
            Poll::Ready(Some(Ok(chunk))) => assert_eq!(chunk, vec![1, 2, 3]),
            other => panic!("Expected first chunk, got {other:?}"),
        }

        // Second chunk is error
        match Pin::new(&mut body_stream).poll_next(&mut ctx) {
            Poll::Ready(Some(Err(RequestBodyStreamError::ConnectionClosed))) => {}
            other => panic!("Expected ConnectionClosed error, got {other:?}"),
        }
    }

    #[test]
    fn stream_error_timeout() {
        // Test RequestBodyStreamError::Timeout (bd-isux)
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> =
            vec![Ok(vec![1, 2]), Err(RequestBodyStreamError::Timeout)];

        let stream = asupersync::stream::iter(chunks);
        let mut body_stream = RequestBodyStream::new(stream);

        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        // First chunk succeeds
        match Pin::new(&mut body_stream).poll_next(&mut ctx) {
            Poll::Ready(Some(Ok(_))) => {}
            other => panic!("Expected first chunk, got {other:?}"),
        }

        // Second is timeout
        match Pin::new(&mut body_stream).poll_next(&mut ctx) {
            Poll::Ready(Some(Err(RequestBodyStreamError::Timeout))) => {}
            other => panic!("Expected Timeout error, got {other:?}"),
        }

        // Verify error display
        let err = RequestBodyStreamError::Timeout;
        assert_eq!(format!("{err}"), "timeout waiting for body data");
    }

    #[test]
    fn stream_error_too_large() {
        // Test RequestBodyStreamError::TooLarge (bd-isux)
        let err = RequestBodyStreamError::TooLarge {
            received: 10_000_000,
            max: 1_000_000,
        };

        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> = vec![Err(err)];

        let stream = asupersync::stream::iter(chunks);
        let mut body_stream = RequestBodyStream::new(stream);

        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        match Pin::new(&mut body_stream).poll_next(&mut ctx) {
            Poll::Ready(Some(Err(RequestBodyStreamError::TooLarge { received, max }))) => {
                assert_eq!(received, 10_000_000);
                assert_eq!(max, 1_000_000);
            }
            other => panic!("Expected TooLarge error, got {other:?}"),
        }

        // Verify error display
        let err = RequestBodyStreamError::TooLarge {
            received: 10_000_000,
            max: 1_000_000,
        };
        assert!(format!("{err}").contains("10000000"));
        assert!(format!("{err}").contains("1000000"));
    }

    #[test]
    fn stream_error_io() {
        // Test RequestBodyStreamError::Io (bd-isux)
        let err = RequestBodyStreamError::Io("disk full".to_string());

        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> = vec![Err(err)];

        let stream = asupersync::stream::iter(chunks);
        let mut body_stream = RequestBodyStream::new(stream);

        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        match Pin::new(&mut body_stream).poll_next(&mut ctx) {
            Poll::Ready(Some(Err(RequestBodyStreamError::Io(msg)))) => {
                assert_eq!(msg, "disk full");
            }
            other => panic!("Expected Io error, got {other:?}"),
        }

        // Verify error display
        let err = RequestBodyStreamError::Io("disk full".to_string());
        assert!(format!("{err}").contains("disk full"));
    }

    #[test]
    fn stream_expected_size_tracking() {
        // Test expected_size is correctly tracked (bd-isux)
        const EXPECTED: usize = 1024;

        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> =
            vec![Ok(vec![0u8; 512]), Ok(vec![0u8; 512])];

        let stream = asupersync::stream::iter(chunks);
        let body_stream = RequestBodyStream::with_expected_size(stream, EXPECTED);

        assert_eq!(body_stream.expected_size(), Some(EXPECTED));
        assert_eq!(body_stream.bytes_received(), 0);
        assert!(!body_stream.is_complete());
    }

    #[test]
    fn stream_collect_accumulates_all_chunks() {
        // Test collect() method gathers all data (bd-isux)
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> =
            vec![Ok(vec![1, 2, 3]), Ok(vec![4, 5]), Ok(vec![6, 7, 8, 9])];

        let stream = asupersync::stream::iter(chunks);
        let body_stream = RequestBodyStream::new(stream);

        let result = futures_executor::block_on(body_stream.collect());
        assert!(result.is_ok());
        let data = result.unwrap();
        assert_eq!(data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn stream_collect_propagates_error() {
        // Test collect() stops and returns error (bd-isux)
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> = vec![
            Ok(vec![1, 2, 3]),
            Err(RequestBodyStreamError::ConnectionClosed),
            Ok(vec![4, 5, 6]),
        ];

        let stream = asupersync::stream::iter(chunks);
        let body_stream = RequestBodyStream::new(stream);

        let result = futures_executor::block_on(body_stream.collect());
        assert!(result.is_err());
        match result {
            Err(RequestBodyStreamError::ConnectionClosed) => {}
            other => panic!("Expected ConnectionClosed, got {other:?}"),
        }
    }

    #[test]
    fn body_streaming_helper_creates_stream() {
        // Test Body::streaming() helper (bd-isux)
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> = vec![Ok(vec![1, 2, 3])];
        let stream = asupersync::stream::iter(chunks);
        let body = Body::streaming(stream);

        assert!(body.is_streaming());
        assert!(!body.is_empty());
    }

    #[test]
    fn body_streaming_with_size_helper() {
        // Test Body::streaming_with_size() helper (bd-isux)
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> = vec![Ok(vec![1, 2, 3])];
        let stream = asupersync::stream::iter(chunks);
        let body = Body::streaming_with_size(stream, 3);

        assert!(body.is_streaming());

        if let Body::Stream(s) = body {
            assert_eq!(s.expected_size(), Some(3));
        } else {
            panic!("Expected Body::Stream");
        }
    }

    #[test]
    fn body_into_bytes_async_handles_stream() {
        // Test Body::into_bytes_async() for streaming bodies (bd-isux)
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> =
            vec![Ok(vec![1, 2]), Ok(vec![3, 4])];
        let stream = asupersync::stream::iter(chunks);
        let body = Body::streaming(stream);

        let result = futures_executor::block_on(body.into_bytes_async());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn body_take_stream_extracts_stream() {
        // Test Body::take_stream() (bd-isux)
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> = vec![Ok(vec![1, 2, 3])];
        let stream = asupersync::stream::iter(chunks);
        let body = Body::streaming(stream);

        let taken = body.take_stream();
        assert!(taken.is_some());

        // Non-streaming bodies return None
        let empty_body = Body::Empty;
        assert!(empty_body.take_stream().is_none());

        let bytes_body = Body::Bytes(vec![1, 2, 3]);
        assert!(bytes_body.take_stream().is_none());
    }

    #[test]
    #[should_panic(expected = "cannot synchronously convert streaming body")]
    fn body_into_bytes_panics_for_stream() {
        // Test that into_bytes() panics for streaming bodies (bd-isux)
        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> = vec![Ok(vec![1, 2, 3])];
        let stream = asupersync::stream::iter(chunks);
        let body = Body::streaming(stream);

        let _ = body.into_bytes(); // Should panic
    }

    // ============================================================
    // bd-3slp: Header lookup optimization tests
    // ============================================================

    #[test]
    fn headers_lowercase_key_fast_path() {
        // Test that lowercase keys work without allocation (bd-3slp)
        let mut headers = Headers::new();
        headers.insert("content-type", b"application/json".to_vec());

        // Lookup with lowercase - fast path (no allocation)
        assert!(headers.get("content-type").is_some());
        assert!(headers.contains("content-type"));

        // Lookup with mixed case - still works but may allocate
        assert!(headers.get("Content-Type").is_some());
        assert!(headers.contains("CONTENT-TYPE"));
    }

    #[test]
    fn headers_case_insensitive_lookup() {
        // Verify case-insensitive behavior is preserved (bd-3slp)
        let mut headers = Headers::new();
        headers.insert("X-Custom-Header", b"value".to_vec());

        // All case variations should work
        assert_eq!(headers.get("x-custom-header"), Some(b"value".as_slice()));
        assert_eq!(headers.get("X-CUSTOM-HEADER"), Some(b"value".as_slice()));
        assert_eq!(headers.get("X-Custom-Header"), Some(b"value".as_slice()));
        assert_eq!(headers.get("x-CuStOm-HeAdEr"), Some(b"value".as_slice()));
    }

    #[test]
    fn headers_remove_case_insensitive() {
        // Verify remove works with case insensitivity (bd-3slp)
        let mut headers = Headers::new();
        headers.insert("Authorization", b"Bearer token".to_vec());

        // Remove with different case
        let removed = headers.remove("AUTHORIZATION");
        assert_eq!(removed, Some(b"Bearer token".to_vec()));
        assert!(!headers.contains("authorization"));
    }

    #[test]
    fn lowercase_header_key_already_lowercase() {
        // Fast path test - already lowercase borrows original (bd-3slp)
        use std::borrow::Cow;

        let result = lowercase_header_key("content-type");
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result.as_ref(), "content-type");
    }

    #[test]
    fn lowercase_header_key_needs_conversion() {
        // Slow path test - uppercase chars need conversion (bd-3slp)
        use std::borrow::Cow;

        let result = lowercase_header_key("Content-Type");
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result.as_ref(), "content-type");
    }

    #[test]
    fn lowercase_header_key_all_uppercase() {
        use std::borrow::Cow;

        let result = lowercase_header_key("CONTENT-TYPE");
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result.as_ref(), "content-type");
    }
}
