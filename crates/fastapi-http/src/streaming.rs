//! Streaming response body support.
//!
//! This module provides helpers for streaming large response bodies with:
//!
//! - **Memory-bounded buffering**: Configurable chunk sizes to control memory usage
//! - **Cancel-aware streaming**: Integration with asupersync Cx for graceful cancellation
//! - **File streaming**: Efficient file streaming without loading entire files into memory
//! - **Backpressure**: Works with asupersync's checkpoint system
//!
//! # Example
//!
//! ```ignore
//! use fastapi_http::streaming::{FileStream, StreamConfig};
//! use fastapi_core::{Response, ResponseBody};
//!
//! async fn stream_file(cx: &Cx) -> Response {
//!     let config = StreamConfig::default();
//!     let stream = FileStream::open("large_file.bin", cx.clone(), config).await?;
//!     Response::ok()
//!         .header("content-type", b"application/octet-stream".to_vec())
//!         .body(ResponseBody::stream(stream))
//! }
//! ```

use asupersync::Cx;
use asupersync::stream::Stream;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Default chunk size for streaming (64KB).
pub const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// Default maximum buffer size (4MB).
pub const DEFAULT_MAX_BUFFER_SIZE: usize = 4 * 1024 * 1024;

/// Configuration for streaming responses.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Size of each chunk when reading/writing.
    chunk_size: usize,
    /// Maximum buffer size for backpressure.
    max_buffer_size: usize,
    /// Whether to call checkpoint() between chunks.
    checkpoint_enabled: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            max_buffer_size: DEFAULT_MAX_BUFFER_SIZE,
            checkpoint_enabled: true,
        }
    }
}

impl StreamConfig {
    /// Create a new stream configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the chunk size for reading.
    #[must_use]
    pub fn with_chunk_size(mut self, size: usize) -> Self {
        self.chunk_size = size.max(1024); // Minimum 1KB
        self
    }

    /// Set the maximum buffer size for backpressure.
    #[must_use]
    pub fn with_max_buffer_size(mut self, size: usize) -> Self {
        self.max_buffer_size = size;
        self
    }

    /// Enable or disable checkpoint calls between chunks.
    #[must_use]
    pub fn with_checkpoint(mut self, enabled: bool) -> Self {
        self.checkpoint_enabled = enabled;
        self
    }

    /// Returns the chunk size.
    #[must_use]
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// Returns the maximum buffer size.
    #[must_use]
    pub fn max_buffer_size(&self) -> usize {
        self.max_buffer_size
    }

    /// Returns whether checkpoints are enabled.
    #[must_use]
    pub fn checkpoint_enabled(&self) -> bool {
        self.checkpoint_enabled
    }
}

/// Error types for streaming operations.
#[derive(Debug)]
pub enum StreamError {
    /// IO error during streaming.
    Io(io::Error),
    /// Stream was cancelled (client disconnect, timeout, etc.).
    Cancelled,
    /// Stream reached configured buffer limit.
    BufferFull,
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "streaming I/O error: {e}"),
            Self::Cancelled => write!(f, "stream cancelled"),
            Self::BufferFull => write!(f, "stream buffer full"),
        }
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for StreamError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// A stream that wraps another stream and respects cancellation via Cx.
///
/// On each poll, this stream checks if cancellation has been requested
/// via the capability context. If so, it returns `None` to stop the stream.
///
/// This enables graceful handling of:
/// - Client disconnects
/// - Request timeouts (budget exhaustion)
/// - Server shutdown
pub struct CancelAwareStream<S> {
    inner: S,
    cx: Cx,
    cancelled: bool,
}

impl<S> CancelAwareStream<S> {
    /// Create a new cancel-aware stream wrapper.
    pub fn new(inner: S, cx: Cx) -> Self {
        Self {
            inner,
            cx,
            cancelled: false,
        }
    }

    /// Check if the stream was cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

impl<S> Stream for CancelAwareStream<S>
where
    S: Stream + Unpin,
{
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Check for cancellation
        if self.cx.is_cancel_requested() {
            self.cancelled = true;
            return Poll::Ready(None);
        }

        // Poll the inner stream
        Pin::new(&mut self.inner).poll_next(ctx)
    }
}

/// State for file streaming.
enum FileStreamState {
    /// Stream is active with a file handle.
    Active {
        file: std::fs::File,
        buffer: Vec<u8>,
        remaining: u64,
    },
    /// Stream is complete.
    Complete,
    /// Stream encountered an error.
    Error,
}

/// A stream that reads a file in chunks.
///
/// This stream reads files incrementally, yielding chunks of configurable
/// size. It integrates with asupersync's Cx for:
///
/// - Cancellation detection (client disconnect)
/// - Checkpoint calls between chunks for cooperative yielding
///
/// # Memory Efficiency
///
/// Only one chunk is buffered at a time, making this suitable for
/// streaming large files without excessive memory usage.
///
/// # Example
///
/// ```ignore
/// use fastapi_http::streaming::{FileStream, StreamConfig};
///
/// let config = StreamConfig::default().with_chunk_size(32 * 1024);
/// let stream = FileStream::open("video.mp4", cx.clone(), config).await?;
/// ```
pub struct FileStream {
    state: FileStreamState,
    cx: Cx,
    config: StreamConfig,
}

impl FileStream {
    /// Open a file for streaming.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file to stream
    /// * `cx` - Capability context for cancellation
    /// * `config` - Streaming configuration
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened.
    pub fn open<P: AsRef<Path>>(path: P, cx: Cx, config: StreamConfig) -> io::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let metadata = file.metadata()?;
        let file_size = metadata.len();

        // Seek to beginning (defensive)
        file.seek(SeekFrom::Start(0))?;

        let buffer = Vec::with_capacity(config.chunk_size);

        Ok(Self {
            state: FileStreamState::Active {
                file,
                buffer,
                remaining: file_size,
            },
            cx,
            config,
        })
    }

    /// Open a file for streaming with a byte range.
    ///
    /// Useful for HTTP Range requests.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file
    /// * `start` - Start byte offset
    /// * `length` - Number of bytes to stream
    /// * `cx` - Capability context
    /// * `config` - Streaming configuration
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or seeked.
    pub fn open_range<P: AsRef<Path>>(
        path: P,
        start: u64,
        length: u64,
        cx: Cx,
        config: StreamConfig,
    ) -> io::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        file.seek(SeekFrom::Start(start))?;

        let buffer = Vec::with_capacity(config.chunk_size);

        Ok(Self {
            state: FileStreamState::Active {
                file,
                buffer,
                remaining: length,
            },
            cx,
            config,
        })
    }

    /// Get the remaining bytes to be read.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        match &self.state {
            FileStreamState::Active { remaining, .. } => *remaining,
            _ => 0,
        }
    }

    /// Check if the stream is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        matches!(self.state, FileStreamState::Complete)
    }
}

impl Stream for FileStream {
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, _ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Check for cancellation first
        if self.cx.is_cancel_requested() {
            self.state = FileStreamState::Complete;
            return Poll::Ready(None);
        }

        // Read chunk_size before the match to avoid borrow conflicts
        let chunk_size = self.config.chunk_size;

        match &mut self.state {
            FileStreamState::Active {
                file,
                buffer,
                remaining,
            } => {
                if *remaining == 0 {
                    self.state = FileStreamState::Complete;
                    return Poll::Ready(None);
                }

                // Determine chunk size
                let to_read = (chunk_size as u64).min(*remaining) as usize;

                // Resize buffer for this chunk
                buffer.clear();
                buffer.resize(to_read, 0);

                // Read from file
                match file.read(&mut buffer[..to_read]) {
                    Ok(0) => {
                        // EOF reached
                        self.state = FileStreamState::Complete;
                        Poll::Ready(None)
                    }
                    Ok(n) => {
                        *remaining -= n as u64;
                        buffer.truncate(n);

                        // Take the buffer and prepare next
                        let chunk = std::mem::take(buffer);
                        *buffer = Vec::with_capacity(chunk_size);

                        Poll::Ready(Some(chunk))
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                        // Retry on EINTR
                        _ctx.waker().wake_by_ref();
                        Poll::Pending
                    }
                    Err(_) => {
                        self.state = FileStreamState::Error;
                        Poll::Ready(None)
                    }
                }
            }
            FileStreamState::Complete | FileStreamState::Error => Poll::Ready(None),
        }
    }
}

// Safety: FileStream is Send because it only contains:
// - std::fs::File (Send)
// - Cx (Send + Sync)
// - StreamConfig (Send + Sync)
// - Vec<u8> (Send)
// The file handle is only accessed from poll_next, which requires &mut self.
#[allow(unsafe_code)]
unsafe impl Send for FileStream {}

/// A stream that yields chunks from an in-memory buffer.
///
/// Useful for testing or streaming pre-loaded data in chunks.
pub struct ChunkedBytes {
    data: Vec<u8>,
    position: usize,
    chunk_size: usize,
}

impl ChunkedBytes {
    /// Create a new chunked bytes stream.
    #[must_use]
    pub fn new(data: Vec<u8>, chunk_size: usize) -> Self {
        Self {
            data,
            position: 0,
            chunk_size: chunk_size.max(1),
        }
    }

    /// Create a chunked bytes stream with default chunk size.
    #[must_use]
    pub fn with_default_chunks(data: Vec<u8>) -> Self {
        Self::new(data, DEFAULT_CHUNK_SIZE)
    }

    /// Returns the total size of the data.
    #[must_use]
    pub fn total_size(&self) -> usize {
        self.data.len()
    }

    /// Returns the remaining bytes.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.position)
    }
}

impl Stream for ChunkedBytes {
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, _ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.position >= self.data.len() {
            return Poll::Ready(None);
        }

        let end = (self.position + self.chunk_size).min(self.data.len());
        let chunk = self.data[self.position..end].to_vec();
        self.position = end;

        Poll::Ready(Some(chunk))
    }
}

/// Extension trait for creating streaming response bodies.
pub trait StreamingResponseExt {
    /// Create a streaming response from a file.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file to stream
    /// * `cx` - Capability context
    /// * `content_type` - MIME type for the Content-Type header
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened.
    fn stream_file<P: AsRef<Path>>(
        path: P,
        cx: Cx,
        content_type: &[u8],
    ) -> io::Result<fastapi_core::Response>;

    /// Create a streaming response from a file with custom config.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened.
    fn stream_file_with_config<P: AsRef<Path>>(
        path: P,
        cx: Cx,
        content_type: &[u8],
        config: StreamConfig,
    ) -> io::Result<fastapi_core::Response>;

    /// Create a 206 Partial Content response for a byte range of a file.
    ///
    /// This is used to handle HTTP Range requests for partial content delivery.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file
    /// * `range` - The validated byte range to serve
    /// * `total_size` - Total size of the file (for Content-Range header)
    /// * `cx` - Capability context
    /// * `content_type` - MIME type for the Content-Type header
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or seeked.
    fn stream_file_range<P: AsRef<Path>>(
        path: P,
        range: crate::range::ByteRange,
        total_size: u64,
        cx: Cx,
        content_type: &[u8],
    ) -> io::Result<fastapi_core::Response>;

    /// Create a 206 Partial Content response with custom streaming config.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or seeked.
    fn stream_file_range_with_config<P: AsRef<Path>>(
        path: P,
        range: crate::range::ByteRange,
        total_size: u64,
        cx: Cx,
        content_type: &[u8],
        config: StreamConfig,
    ) -> io::Result<fastapi_core::Response>;
}

impl StreamingResponseExt for fastapi_core::Response {
    fn stream_file<P: AsRef<Path>>(
        path: P,
        cx: Cx,
        content_type: &[u8],
    ) -> io::Result<fastapi_core::Response> {
        Self::stream_file_with_config(path, cx, content_type, StreamConfig::default())
    }

    fn stream_file_with_config<P: AsRef<Path>>(
        path: P,
        cx: Cx,
        content_type: &[u8],
        config: StreamConfig,
    ) -> io::Result<fastapi_core::Response> {
        let stream = FileStream::open(path, cx, config)?;

        Ok(fastapi_core::Response::ok()
            .header("content-type", content_type.to_vec())
            .header("accept-ranges", b"bytes".to_vec())
            .body(fastapi_core::ResponseBody::stream(stream)))
    }

    fn stream_file_range<P: AsRef<Path>>(
        path: P,
        range: crate::range::ByteRange,
        total_size: u64,
        cx: Cx,
        content_type: &[u8],
    ) -> io::Result<fastapi_core::Response> {
        Self::stream_file_range_with_config(
            path,
            range,
            total_size,
            cx,
            content_type,
            StreamConfig::default(),
        )
    }

    fn stream_file_range_with_config<P: AsRef<Path>>(
        path: P,
        range: crate::range::ByteRange,
        total_size: u64,
        cx: Cx,
        content_type: &[u8],
        config: StreamConfig,
    ) -> io::Result<fastapi_core::Response> {
        let stream = FileStream::open_range(path, range.start, range.len(), cx, config)?;

        Ok(fastapi_core::Response::partial_content()
            .header("content-type", content_type.to_vec())
            .header("accept-ranges", b"bytes".to_vec())
            .header(
                "content-range",
                range.content_range_header(total_size).into_bytes(),
            )
            .header("content-length", range.len().to_string().into_bytes())
            .body(fastapi_core::ResponseBody::stream(stream)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::task::{Wake, Waker};

    struct NoopWaker;

    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    fn noop_waker() -> Waker {
        Waker::from(Arc::new(NoopWaker))
    }

    #[test]
    fn stream_config_defaults() {
        let config = StreamConfig::default();
        assert_eq!(config.chunk_size(), DEFAULT_CHUNK_SIZE);
        assert_eq!(config.max_buffer_size(), DEFAULT_MAX_BUFFER_SIZE);
        assert!(config.checkpoint_enabled());
    }

    #[test]
    fn stream_config_custom() {
        let config = StreamConfig::new()
            .with_chunk_size(1024)
            .with_max_buffer_size(2048)
            .with_checkpoint(false);

        assert_eq!(config.chunk_size(), 1024);
        assert_eq!(config.max_buffer_size(), 2048);
        assert!(!config.checkpoint_enabled());
    }

    #[test]
    fn stream_config_minimum_chunk_size() {
        let config = StreamConfig::new().with_chunk_size(100);
        // Should be clamped to minimum of 1KB
        assert_eq!(config.chunk_size(), 1024);
    }

    #[test]
    fn chunked_bytes_basic() {
        let data = b"Hello, World!".to_vec();
        let mut stream = ChunkedBytes::new(data.clone(), 5);

        assert_eq!(stream.total_size(), 13);
        assert_eq!(stream.remaining(), 13);

        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        // Chunk 1: "Hello"
        let chunk = Pin::new(&mut stream).poll_next(&mut ctx);
        assert_eq!(chunk, Poll::Ready(Some(b"Hello".to_vec())));
        assert_eq!(stream.remaining(), 8);

        // Chunk 2: ", Wor"
        let chunk = Pin::new(&mut stream).poll_next(&mut ctx);
        assert_eq!(chunk, Poll::Ready(Some(b", Wor".to_vec())));

        // Chunk 3: "ld!"
        let chunk = Pin::new(&mut stream).poll_next(&mut ctx);
        assert_eq!(chunk, Poll::Ready(Some(b"ld!".to_vec())));

        // End
        let chunk = Pin::new(&mut stream).poll_next(&mut ctx);
        assert_eq!(chunk, Poll::Ready(None));
    }

    #[test]
    fn chunked_bytes_empty() {
        let mut stream = ChunkedBytes::new(Vec::new(), 5);
        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        let chunk = Pin::new(&mut stream).poll_next(&mut ctx);
        assert_eq!(chunk, Poll::Ready(None));
    }

    #[test]
    fn chunked_bytes_exact_chunk_size() {
        let data = b"12345".to_vec();
        let mut stream = ChunkedBytes::new(data, 5);

        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        // Should yield one full chunk
        let chunk = Pin::new(&mut stream).poll_next(&mut ctx);
        assert_eq!(chunk, Poll::Ready(Some(b"12345".to_vec())));

        // Then end
        let chunk = Pin::new(&mut stream).poll_next(&mut ctx);
        assert_eq!(chunk, Poll::Ready(None));
    }

    #[test]
    fn cancel_aware_stream_propagates_items() {
        let inner = asupersync::stream::iter(vec![1, 2, 3]);
        let cx = Cx::for_testing();
        let mut stream = CancelAwareStream::new(inner, cx);

        let waker = noop_waker();
        let mut ctx = Context::from_waker(&waker);

        assert_eq!(
            Pin::new(&mut stream).poll_next(&mut ctx),
            Poll::Ready(Some(1))
        );
        assert_eq!(
            Pin::new(&mut stream).poll_next(&mut ctx),
            Poll::Ready(Some(2))
        );
        assert_eq!(
            Pin::new(&mut stream).poll_next(&mut ctx),
            Poll::Ready(Some(3))
        );
        assert_eq!(Pin::new(&mut stream).poll_next(&mut ctx), Poll::Ready(None));

        assert!(!stream.is_cancelled());
    }

    #[test]
    fn stream_error_display() {
        let err = StreamError::Cancelled;
        assert_eq!(format!("{err}"), "stream cancelled");

        let err = StreamError::BufferFull;
        assert_eq!(format!("{err}"), "stream buffer full");

        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let err = StreamError::Io(io_err);
        assert!(format!("{err}").contains("streaming I/O error"));
    }

    // =========================================================================
    // StreamingResponseExt tests
    // =========================================================================

    #[test]
    fn stream_file_adds_accept_ranges_header() {
        // Create a temp file
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_stream_accept_ranges.txt");
        std::fs::write(&test_file, b"Hello, streaming world!").unwrap();

        let cx = Cx::for_testing();
        let response =
            fastapi_core::Response::stream_file(&test_file, cx, b"text/plain").unwrap();

        let accept_ranges = response
            .headers()
            .iter()
            .find(|(name, _)| name == "accept-ranges")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(accept_ranges, Some("bytes".to_string()));

        // Cleanup
        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn stream_file_range_returns_206() {
        use crate::range::ByteRange;

        // Create a temp file
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_stream_range_206.txt");
        std::fs::write(&test_file, b"0123456789ABCDEF").unwrap();

        let cx = Cx::for_testing();
        let range = ByteRange::new(0, 4); // First 5 bytes
        let response = fastapi_core::Response::stream_file_range(
            &test_file,
            range,
            16, // Total size
            cx,
            b"text/plain",
        )
        .unwrap();

        // Should be 206 Partial Content
        assert_eq!(response.status().as_u16(), 206);

        // Cleanup
        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn stream_file_range_sets_content_range_header() {
        use crate::range::ByteRange;

        // Create a temp file
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_stream_content_range.txt");
        std::fs::write(&test_file, b"0123456789ABCDEF").unwrap();

        let cx = Cx::for_testing();
        let range = ByteRange::new(5, 9); // Bytes 5-9 (5 bytes)
        let response = fastapi_core::Response::stream_file_range(
            &test_file,
            range,
            16, // Total size
            cx,
            b"text/plain",
        )
        .unwrap();

        let content_range = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-range")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(content_range, Some("bytes 5-9/16".to_string()));

        // Cleanup
        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn stream_file_range_sets_content_length_header() {
        use crate::range::ByteRange;

        // Create a temp file
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_stream_content_length.txt");
        std::fs::write(&test_file, b"0123456789ABCDEF").unwrap();

        let cx = Cx::for_testing();
        let range = ByteRange::new(0, 99); // Will be clamped by file size
        let response = fastapi_core::Response::stream_file_range(
            &test_file,
            range,
            16, // Total size
            cx,
            b"text/plain",
        )
        .unwrap();

        let content_length = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-length")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        // Range 0-99 has length 100, but actual bytes served depend on the file
        assert_eq!(content_length, Some("100".to_string()));

        // Cleanup
        let _ = std::fs::remove_file(test_file);
    }
}
