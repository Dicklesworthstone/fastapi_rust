//! Server-Sent Events (SSE) support.
//!
//! This module provides types and utilities for implementing server-sent events
//! as defined in the [HTML Living Standard](https://html.spec.whatwg.org/multipage/server-sent-events.html).
//!
//! # Overview
//!
//! Server-Sent Events allow servers to push data to clients over HTTP. The connection
//! stays open and the server can send events whenever new data is available.
//!
//! # Example
//!
//! ```ignore
//! use fastapi_core::sse::{SseEvent, SseResponse};
//! use fastapi_core::Response;
//! use asupersync::stream;
//!
//! async fn event_stream() -> Response {
//!     // Create an async stream of events
//!     let events = stream::iter(vec![
//!         SseEvent::message("Hello!"),
//!         SseEvent::new("World!")
//!             .event_type("greeting")
//!             .id("1"),
//!     ]);
//!
//!     SseResponse::new(events).into_response()
//! }
//! ```
//!
//! # Event Format
//!
//! Each event is sent as a text block with the following format:
//!
//! ```text
//! event: <event-type>\n
//! id: <id>\n
//! retry: <milliseconds>\n
//! data: <data-line-1>\n
//! data: <data-line-2>\n
//! \n
//! ```
//!
//! - `event`: Optional event type (default is "message")
//! - `id`: Optional event ID for resumption
//! - `retry`: Optional reconnection time in milliseconds
//! - `data`: The actual payload (required, can be multiple lines)
//!
//! # Keep-Alive
//!
//! Use [`SseEvent::comment()`] to send keep-alive comments that prevent
//! connection timeouts without sending actual events.
//!
//! # Cancellation
//!
//! SSE streams integrate with asupersync's cancellation. When the client
//! disconnects, the stream will be cancelled at the next checkpoint.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use asupersync::stream::Stream;

use crate::response::{Response, ResponseBody, StatusCode};

/// A Server-Sent Event.
///
/// Events consist of one or more fields:
/// - `data`: The message payload (required, can contain newlines)
/// - `event`: The event type (optional, defaults to "message")
/// - `id`: An identifier for the event (optional, used for resumption)
/// - `retry`: Reconnection time in milliseconds (optional)
/// - `comment`: A comment (not delivered to the event listener)
///
/// # Example
///
/// ```ignore
/// use fastapi_core::sse::SseEvent;
///
/// // Simple message
/// let event = SseEvent::message("Hello, World!");
///
/// // Event with type and ID
/// let event = SseEvent::new("user joined")
///     .event_type("join")
///     .id("123");
///
/// // JSON data
/// let event = SseEvent::new(r#"{"user":"alice","action":"login"}"#)
///     .event_type("user_event");
///
/// // Keep-alive comment
/// let event = SseEvent::comment("keep-alive");
/// ```
#[derive(Debug, Clone)]
pub struct SseEvent {
    data: Option<String>,
    event_type: Option<String>,
    id: Option<String>,
    retry: Option<u64>,
    comment: Option<String>,
}

impl SseEvent {
    /// Create a new SSE event with the given data.
    ///
    /// The data can contain newlines - each line will be prefixed with `data: `.
    #[must_use]
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            data: Some(data.into()),
            event_type: None,
            id: None,
            retry: None,
            comment: None,
        }
    }

    /// Create a simple message event.
    ///
    /// Equivalent to `SseEvent::new(data)`.
    #[must_use]
    pub fn message(data: impl Into<String>) -> Self {
        Self::new(data)
    }

    /// Create a keep-alive comment.
    ///
    /// Comments are sent but not delivered to event listeners.
    /// Use these to prevent connection timeouts.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let keepalive = SseEvent::comment("heartbeat");
    /// ```
    #[must_use]
    pub fn comment(comment: impl Into<String>) -> Self {
        Self {
            data: None,
            event_type: None,
            id: None,
            retry: None,
            comment: Some(comment.into()),
        }
    }

    /// Set the event type.
    ///
    /// If not set, the event type defaults to "message".
    ///
    /// # Example
    ///
    /// ```ignore
    /// let event = SseEvent::new("user logged in")
    ///     .event_type("login");
    /// ```
    #[must_use]
    pub fn event_type(mut self, event_type: impl Into<String>) -> Self {
        self.event_type = Some(event_type.into());
        self
    }

    /// Set the event ID.
    ///
    /// The client stores this ID and sends it in the `Last-Event-ID` header
    /// when reconnecting, allowing the server to resume from where it left off.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let event = SseEvent::new("data")
    ///     .id("12345");
    /// ```
    #[must_use]
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the retry timeout in milliseconds.
    ///
    /// This tells the client how long to wait before attempting to reconnect
    /// if the connection is lost.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let event = SseEvent::new("data")
    ///     .retry_ms(5000); // 5 seconds
    /// ```
    #[must_use]
    pub fn retry_ms(mut self, milliseconds: u64) -> Self {
        self.retry = Some(milliseconds);
        self
    }

    /// Set the retry timeout from a Duration.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::time::Duration;
    ///
    /// let event = SseEvent::new("data")
    ///     .retry(Duration::from_secs(5));
    /// ```
    #[must_use]
    pub fn retry(self, duration: Duration) -> Self {
        self.retry_ms(duration.as_millis() as u64)
    }

    /// Format the event as SSE wire format.
    ///
    /// Returns bytes ready to be sent to the client.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(256);

        // Write comment if present
        if let Some(ref comment) = self.comment {
            for line in comment.lines() {
                output.extend_from_slice(b": ");
                output.extend_from_slice(line.as_bytes());
                output.push(b'\n');
            }
        }

        // Write event type if present
        if let Some(ref event_type) = self.event_type {
            output.extend_from_slice(b"event: ");
            output.extend_from_slice(event_type.as_bytes());
            output.push(b'\n');
        }

        // Write ID if present
        if let Some(ref id) = self.id {
            output.extend_from_slice(b"id: ");
            output.extend_from_slice(id.as_bytes());
            output.push(b'\n');
        }

        // Write retry if present
        if let Some(retry) = self.retry {
            output.extend_from_slice(b"retry: ");
            output.extend_from_slice(retry.to_string().as_bytes());
            output.push(b'\n');
        }

        // Write data lines (each line gets "data: " prefix)
        if let Some(ref data) = self.data {
            for line in data.lines() {
                output.extend_from_slice(b"data: ");
                output.extend_from_slice(line.as_bytes());
                output.push(b'\n');
            }
            // Handle case where data is empty or ends with newline
            if data.is_empty() {
                output.extend_from_slice(b"data: \n");
            }
        }

        // Events are terminated by a blank line
        output.push(b'\n');

        output
    }
}

impl From<&str> for SseEvent {
    fn from(data: &str) -> Self {
        Self::new(data)
    }
}

impl From<String> for SseEvent {
    fn from(data: String) -> Self {
        Self::new(data)
    }
}

/// A wrapper that converts an async stream of SSE events into formatted bytes.
///
/// This stream produces `Vec<u8>` chunks suitable for sending over HTTP.
pub struct SseStream<S> {
    inner: S,
}

impl<S> SseStream<S> {
    /// Create a new SSE stream wrapper.
    pub fn new(stream: S) -> Self {
        Self { inner: stream }
    }
}

impl<S> Stream for SseStream<S>
where
    S: Stream<Item = SseEvent> + Unpin,
{
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(event.to_bytes())),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Configuration for SSE responses.
#[derive(Debug, Clone)]
pub struct SseConfig {
    /// Keep-alive interval in seconds (0 = disabled).
    pub keep_alive_secs: u64,
    /// Comment to send for keep-alive.
    pub keep_alive_comment: String,
}

impl Default for SseConfig {
    fn default() -> Self {
        Self {
            keep_alive_secs: 30,
            keep_alive_comment: "keep-alive".to_string(),
        }
    }
}

impl SseConfig {
    /// Create a new SSE configuration with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the keep-alive interval.
    #[must_use]
    pub fn keep_alive_secs(mut self, seconds: u64) -> Self {
        self.keep_alive_secs = seconds;
        self
    }

    /// Disable keep-alive.
    #[must_use]
    pub fn disable_keep_alive(mut self) -> Self {
        self.keep_alive_secs = 0;
        self
    }

    /// Set the keep-alive comment text.
    #[must_use]
    pub fn keep_alive_comment(mut self, comment: impl Into<String>) -> Self {
        self.keep_alive_comment = comment.into();
        self
    }
}

/// Builder for creating SSE responses.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::sse::{SseEvent, SseResponse};
/// use asupersync::stream;
///
/// async fn events() -> Response {
///     let events = stream::iter(vec![
///         SseEvent::message("Hello"),
///         SseEvent::message("World"),
///     ]);
///
///     SseResponse::new(events)
///         .into_response()
/// }
/// ```
pub struct SseResponse<S> {
    stream: S,
    _config: SseConfig,
}

impl<S> SseResponse<S>
where
    S: Stream<Item = SseEvent> + Send + Unpin + 'static,
{
    /// Create a new SSE response from an event stream.
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            _config: SseConfig::default(),
        }
    }

    /// Create an SSE response with custom configuration.
    pub fn with_config(stream: S, config: SseConfig) -> Self {
        Self {
            stream,
            _config: config,
        }
    }

    /// Convert to an HTTP Response.
    ///
    /// Sets the appropriate headers for SSE:
    /// - `Content-Type: text/event-stream`
    /// - `Cache-Control: no-cache`
    /// - `Connection: keep-alive`
    #[must_use]
    pub fn into_response(self) -> Response {
        let sse_stream = SseStream::new(self.stream);

        Response::with_status(StatusCode::OK)
            .header("content-type", b"text/event-stream".to_vec())
            .header("cache-control", b"no-cache".to_vec())
            .header("connection", b"keep-alive".to_vec())
            .header("x-accel-buffering", b"no".to_vec()) // Disable nginx buffering
            .body(ResponseBody::stream(sse_stream))
    }
}

/// Convenience function to create an SSE response from an iterator.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::sse::{sse_response, SseEvent};
///
/// let events = vec![
///     SseEvent::message("Hello"),
///     SseEvent::message("World"),
/// ];
///
/// let response = sse_response(asupersync::stream::iter(events));
/// ```
pub fn sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = SseEvent> + Send + Unpin + 'static,
{
    SseResponse::new(stream).into_response()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_simple_message() {
        let event = SseEvent::message("Hello, World!");
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains("data: Hello, World!\n"));
        assert!(output.ends_with("\n\n"));
    }

    #[test]
    fn event_with_type() {
        let event = SseEvent::new("user joined").event_type("join");
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains("event: join\n"));
        assert!(output.contains("data: user joined\n"));
    }

    #[test]
    fn event_with_id() {
        let event = SseEvent::new("data").id("12345");
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains("id: 12345\n"));
    }

    #[test]
    fn event_with_retry() {
        let event = SseEvent::new("data").retry_ms(5000);
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains("retry: 5000\n"));
    }

    #[test]
    fn event_multiline_data() {
        let event = SseEvent::new("line1\nline2\nline3");
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains("data: line1\n"));
        assert!(output.contains("data: line2\n"));
        assert!(output.contains("data: line3\n"));
    }

    #[test]
    fn event_comment() {
        let event = SseEvent::comment("keep-alive");
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains(": keep-alive\n"));
    }

    #[test]
    fn event_full_format() {
        let event = SseEvent::new("payload")
            .event_type("update")
            .id("42")
            .retry_ms(3000);
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);

        // Check order: event, id, retry, data
        let event_pos = output.find("event:").unwrap();
        let id_pos = output.find("id:").unwrap();
        let retry_pos = output.find("retry:").unwrap();
        let data_pos = output.find("data:").unwrap();

        assert!(event_pos < id_pos);
        assert!(id_pos < retry_pos);
        assert!(retry_pos < data_pos);
    }

    #[test]
    fn event_from_str() {
        let event: SseEvent = "Hello".into();
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains("data: Hello\n"));
    }

    #[test]
    fn event_from_string() {
        let event: SseEvent = String::from("World").into();
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains("data: World\n"));
    }

    #[test]
    fn config_defaults() {
        let config = SseConfig::default();
        assert_eq!(config.keep_alive_secs, 30);
        assert_eq!(config.keep_alive_comment, "keep-alive");
    }

    #[test]
    fn config_custom() {
        let config = SseConfig::new()
            .keep_alive_secs(60)
            .keep_alive_comment("heartbeat");
        assert_eq!(config.keep_alive_secs, 60);
        assert_eq!(config.keep_alive_comment, "heartbeat");
    }

    #[test]
    fn config_disable_keepalive() {
        let config = SseConfig::new().disable_keep_alive();
        assert_eq!(config.keep_alive_secs, 0);
    }

    #[test]
    fn event_empty_data() {
        let event = SseEvent::new("");
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains("data: \n"));
    }

    #[test]
    fn retry_from_duration() {
        let event = SseEvent::new("data").retry(Duration::from_secs(10));
        let bytes = event.to_bytes();
        let output = String::from_utf8_lossy(&bytes);
        assert!(output.contains("retry: 10000\n"));
    }
}
