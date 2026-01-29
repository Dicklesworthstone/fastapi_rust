//! HTTP server with asupersync integration.
//!
//! This module provides a TCP server that uses asupersync for structured
//! concurrency and cancel-correct request handling.
//!
// NOTE: This module is scaffolding for Phase 1 TCP server implementation.
// Most types are defined but not yet wired into the main application.
#![allow(dead_code)]
//!
//! # Architecture
//!
//! The server creates a region hierarchy:
//!
//! ```text
//! Server Region (root)
//! ├── Connection Region 1
//! │   ├── Request Task 1 (with Cx, Budget)
//! │   ├── Request Task 2 (with Cx, Budget)
//! │   └── ...
//! ├── Connection Region 2
//! │   └── ...
//! └── ...
//! ```
//!
//! Each request runs with its own [`RequestContext`](fastapi_core::RequestContext)
//! that wraps the asupersync [`Cx`](asupersync::Cx), providing:
//!
//! - Cancel-correct request handling via checkpoints
//! - Budget-based request timeouts
//! - Structured concurrency for background work
//!
//! # Example
//!
//! ```ignore
//! use fastapi_http::TcpServer;
//! use fastapi_core::{RequestContext, Request, Response};
//!
//! async fn handler(ctx: &RequestContext, req: Request) -> Response {
//!     Response::ok().body("Hello, World!")
//! }
//!
//! let config = ServerConfig::new("127.0.0.1:8080");
//! let server = TcpServer::new(config);
//! server.serve(handler).await?;
//! ```

use crate::connection::should_keep_alive;
use crate::expect::{CONTINUE_RESPONSE, ExpectHandler, ExpectResult};
use crate::parser::{ParseError, ParseLimits, ParseStatus, Parser, StatefulParser};
use crate::response::{ResponseWrite, ResponseWriter};
use asupersync::io::{AsyncRead, AsyncWrite, ReadBuf};
use asupersync::net::{TcpListener, TcpStream};
use asupersync::runtime::{RuntimeState, SpawnError, TaskHandle};
use asupersync::signal::{GracefulOutcome, ShutdownController, ShutdownReceiver};
use asupersync::stream::Stream;
use asupersync::time::timeout;
use asupersync::{Budget, Cx, Scope, Time};
use fastapi_core::app::App;
use fastapi_core::{Request, RequestContext, Response, StatusCode};
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;
use std::time::{Duration, Instant};

/// Global start time for computing asupersync Time values.
/// This is lazily initialized on first use.
static START_TIME: OnceLock<Instant> = OnceLock::new();

/// Returns the current time as an asupersync Time value.
///
/// This uses wall clock time relative to a lazily-initialized start point,
/// which is compatible with asupersync's standalone timer mechanism.
fn current_time() -> Time {
    let start = START_TIME.get_or_init(Instant::now);
    let now = Instant::now();
    if now < *start {
        Time::ZERO
    } else {
        let elapsed = now.duration_since(*start);
        Time::from_nanos(elapsed.as_nanos() as u64)
    }
}

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Default read buffer size in bytes.
pub const DEFAULT_READ_BUFFER_SIZE: usize = 8192;

/// Default maximum connections (0 = unlimited).
pub const DEFAULT_MAX_CONNECTIONS: usize = 0;

/// Default keep-alive timeout in seconds (time to wait for next request).
pub const DEFAULT_KEEP_ALIVE_TIMEOUT_SECS: u64 = 75;

/// Default max requests per connection (0 = unlimited).
pub const DEFAULT_MAX_REQUESTS_PER_CONNECTION: usize = 100;

/// Default drain timeout in seconds (time to wait for in-flight requests on shutdown).
pub const DEFAULT_DRAIN_TIMEOUT_SECS: u64 = 30;

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Address to bind to.
    pub bind_addr: String,
    /// Default request timeout.
    pub request_timeout: Time,
    /// Maximum connections (0 = unlimited).
    pub max_connections: usize,
    /// Read buffer size.
    pub read_buffer_size: usize,
    /// HTTP parse limits.
    pub parse_limits: ParseLimits,
    /// Allowed hostnames for Host header validation (empty = allow all).
    pub allowed_hosts: Vec<String>,
    /// Whether to trust X-Forwarded-Host for host validation.
    pub trust_x_forwarded_host: bool,
    /// Enable TCP_NODELAY.
    pub tcp_nodelay: bool,
    /// Keep-alive timeout (time to wait for next request on a connection).
    /// Set to 0 to disable keep-alive timeout.
    pub keep_alive_timeout: Duration,
    /// Maximum requests per connection (0 = unlimited).
    pub max_requests_per_connection: usize,
    /// Drain timeout (time to wait for in-flight requests on shutdown).
    /// After this timeout, connections are forcefully closed.
    pub drain_timeout: Duration,
}

impl ServerConfig {
    /// Creates a new server configuration with the given bind address.
    #[must_use]
    pub fn new(bind_addr: impl Into<String>) -> Self {
        Self {
            bind_addr: bind_addr.into(),
            request_timeout: Time::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS),
            max_connections: DEFAULT_MAX_CONNECTIONS,
            read_buffer_size: DEFAULT_READ_BUFFER_SIZE,
            parse_limits: ParseLimits::default(),
            allowed_hosts: Vec::new(),
            trust_x_forwarded_host: false,
            tcp_nodelay: true,
            keep_alive_timeout: Duration::from_secs(DEFAULT_KEEP_ALIVE_TIMEOUT_SECS),
            max_requests_per_connection: DEFAULT_MAX_REQUESTS_PER_CONNECTION,
            drain_timeout: Duration::from_secs(DEFAULT_DRAIN_TIMEOUT_SECS),
        }
    }

    /// Sets the request timeout.
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Time) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Sets the request timeout in seconds.
    #[must_use]
    pub fn with_request_timeout_secs(mut self, secs: u64) -> Self {
        self.request_timeout = Time::from_secs(secs);
        self
    }

    /// Sets the maximum number of connections.
    #[must_use]
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Sets the read buffer size.
    #[must_use]
    pub fn with_read_buffer_size(mut self, size: usize) -> Self {
        self.read_buffer_size = size;
        self
    }

    /// Sets the HTTP parse limits.
    #[must_use]
    pub fn with_parse_limits(mut self, limits: ParseLimits) -> Self {
        self.parse_limits = limits;
        self
    }

    /// Sets allowed hosts for Host header validation.
    ///
    /// An empty list means "allow any host".
    #[must_use]
    pub fn with_allowed_hosts<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_hosts = hosts.into_iter().map(Into::into).collect();
        self
    }

    /// Adds a single allowed host.
    #[must_use]
    pub fn allow_host(mut self, host: impl Into<String>) -> Self {
        self.allowed_hosts.push(host.into());
        self
    }

    /// Enables or disables trust of X-Forwarded-Host.
    #[must_use]
    pub fn with_trust_x_forwarded_host(mut self, trust: bool) -> Self {
        self.trust_x_forwarded_host = trust;
        self
    }

    /// Enables or disables TCP_NODELAY.
    #[must_use]
    pub fn with_tcp_nodelay(mut self, enabled: bool) -> Self {
        self.tcp_nodelay = enabled;
        self
    }

    /// Sets the keep-alive timeout.
    ///
    /// This is the time to wait for another request on a keep-alive connection
    /// before closing it. Set to Duration::ZERO to disable keep-alive timeout.
    #[must_use]
    pub fn with_keep_alive_timeout(mut self, timeout: Duration) -> Self {
        self.keep_alive_timeout = timeout;
        self
    }

    /// Sets the keep-alive timeout in seconds.
    #[must_use]
    pub fn with_keep_alive_timeout_secs(mut self, secs: u64) -> Self {
        self.keep_alive_timeout = Duration::from_secs(secs);
        self
    }

    /// Sets the maximum requests per connection.
    ///
    /// Set to 0 for unlimited requests per connection.
    #[must_use]
    pub fn with_max_requests_per_connection(mut self, max: usize) -> Self {
        self.max_requests_per_connection = max;
        self
    }

    /// Sets the drain timeout.
    ///
    /// This is the time to wait for in-flight requests to complete during
    /// shutdown. After this timeout, connections are forcefully closed.
    #[must_use]
    pub fn with_drain_timeout(mut self, timeout: Duration) -> Self {
        self.drain_timeout = timeout;
        self
    }

    /// Sets the drain timeout in seconds.
    #[must_use]
    pub fn with_drain_timeout_secs(mut self, secs: u64) -> Self {
        self.drain_timeout = Duration::from_secs(secs);
        self
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self::new("127.0.0.1:8080")
    }
}

/// HTTP server error.
#[derive(Debug)]
pub enum ServerError {
    /// IO error.
    Io(io::Error),
    /// Parse error.
    Parse(ParseError),
    /// Server was shut down.
    Shutdown,
    /// Connection limit reached.
    ConnectionLimitReached,
    /// Keep-alive timeout expired (idle connection).
    KeepAliveTimeout,
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Parse(e) => write!(f, "Parse error: {e}"),
            Self::Shutdown => write!(f, "Server shutdown"),
            Self::ConnectionLimitReached => write!(f, "Connection limit reached"),
            Self::KeepAliveTimeout => write!(f, "Keep-alive timeout"),
        }
    }
}

impl std::error::Error for ServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse(e) => Some(e),
            _ => None,
        }
    }
}

// ============================================================================
// Host Header Validation
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostValidationErrorKind {
    Missing,
    Invalid,
    NotAllowed,
}

#[derive(Debug, Clone)]
struct HostValidationError {
    kind: HostValidationErrorKind,
    detail: String,
}

impl HostValidationError {
    fn missing() -> Self {
        Self {
            kind: HostValidationErrorKind::Missing,
            detail: "missing Host header".to_string(),
        }
    }

    fn invalid(detail: impl Into<String>) -> Self {
        Self {
            kind: HostValidationErrorKind::Invalid,
            detail: detail.into(),
        }
    }

    fn not_allowed(detail: impl Into<String>) -> Self {
        Self {
            kind: HostValidationErrorKind::NotAllowed,
            detail: detail.into(),
        }
    }

    fn response(&self) -> Response {
        let message = match self.kind {
            HostValidationErrorKind::Missing => "Bad Request: Host header required",
            HostValidationErrorKind::Invalid => "Bad Request: invalid Host header",
            HostValidationErrorKind::NotAllowed => "Bad Request: Host not allowed",
        };
        Response::with_status(StatusCode::BAD_REQUEST).body(fastapi_core::ResponseBody::Bytes(
            message.as_bytes().to_vec(),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HostHeader {
    host: String,
    port: Option<u16>,
}

fn validate_host_header(
    request: &Request,
    config: &ServerConfig,
) -> Result<HostHeader, HostValidationError> {
    let raw = extract_effective_host(request, config)?;
    let parsed = parse_host_header(&raw)
        .ok_or_else(|| HostValidationError::invalid(format!("invalid host value: {raw}")))?;

    if !is_allowed_host(&parsed, &config.allowed_hosts) {
        return Err(HostValidationError::not_allowed(format!(
            "host not allowed: {}",
            parsed.host
        )));
    }

    Ok(parsed)
}

fn extract_effective_host(
    request: &Request,
    config: &ServerConfig,
) -> Result<String, HostValidationError> {
    if config.trust_x_forwarded_host {
        if let Some(value) = header_value(request, "x-forwarded-host")? {
            let forwarded = extract_first_list_value(&value)
                .ok_or_else(|| HostValidationError::invalid("empty X-Forwarded-Host value"))?;
            return Ok(forwarded.to_string());
        }
    }

    match header_value(request, "host")? {
        Some(value) => Ok(value),
        None => Err(HostValidationError::missing()),
    }
}

fn header_value(request: &Request, name: &str) -> Result<Option<String>, HostValidationError> {
    request
        .headers()
        .get(name)
        .map(|bytes| {
            std::str::from_utf8(bytes)
                .map(|s| s.trim().to_string())
                .map_err(|_| {
                    HostValidationError::invalid(format!("invalid UTF-8 in {name} header"))
                })
        })
        .transpose()
}

fn extract_first_list_value(value: &str) -> Option<&str> {
    value.split(',').map(str::trim).find(|v| !v.is_empty())
}

fn parse_host_header(value: &str) -> Option<HostHeader> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }

    if value.starts_with('[') {
        let end = value.find(']')?;
        let host = &value[1..end];
        if host.is_empty() {
            return None;
        }
        if host.parse::<Ipv6Addr>().is_err() {
            return None;
        }
        let rest = &value[end + 1..];
        let port = if rest.is_empty() {
            None
        } else if let Some(port_str) = rest.strip_prefix(':') {
            parse_port(port_str)
        } else {
            return None;
        };
        return Some(HostHeader {
            host: host.to_ascii_lowercase(),
            port,
        });
    }

    let mut parts = value.split(':');
    let host = parts.next().unwrap_or("");
    let port_part = parts.next();
    if parts.next().is_some() {
        // Multiple colons without brackets (likely IPv6) are invalid
        return None;
    }
    if host.is_empty() {
        return None;
    }

    let port = match port_part {
        Some(p) => parse_port(p),
        None => None,
    };

    if host.parse::<Ipv4Addr>().is_ok() || is_valid_hostname(host) {
        Some(HostHeader {
            host: host.to_ascii_lowercase(),
            port,
        })
    } else {
        None
    }
}

fn parse_port(port: &str) -> Option<u16> {
    if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value = port.parse::<u16>().ok()?;
    if value == 0 { None } else { Some(value) }
}

fn is_valid_hostname(host: &str) -> bool {
    if host.len() > 253 {
        return false;
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        let bytes = label.as_bytes();
        if bytes.first() == Some(&b'-') || bytes.last() == Some(&b'-') {
            return false;
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }
    true
}

fn is_allowed_host(host: &HostHeader, allowed_hosts: &[String]) -> bool {
    if allowed_hosts.is_empty() {
        return true;
    }

    allowed_hosts
        .iter()
        .any(|pattern| host_matches_pattern(host, pattern))
}

fn host_matches_pattern(host: &HostHeader, pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        let suffix = suffix.to_ascii_lowercase();
        if host.host == suffix {
            return false;
        }
        return host.host.ends_with(&format!(".{suffix}"));
    }

    if let Some(parsed) = parse_host_header(pattern) {
        if parsed.host != host.host {
            return false;
        }
        if let Some(port) = parsed.port {
            return host.port == Some(port);
        }
        return true;
    }

    false
}

impl From<io::Error> for ServerError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ParseError> for ServerError {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}

/// Processes a connection with the given handler.
///
/// This is the unified connection handling logic used by all server modes.
async fn process_connection<H, Fut>(
    cx: &Cx,
    request_counter: &AtomicU64,
    mut stream: TcpStream,
    _peer_addr: SocketAddr,
    config: &ServerConfig,
    handler: H,
) -> Result<(), ServerError>
where
    H: Fn(RequestContext, &mut Request) -> Fut,
    Fut: Future<Output = Response>,
{
    let mut parser = StatefulParser::new().with_limits(config.parse_limits.clone());
    let mut read_buffer = vec![0u8; config.read_buffer_size];
    let mut response_writer = ResponseWriter::new();
    let mut requests_on_connection: usize = 0;
    let max_requests = config.max_requests_per_connection;

    loop {
        // Check for cancellation
        if cx.is_cancel_requested() {
            return Ok(());
        }

        // Try to parse a complete request from buffered data first
        let parse_result = parser.feed(&[])?;

        let mut request = match parse_result {
            ParseStatus::Complete { request, .. } => request,
            ParseStatus::Incomplete => {
                let keep_alive_timeout = config.keep_alive_timeout;

                let bytes_read = if keep_alive_timeout.is_zero() {
                    read_into_buffer(&mut stream, &mut read_buffer).await?
                } else {
                    match read_with_timeout(&mut stream, &mut read_buffer, keep_alive_timeout).await
                    {
                        Ok(0) => return Ok(()),
                        Ok(n) => n,
                        Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                            cx.trace(&format!(
                                "Keep-alive timeout ({:?}) - closing idle connection",
                                keep_alive_timeout
                            ));
                            return Err(ServerError::KeepAliveTimeout);
                        }
                        Err(e) => return Err(ServerError::Io(e)),
                    }
                };

                if bytes_read == 0 {
                    return Ok(());
                }

                match parser.feed(&read_buffer[..bytes_read])? {
                    ParseStatus::Complete { request, .. } => request,
                    ParseStatus::Incomplete => continue,
                }
            }
        };

        requests_on_connection += 1;

        // Generate unique request ID for this request with timeout budget
        let request_id = request_counter.fetch_add(1, Ordering::Relaxed);
        let request_budget = Budget::new().with_deadline(config.request_timeout);
        let request_cx = Cx::for_testing_with_budget(request_budget);
        let ctx = RequestContext::new(request_cx, request_id);

        // Validate Host header
        if let Err(err) = validate_host_header(&request, config) {
            ctx.trace(&format!("Rejecting request: {}", err.detail));
            let response = err.response().header("connection", b"close".to_vec());
            let response_write = response_writer.write(response);
            write_response(&mut stream, response_write).await?;
            return Ok(());
        }

        // Handle Expect: 100-continue
        // RFC 7231 Section 5.1.1: If the server receives a request with Expect: 100-continue,
        // it should either send 100 Continue (to proceed) or a final status code (to reject).
        match ExpectHandler::check_expect(&request) {
            ExpectResult::NoExpectation => {
                // No Expect header - proceed normally
            }
            ExpectResult::ExpectsContinue => {
                // Expect: 100-continue present
                // Send 100 Continue to tell client to proceed with body
                // Note: In a full implementation, pre-body validation hooks would run here
                // to validate auth, content-type, content-length before accepting the body.
                ctx.trace("Sending 100 Continue for Expect: 100-continue");
                write_raw_response(&mut stream, CONTINUE_RESPONSE).await?;
            }
            ExpectResult::UnknownExpectation(value) => {
                // Unknown expectation - return 417 Expectation Failed
                ctx.trace(&format!("Rejecting unknown Expect value: {}", value));
                let response =
                    ExpectHandler::expectation_failed(format!("Unsupported Expect value: {value}"));
                let response_write = response_writer.write(response);
                write_response(&mut stream, response_write).await?;
                return Ok(());
            }
        }

        let client_wants_keep_alive = should_keep_alive(&request);
        let at_max_requests = max_requests > 0 && requests_on_connection >= max_requests;
        let server_will_keep_alive = client_wants_keep_alive && !at_max_requests;

        let request_start = Instant::now();
        let timeout_duration = Duration::from_nanos(config.request_timeout.as_nanos());

        // Call the handler
        let response = handler(ctx, &mut request).await;

        let mut response = if request_start.elapsed() > timeout_duration {
            Response::with_status(StatusCode::GATEWAY_TIMEOUT).body(
                fastapi_core::ResponseBody::Bytes(
                    b"Gateway Timeout: request processing exceeded time limit".to_vec(),
                ),
            )
        } else {
            response
        };

        response = if server_will_keep_alive {
            response.header("connection", b"keep-alive".to_vec())
        } else {
            response.header("connection", b"close".to_vec())
        };

        let response_write = response_writer.write(response);
        write_response(&mut stream, response_write).await?;

        if let Some(tasks) = App::take_background_tasks(&mut request) {
            tasks.execute_all().await;
        }

        if !server_will_keep_alive {
            return Ok(());
        }
    }
}

/// TCP server with asupersync integration.
///
/// This server manages the lifecycle of connections and requests using
/// asupersync's structured concurrency primitives. Each connection runs
/// in its own region, and each request gets its own task with a budget.
#[derive(Debug)]
pub struct TcpServer {
    config: ServerConfig,
    request_counter: Arc<AtomicU64>,
    /// Current number of active connections (wrapped in Arc for concurrent feature).
    connection_counter: Arc<AtomicU64>,
    /// Whether the server is draining (shutting down gracefully).
    draining: Arc<AtomicBool>,
    /// Handles to spawned connection tasks for graceful shutdown.
    connection_handles: Mutex<Vec<TaskHandle<()>>>,
    /// Shutdown controller for coordinated graceful shutdown.
    shutdown_controller: Arc<ShutdownController>,
    /// Connection pool metrics counters.
    metrics_counters: Arc<MetricsCounters>,
}

impl TcpServer {
    /// Creates a new TCP server with the given configuration.
    #[must_use]
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            request_counter: Arc::new(AtomicU64::new(0)),
            connection_counter: Arc::new(AtomicU64::new(0)),
            draining: Arc::new(AtomicBool::new(false)),
            connection_handles: Mutex::new(Vec::new()),
            shutdown_controller: Arc::new(ShutdownController::new()),
            metrics_counters: Arc::new(MetricsCounters::new()),
        }
    }

    /// Returns the server configuration.
    #[must_use]
    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Generates a unique request ID.
    fn next_request_id(&self) -> u64 {
        self.request_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Returns the current number of active connections.
    #[must_use]
    pub fn current_connections(&self) -> u64 {
        self.connection_counter.load(Ordering::Relaxed)
    }

    /// Returns a snapshot of the server's connection pool metrics.
    #[must_use]
    pub fn metrics(&self) -> ServerMetrics {
        ServerMetrics {
            active_connections: self.connection_counter.load(Ordering::Relaxed),
            total_accepted: self.metrics_counters.total_accepted.load(Ordering::Relaxed),
            total_rejected: self.metrics_counters.total_rejected.load(Ordering::Relaxed),
            total_timed_out: self.metrics_counters.total_timed_out.load(Ordering::Relaxed),
            total_requests: self.request_counter.load(Ordering::Relaxed),
            bytes_in: self.metrics_counters.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.metrics_counters.bytes_out.load(Ordering::Relaxed),
        }
    }

    /// Records bytes read from a client.
    fn record_bytes_in(&self, n: u64) {
        self.metrics_counters.bytes_in.fetch_add(n, Ordering::Relaxed);
    }

    /// Records bytes written to a client.
    fn record_bytes_out(&self, n: u64) {
        self.metrics_counters.bytes_out.fetch_add(n, Ordering::Relaxed);
    }

    /// Attempts to acquire a connection slot.
    ///
    /// Returns true if a slot was acquired, false if the connection limit
    /// has been reached. If max_connections is 0 (unlimited), always returns true.
    fn try_acquire_connection(&self) -> bool {
        let max = self.config.max_connections;
        if max == 0 {
            // Unlimited connections
            self.connection_counter.fetch_add(1, Ordering::Relaxed);
            self.metrics_counters.total_accepted.fetch_add(1, Ordering::Relaxed);
            return true;
        }

        // Try to increment if under limit
        let mut current = self.connection_counter.load(Ordering::Relaxed);
        loop {
            if current >= max as u64 {
                self.metrics_counters.total_rejected.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            match self.connection_counter.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.metrics_counters.total_accepted.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
                Err(actual) => current = actual,
            }
        }
    }

    /// Releases a connection slot.
    fn release_connection(&self) {
        self.connection_counter.fetch_sub(1, Ordering::Relaxed);
    }

    /// Returns true if the server is draining (shutting down gracefully).
    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Acquire)
    }

    /// Starts the drain process for graceful shutdown.
    ///
    /// This sets the draining flag, which causes the server to:
    /// - Stop accepting new connections
    /// - Return 503 to new connection attempts
    /// - Allow in-flight requests to complete
    pub fn start_drain(&self) {
        self.draining.store(true, Ordering::Release);
    }

    /// Waits for all in-flight connections to drain, with a timeout.
    ///
    /// Returns `true` if all connections drained successfully,
    /// `false` if the timeout was reached with connections still active.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time to wait for connections to drain
    /// * `poll_interval` - How often to check connection count (default 10ms)
    pub async fn wait_for_drain(&self, timeout: Duration, poll_interval: Option<Duration>) -> bool {
        let start = Instant::now();
        let poll_interval = poll_interval.unwrap_or(Duration::from_millis(10));

        while self.current_connections() > 0 {
            if start.elapsed() >= timeout {
                return false;
            }
            // Yield to allow other tasks to make progress
            // In production, this would use proper async sleep
            std::thread::sleep(poll_interval);
        }
        true
    }

    /// Initiates graceful shutdown and waits for connections to drain.
    ///
    /// This is a convenience method that combines `start_drain()` and
    /// `wait_for_drain()` using the configured drain timeout.
    ///
    /// Returns the number of connections that were forcefully closed
    /// (0 if all drained within the timeout).
    pub async fn drain(&self) -> u64 {
        self.start_drain();
        let drained = self.wait_for_drain(self.config.drain_timeout, None).await;
        if drained {
            0
        } else {
            self.current_connections()
        }
    }

    /// Returns a reference to the server's shutdown controller.
    ///
    /// This can be used to coordinate shutdown from external code,
    /// such as signal handlers or health check endpoints.
    #[must_use]
    pub fn shutdown_controller(&self) -> &Arc<ShutdownController> {
        &self.shutdown_controller
    }

    /// Returns a receiver for shutdown notifications.
    ///
    /// Use this to receive shutdown signals in other parts of your application.
    /// Multiple receivers can be created and they will all be notified.
    #[must_use]
    pub fn subscribe_shutdown(&self) -> ShutdownReceiver {
        self.shutdown_controller.subscribe()
    }

    /// Initiates server shutdown.
    ///
    /// This triggers the shutdown process:
    /// 1. Sets the draining flag to stop accepting new connections
    /// 2. Notifies all shutdown receivers
    /// 3. The server's accept loop will exit and drain connections
    ///
    /// This method is safe to call multiple times - subsequent calls are no-ops.
    pub fn shutdown(&self) {
        self.start_drain();
        self.shutdown_controller.shutdown();
    }

    /// Checks if shutdown has been initiated.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown_controller.is_shutting_down() || self.is_draining()
    }

    /// Runs the server with graceful shutdown support.
    ///
    /// The server will run until either:
    /// - The provided shutdown receiver signals shutdown
    /// - The server Cx is cancelled
    /// - An unrecoverable error occurs
    ///
    /// When shutdown is signaled, the server will:
    /// 1. Stop accepting new connections
    /// 2. Wait for existing connections to complete (up to drain_timeout)
    /// 3. Return gracefully
    ///
    /// # Example
    ///
    /// ```ignore
    /// use asupersync::signal::ShutdownController;
    /// use fastapi_http::{TcpServer, ServerConfig};
    ///
    /// let controller = ShutdownController::new();
    /// let server = TcpServer::new(ServerConfig::new("127.0.0.1:8080"));
    ///
    /// // Get a shutdown receiver
    /// let shutdown = controller.subscribe();
    ///
    /// // In another task, you can trigger shutdown:
    /// // controller.shutdown();
    ///
    /// server.serve_with_shutdown(&cx, shutdown, handler).await?;
    /// ```
    pub async fn serve_with_shutdown<H, Fut>(
        &self,
        cx: &Cx,
        mut shutdown: ShutdownReceiver,
        handler: H,
    ) -> Result<GracefulOutcome<()>, ServerError>
    where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let bind_addr = self.config.bind_addr.clone();
        let listener = TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?;

        cx.trace(&format!(
            "Server listening on {local_addr} (with graceful shutdown)"
        ));

        // Run the accept loop with shutdown racing
        let result = self
            .accept_loop_with_shutdown(cx, listener, handler, &mut shutdown)
            .await;

        match result {
            Ok(outcome) => {
                if outcome.is_shutdown() {
                    cx.trace("Shutdown signal received, draining connections");
                    self.start_drain();
                    self.drain_connection_tasks(cx).await;
                }
                Ok(outcome)
            }
            Err(e) => Err(e),
        }
    }

    /// Accept loop that checks for shutdown signals.
    async fn accept_loop_with_shutdown<H, Fut>(
        &self,
        cx: &Cx,
        listener: TcpListener,
        handler: H,
        shutdown: &mut ShutdownReceiver,
    ) -> Result<GracefulOutcome<()>, ServerError>
    where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let handler = Arc::new(handler);

        loop {
            // Check for shutdown or cancellation first
            if shutdown.is_shutting_down() {
                return Ok(GracefulOutcome::ShutdownSignaled);
            }
            if cx.is_cancel_requested() || self.is_draining() {
                return Ok(GracefulOutcome::ShutdownSignaled);
            }

            // Accept a connection
            let (mut stream, peer_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => {
                    cx.trace(&format!("Accept error: {e}"));
                    if is_fatal_accept_error(&e) {
                        return Err(ServerError::Io(e));
                    }
                    continue;
                }
            };

            // Check connection limit before processing
            if !self.try_acquire_connection() {
                cx.trace(&format!(
                    "Connection limit reached ({}), rejecting {peer_addr}",
                    self.config.max_connections
                ));

                let response = Response::with_status(StatusCode::SERVICE_UNAVAILABLE)
                    .header("connection", b"close".to_vec())
                    .body(fastapi_core::ResponseBody::Bytes(
                        b"503 Service Unavailable: connection limit reached".to_vec(),
                    ));
                let mut writer = crate::response::ResponseWriter::new();
                let response_bytes = writer.write(response);
                let _ = write_response(&mut stream, response_bytes).await;
                continue;
            }

            // Configure the connection
            if self.config.tcp_nodelay {
                let _ = stream.set_nodelay(true);
            }

            cx.trace(&format!(
                "Accepted connection from {peer_addr} ({}/{})",
                self.current_connections(),
                if self.config.max_connections == 0 {
                    "∞".to_string()
                } else {
                    self.config.max_connections.to_string()
                }
            ));

            let request_id = self.next_request_id();
            let request_budget = Budget::new().with_deadline(self.config.request_timeout);
            let request_cx = Cx::for_testing_with_budget(request_budget);
            let ctx = RequestContext::new(request_cx, request_id);

            // Handle connection inline (single-threaded mode)
            let result = self
                .handle_connection(&ctx, stream, peer_addr, &*handler)
                .await;

            self.release_connection();

            if let Err(e) = result {
                cx.trace(&format!("Connection error from {peer_addr}: {e}"));
            }
        }
    }

    /// Runs the server, accepting connections and handling requests.
    ///
    /// This method will run until the server Cx is cancelled or an
    /// unrecoverable error occurs.
    ///
    /// # Arguments
    ///
    /// * `cx` - The capability context for the server region
    /// * `handler` - The request handler function
    ///
    /// # Errors
    ///
    /// Returns an error if binding fails or an unrecoverable IO error occurs.
    pub async fn serve<H, Fut>(&self, cx: &Cx, handler: H) -> Result<(), ServerError>
    where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let bind_addr = self.config.bind_addr.clone();
        let listener = TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?;

        cx.trace(&format!("Server listening on {local_addr}"));

        self.accept_loop(cx, listener, handler).await
    }

    /// Runs the server on a specific listener.
    ///
    /// This is useful when you already have a bound listener.
    pub async fn serve_on<H, Fut>(
        &self,
        cx: &Cx,
        listener: TcpListener,
        handler: H,
    ) -> Result<(), ServerError>
    where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.accept_loop(cx, listener, handler).await
    }

    /// Runs the server with a Handler trait object.
    ///
    /// This is the recommended way to serve an application that implements
    /// the [`Handler`] trait (like [`App`]).
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_http::TcpServer;
    /// use fastapi_core::{App, Handler};
    /// use std::sync::Arc;
    ///
    /// let app = App::builder()
    ///     .get("/", handler_fn)
    ///     .build();
    ///
    /// let server = TcpServer::new(ServerConfig::new("127.0.0.1:8080"));
    /// let cx = Cx::for_testing();
    /// server.serve_handler(&cx, Arc::new(app)).await?;
    /// ```
    pub async fn serve_handler(
        &self,
        cx: &Cx,
        handler: Arc<dyn fastapi_core::Handler>,
    ) -> Result<(), ServerError> {
        let bind_addr = self.config.bind_addr.clone();
        let listener = TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?;

        cx.trace(&format!("Server listening on {local_addr}"));

        self.accept_loop_handler(cx, listener, handler).await
    }

    /// Runs the server on a specific listener with a Handler trait object.
    pub async fn serve_on_handler(
        &self,
        cx: &Cx,
        listener: TcpListener,
        handler: Arc<dyn fastapi_core::Handler>,
    ) -> Result<(), ServerError> {
        self.accept_loop_handler(cx, listener, handler).await
    }

    /// Accept loop for Handler trait objects.
    async fn accept_loop_handler(
        &self,
        cx: &Cx,
        listener: TcpListener,
        handler: Arc<dyn fastapi_core::Handler>,
    ) -> Result<(), ServerError> {
        loop {
            // Check for cancellation at each iteration.
            if cx.is_cancel_requested() {
                cx.trace("Server shutdown requested");
                return Ok(());
            }

            // Check if draining (graceful shutdown)
            if self.is_draining() {
                cx.trace("Server draining, stopping accept loop");
                return Err(ServerError::Shutdown);
            }

            // Accept a connection.
            let (mut stream, peer_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => {
                    cx.trace(&format!("Accept error: {e}"));
                    if is_fatal_accept_error(&e) {
                        return Err(ServerError::Io(e));
                    }
                    continue;
                }
            };

            // Check connection limit before processing
            if !self.try_acquire_connection() {
                cx.trace(&format!(
                    "Connection limit reached ({}), rejecting {peer_addr}",
                    self.config.max_connections
                ));

                let response = Response::with_status(StatusCode::SERVICE_UNAVAILABLE)
                    .header("connection", b"close".to_vec())
                    .body(fastapi_core::ResponseBody::Bytes(
                        b"503 Service Unavailable: connection limit reached".to_vec(),
                    ));
                let mut writer = crate::response::ResponseWriter::new();
                let response_bytes = writer.write(response);
                let _ = write_response(&mut stream, response_bytes).await;
                continue;
            }

            // Configure the connection.
            if self.config.tcp_nodelay {
                let _ = stream.set_nodelay(true);
            }

            cx.trace(&format!(
                "Accepted connection from {peer_addr} ({}/{})",
                self.current_connections(),
                if self.config.max_connections == 0 {
                    "∞".to_string()
                } else {
                    self.config.max_connections.to_string()
                }
            ));

            // Handle the connection with the Handler trait object
            let result = self
                .handle_connection_handler(cx, stream, peer_addr, &*handler)
                .await;

            self.release_connection();

            if let Err(e) = result {
                cx.trace(&format!("Connection error from {peer_addr}: {e}"));
            }
        }
    }

    /// Serves HTTP requests with concurrent connection handling using asupersync Scope.
    ///
    /// This method uses `Scope::spawn_registered` for proper structured concurrency,
    /// ensuring all spawned connection tasks are tracked and can be properly drained
    /// during shutdown.
    ///
    /// # Arguments
    ///
    /// * `cx` - The asupersync context for cancellation and tracing
    /// * `scope` - A scope for spawning connection tasks
    /// * `state` - Runtime state for task registration
    /// * `handler` - The request handler
    #[allow(clippy::too_many_lines)]
    pub async fn serve_concurrent<H, Fut>(
        &self,
        cx: &Cx,
        scope: &Scope<'_>,
        state: &mut RuntimeState,
        handler: H,
    ) -> Result<(), ServerError>
    where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let bind_addr = self.config.bind_addr.clone();
        let listener = TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?;

        cx.trace(&format!(
            "Server listening on {local_addr} (concurrent mode)"
        ));

        let handler = Arc::new(handler);

        self.accept_loop_concurrent(cx, scope, state, listener, handler)
            .await
    }

    /// Accept loop that spawns connection handlers concurrently using Scope.
    async fn accept_loop_concurrent<H, Fut>(
        &self,
        cx: &Cx,
        scope: &Scope<'_>,
        state: &mut RuntimeState,
        listener: TcpListener,
        handler: Arc<H>,
    ) -> Result<(), ServerError>
    where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        loop {
            // Check for cancellation or drain
            if cx.is_cancel_requested() || self.is_draining() {
                cx.trace("Server shutting down, draining connections");
                self.drain_connection_tasks(cx).await;
                return Ok(());
            }

            // Accept a connection
            let (mut stream, peer_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(e) => {
                    cx.trace(&format!("Accept error: {e}"));
                    if is_fatal_accept_error(&e) {
                        return Err(ServerError::Io(e));
                    }
                    continue;
                }
            };

            // Check connection limit before processing
            if !self.try_acquire_connection() {
                cx.trace(&format!(
                    "Connection limit reached ({}), rejecting {peer_addr}",
                    self.config.max_connections
                ));

                let response = Response::with_status(StatusCode::SERVICE_UNAVAILABLE)
                    .header("connection", b"close".to_vec())
                    .body(fastapi_core::ResponseBody::Bytes(
                        b"503 Service Unavailable: connection limit reached".to_vec(),
                    ));
                let mut writer = crate::response::ResponseWriter::new();
                let response_bytes = writer.write(response);
                let _ = write_response(&mut stream, response_bytes).await;
                continue;
            }

            // Configure the connection
            if self.config.tcp_nodelay {
                let _ = stream.set_nodelay(true);
            }

            cx.trace(&format!(
                "Accepted connection from {peer_addr} ({}/{})",
                self.current_connections(),
                if self.config.max_connections == 0 {
                    "∞".to_string()
                } else {
                    self.config.max_connections.to_string()
                }
            ));

            // Spawn connection task using Scope
            match self.spawn_connection_task(
                scope,
                state,
                cx,
                stream,
                peer_addr,
                Arc::clone(&handler),
            ) {
                Ok(handle) => {
                    // Store handle for draining
                    if let Ok(mut handles) = self.connection_handles.lock() {
                        handles.push(handle);
                    }
                    // Periodically clean up completed handles
                    self.cleanup_completed_handles();
                }
                Err(e) => {
                    cx.trace(&format!("Failed to spawn connection task: {e:?}"));
                    self.release_connection();
                }
            }
        }
    }

    /// Spawns a connection handler task using Scope::spawn_registered.
    fn spawn_connection_task<H, Fut>(
        &self,
        scope: &Scope<'_>,
        state: &mut RuntimeState,
        cx: &Cx,
        stream: TcpStream,
        peer_addr: SocketAddr,
        handler: Arc<H>,
    ) -> Result<TaskHandle<()>, SpawnError>
    where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let config = self.config.clone();
        let request_counter = Arc::clone(&self.request_counter);
        let connection_counter = Arc::clone(&self.connection_counter);

        scope.spawn_registered(state, cx, move |task_cx| async move {
            let result = process_connection(
                &task_cx,
                &request_counter,
                stream,
                peer_addr,
                &config,
                |ctx, req| handler(ctx, req),
            )
            .await;

            // Release connection slot (always, regardless of success/failure)
            connection_counter.fetch_sub(1, Ordering::Relaxed);

            if let Err(e) = result {
                // Log error - in production this would use proper logging
                eprintln!("Connection error from {peer_addr}: {e}");
            }
        })
    }

    /// Removes completed task handles from the tracking vector.
    fn cleanup_completed_handles(&self) {
        if let Ok(mut handles) = self.connection_handles.lock() {
            handles.retain(|handle| !handle.is_finished());
        }
    }

    /// Drains all connection tasks during shutdown.
    async fn drain_connection_tasks(&self, cx: &Cx) {
        let drain_timeout = self.config.drain_timeout;
        let start = Instant::now();

        cx.trace(&format!(
            "Draining {} connection tasks (timeout: {:?})",
            self.connection_handles.lock().map_or(0, |h| h.len()),
            drain_timeout
        ));

        // Wait for all tasks to complete or timeout
        while start.elapsed() < drain_timeout {
            let remaining = self
                .connection_handles
                .lock()
                .map_or(0, |h| h.iter().filter(|t| !t.is_finished()).count());

            if remaining == 0 {
                cx.trace("All connection tasks drained successfully");
                return;
            }

            // Yield to allow tasks to make progress
            asupersync::runtime::yield_now().await;
        }

        cx.trace(&format!(
            "Drain timeout reached with {} tasks still running",
            self.connection_handles
                .lock()
                .map_or(0, |h| h.iter().filter(|t| !t.is_finished()).count())
        ));
    }

    /// Handles a single connection using the Handler trait.
    ///
    /// This is a specialized version for trait objects where we cannot use a closure
    /// due to lifetime constraints of BoxFuture.
    async fn handle_connection_handler(
        &self,
        cx: &Cx,
        mut stream: TcpStream,
        _peer_addr: SocketAddr,
        handler: &dyn fastapi_core::Handler,
    ) -> Result<(), ServerError> {
        let mut parser = StatefulParser::new().with_limits(self.config.parse_limits.clone());
        let mut read_buffer = vec![0u8; self.config.read_buffer_size];
        let mut response_writer = ResponseWriter::new();
        let mut requests_on_connection: usize = 0;
        let max_requests = self.config.max_requests_per_connection;

        loop {
            // Check for cancellation
            if cx.is_cancel_requested() {
                return Ok(());
            }

            // Parse request from connection
            let parse_result = parser.feed(&[])?;

            let mut request = match parse_result {
                ParseStatus::Complete { request, .. } => request,
                ParseStatus::Incomplete => {
                    let keep_alive_timeout = self.config.keep_alive_timeout;
                    let bytes_read = if keep_alive_timeout.is_zero() {
                        read_into_buffer(&mut stream, &mut read_buffer).await?
                    } else {
                        match read_with_timeout(&mut stream, &mut read_buffer, keep_alive_timeout)
                            .await
                        {
                            Ok(0) => return Ok(()),
                            Ok(n) => n,
                            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                                self.metrics_counters.total_timed_out.fetch_add(1, Ordering::Relaxed);
                                return Err(ServerError::KeepAliveTimeout);
                            }
                            Err(e) => return Err(ServerError::Io(e)),
                        }
                    };

                    if bytes_read == 0 {
                        return Ok(());
                    }

                    self.record_bytes_in(bytes_read as u64);

                    match parser.feed(&read_buffer[..bytes_read])? {
                        ParseStatus::Complete { request, .. } => request,
                        ParseStatus::Incomplete => continue,
                    }
                }
            };

            requests_on_connection += 1;

            // Create request context
            let request_id = self.request_counter.fetch_add(1, Ordering::Relaxed);
            let request_budget = Budget::new().with_deadline(self.config.request_timeout);
            let request_cx = Cx::for_testing_with_budget(request_budget);
            let ctx = RequestContext::new(request_cx, request_id);

            // Call handler - ctx lives until after await
            let response = handler.call(&ctx, &mut request).await;

            // Determine keep-alive behavior
            let client_wants_keep_alive = should_keep_alive(&request);
            let server_will_keep_alive = client_wants_keep_alive
                && (max_requests == 0 || requests_on_connection < max_requests);

            let response = if server_will_keep_alive {
                response.header("connection", b"keep-alive".to_vec())
            } else {
                response.header("connection", b"close".to_vec())
            };

            let response_write = response_writer.write(response);
            if let ResponseWrite::Full(ref bytes) = response_write {
                self.record_bytes_out(bytes.len() as u64);
            }
            write_response(&mut stream, response_write).await?;

            if !server_will_keep_alive {
                return Ok(());
            }
        }
    }

    /// The main accept loop.
    async fn accept_loop<H, Fut>(
        &self,
        cx: &Cx,
        listener: TcpListener,
        handler: H,
    ) -> Result<(), ServerError>
    where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let handler = Arc::new(handler);

        loop {
            // Check for cancellation at each iteration.
            if cx.is_cancel_requested() {
                cx.trace("Server shutdown requested");
                return Ok(());
            }

            // Check if draining (graceful shutdown)
            if self.is_draining() {
                cx.trace("Server draining, stopping accept loop");
                return Err(ServerError::Shutdown);
            }

            // Accept a connection.
            let (mut stream, peer_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    // Yield and retry.
                    continue;
                }
                Err(e) => {
                    cx.trace(&format!("Accept error: {e}"));
                    // For most errors, we continue accepting.
                    // Only fatal errors should propagate.
                    if is_fatal_accept_error(&e) {
                        return Err(ServerError::Io(e));
                    }
                    continue;
                }
            };

            // Check connection limit before processing
            if !self.try_acquire_connection() {
                cx.trace(&format!(
                    "Connection limit reached ({}), rejecting {peer_addr}",
                    self.config.max_connections
                ));

                // Send a 503 Service Unavailable response and close
                let response = Response::with_status(StatusCode::SERVICE_UNAVAILABLE)
                    .header("connection", b"close".to_vec())
                    .body(fastapi_core::ResponseBody::Bytes(
                        b"503 Service Unavailable: connection limit reached".to_vec(),
                    ));
                let mut writer = crate::response::ResponseWriter::new();
                let response_bytes = writer.write(response);
                let _ = write_response(&mut stream, response_bytes).await;
                continue;
            }

            // Configure the connection.
            if self.config.tcp_nodelay {
                let _ = stream.set_nodelay(true);
            }

            cx.trace(&format!(
                "Accepted connection from {peer_addr} ({}/{})",
                self.current_connections(),
                if self.config.max_connections == 0 {
                    "∞".to_string()
                } else {
                    self.config.max_connections.to_string()
                }
            ));

            // Spawn connection handling concurrently when the feature is enabled.
            // When asupersync has an accessible spawn API from Cx, we can use that
            // for proper structured concurrency. For now, use tokio::spawn.
            #[cfg(feature = "concurrent")]
            {
                self.spawn_connection_handler(cx.clone(), stream, peer_addr, Arc::clone(&handler));
            }

            // Without the concurrent feature, handle inline (blocking accept loop).
            // This is simpler but means only one connection is handled at a time.
            #[cfg(not(feature = "concurrent"))]
            {
                let request_id = self.next_request_id();
                let request_budget = Budget::new().with_deadline(self.config.request_timeout);

                // Create a RequestContext for this request with the configured timeout budget.
                // In the full implementation, the Cx would be derived from the connection region.
                // For now, we use for_testing_with_budget to apply the timeout.
                let request_cx = Cx::for_testing_with_budget(request_budget);
                let ctx = RequestContext::new(request_cx, request_id);

                // Handle the connection and release the slot when done.
                let result = self
                    .handle_connection(&ctx, stream, peer_addr, &*handler)
                    .await;

                // Release connection slot (always, regardless of success/failure)
                self.release_connection();

                if let Err(e) = result {
                    cx.trace(&format!("Connection error from {peer_addr}: {e}"));
                }
            }
        }
    }

    /// Spawns a connection handler as a separate task.
    ///
    /// This is used when the `concurrent` feature is enabled to handle
    /// connections concurrently without blocking the accept loop.
    ///
    /// When asupersync has an accessible spawn API from Cx, this should be
    /// migrated to use that for proper structured concurrency.
    #[cfg(feature = "concurrent")]
    fn spawn_connection_handler<H, Fut>(
        &self,
        server_cx: Cx,
        stream: TcpStream,
        peer_addr: SocketAddr,
        handler: Arc<H>,
    ) where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        // Clone values needed for the spawned task
        let config = self.config.clone();
        let request_counter = Arc::clone(&self.request_counter);
        let connection_counter = Arc::clone(&self.connection_counter);

        // Spawn the connection handler
        // Note: Using tokio::spawn as a transitional solution.
        // When asupersync's Scope::spawn is accessible from Cx, migrate to that.
        tokio::spawn(async move {
            let result = process_connection(
                &server_cx,
                &request_counter,
                stream,
                peer_addr,
                &config,
                |ctx, req| handler(ctx, req),
            )
            .await;

            // Release connection slot (always, regardless of success/failure)
            connection_counter.fetch_sub(1, Ordering::Relaxed);

            if let Err(e) = result {
                server_cx.trace(&format!("Connection error from {peer_addr}: {e}"));
            }
        });
    }

    /// Handles a single connection.
    ///
    /// This reads requests from the connection, passes them to the handler,
    /// and sends responses. For HTTP/1.1, it handles keep-alive by processing
    /// multiple requests on the same connection.
    async fn handle_connection<H, Fut>(
        &self,
        ctx: &RequestContext,
        stream: TcpStream,
        peer_addr: SocketAddr,
        handler: &H,
    ) -> Result<(), ServerError>
    where
        H: Fn(RequestContext, &mut Request) -> Fut + Send + Sync,
        Fut: Future<Output = Response> + Send,
    {
        process_connection(
            ctx.cx(),
            &self.request_counter,
            stream,
            peer_addr,
            &self.config,
            |ctx, req| handler(ctx, req),
        )
        .await
    }
}

/// Snapshot of server metrics at a point in time.
///
/// Returned by [`TcpServer::metrics()`]. All counters are monotonically
/// increasing except `active_connections` which reflects the current gauge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMetrics {
    /// Current number of active (in-flight) connections.
    pub active_connections: u64,
    /// Total connections accepted since server start.
    pub total_accepted: u64,
    /// Total connections rejected due to connection limit.
    pub total_rejected: u64,
    /// Total requests that timed out.
    pub total_timed_out: u64,
    /// Total requests served since server start.
    pub total_requests: u64,
    /// Total bytes read from clients.
    pub bytes_in: u64,
    /// Total bytes written to clients.
    pub bytes_out: u64,
}

/// Atomic counters backing [`ServerMetrics`].
///
/// These live inside `TcpServer` and are updated as connections are
/// accepted, rejected, or timed out.
#[derive(Debug)]
struct MetricsCounters {
    total_accepted: AtomicU64,
    total_rejected: AtomicU64,
    total_timed_out: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

impl MetricsCounters {
    fn new() -> Self {
        Self {
            total_accepted: AtomicU64::new(0),
            total_rejected: AtomicU64::new(0),
            total_timed_out: AtomicU64::new(0),
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
        }
    }
}

impl Default for TcpServer {
    fn default() -> Self {
        Self::new(ServerConfig::default())
    }
}

/// Returns true if the accept error is fatal (should stop the server).
fn is_fatal_accept_error(e: &io::Error) -> bool {
    // These errors indicate the listener itself is broken.
    matches!(
        e.kind(),
        io::ErrorKind::NotConnected | io::ErrorKind::InvalidInput
    )
}

/// Reads data from a TCP stream into a buffer.
///
/// Returns the number of bytes read, or 0 if the connection was closed.
async fn read_into_buffer(stream: &mut TcpStream, buffer: &mut [u8]) -> io::Result<usize> {
    use std::future::poll_fn;

    poll_fn(|cx| {
        let mut read_buf = ReadBuf::new(buffer);
        match Pin::new(&mut *stream).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.filled().len())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// Reads data from a TCP stream with a timeout.
///
/// Uses asupersync's timer system for proper async timeout handling.
/// The timeout is implemented using asupersync's `timeout` future wrapper,
/// which properly integrates with the async runtime's timer driver.
///
/// # Arguments
///
/// * `stream` - The TCP stream to read from
/// * `buffer` - The buffer to read into
/// * `timeout_duration` - Maximum time to wait for data
///
/// # Returns
///
/// * `Ok(n)` - Number of bytes read (0 means connection closed)
/// * `Err(TimedOut)` - Timeout expired with no data
/// * `Err(other)` - IO error from the underlying stream
async fn read_with_timeout(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    timeout_duration: Duration,
) -> io::Result<usize> {
    // Get current time for the timeout calculation
    let now = current_time();

    // Create the read future - we need to box it for Unpin
    let read_future = Box::pin(read_into_buffer(stream, buffer));

    // Wrap with asupersync timeout
    match timeout(now, timeout_duration, read_future).await {
        Ok(result) => result,
        Err(_elapsed) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "keep-alive timeout expired",
        )),
    }
}

/// Writes raw bytes to a TCP stream (e.g., for 100 Continue response).
///
/// This writes the bytes directly without any HTTP formatting.
async fn write_raw_response(stream: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
    use std::future::poll_fn;
    write_all(stream, bytes).await?;
    poll_fn(|cx| Pin::new(&mut *stream).poll_flush(cx)).await?;
    Ok(())
}

/// Writes a response to a TCP stream.
///
/// Handles both full (buffered) and streaming (chunked) responses.
async fn write_response(stream: &mut TcpStream, response: ResponseWrite) -> io::Result<()> {
    use std::future::poll_fn;

    match response {
        ResponseWrite::Full(bytes) => {
            write_all(stream, &bytes).await?;
        }
        ResponseWrite::Stream(mut encoder) => {
            // Write chunks as they become available
            loop {
                let chunk = poll_fn(|cx| Pin::new(&mut encoder).poll_next(cx)).await;
                match chunk {
                    Some(bytes) => {
                        write_all(stream, &bytes).await?;
                    }
                    None => break,
                }
            }
        }
    }

    // Flush the stream
    poll_fn(|cx| Pin::new(&mut *stream).poll_flush(cx)).await?;

    Ok(())
}

/// Writes all bytes to a stream.
async fn write_all(stream: &mut TcpStream, mut buf: &[u8]) -> io::Result<()> {
    use std::future::poll_fn;

    while !buf.is_empty() {
        let n = poll_fn(|cx| Pin::new(&mut *stream).poll_write(cx, buf)).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to write whole buffer",
            ));
        }
        buf = &buf[n..];
    }
    Ok(())
}

// Connection header handling moved to crate::connection module

// ============================================================================
// Synchronous Server (for compatibility)
// ============================================================================

/// Synchronous HTTP server for request/response conversion.
///
/// This is a simpler, non-async server that just provides parsing and
/// serialization utilities. It's useful for testing or when you don't
/// need full async TCP handling.
pub struct Server {
    parser: Parser,
}

impl Server {
    /// Create a new server.
    #[must_use]
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
        }
    }

    /// Parse a request from bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the request is malformed.
    pub fn parse_request(&self, bytes: &[u8]) -> Result<Request, ParseError> {
        self.parser.parse(bytes)
    }

    /// Write a response to bytes.
    #[must_use]
    pub fn write_response(&self, response: Response) -> ResponseWrite {
        let mut writer = ResponseWriter::new();
        writer.write(response)
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_config_builder() {
        let config = ServerConfig::new("0.0.0.0:3000")
            .with_request_timeout_secs(60)
            .with_max_connections(1000)
            .with_tcp_nodelay(false)
            .with_allowed_hosts(["example.com", "api.example.com"])
            .with_trust_x_forwarded_host(true);

        assert_eq!(config.bind_addr, "0.0.0.0:3000");
        assert_eq!(config.request_timeout, Time::from_secs(60));
        assert_eq!(config.max_connections, 1000);
        assert!(!config.tcp_nodelay);
        assert_eq!(config.allowed_hosts.len(), 2);
        assert!(config.trust_x_forwarded_host);
    }

    #[test]
    fn server_config_defaults() {
        let config = ServerConfig::default();
        assert_eq!(config.bind_addr, "127.0.0.1:8080");
        assert_eq!(
            config.request_timeout,
            Time::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS)
        );
        assert_eq!(config.max_connections, DEFAULT_MAX_CONNECTIONS);
        assert!(config.tcp_nodelay);
        assert!(config.allowed_hosts.is_empty());
        assert!(!config.trust_x_forwarded_host);
    }

    #[test]
    fn tcp_server_creates_request_ids() {
        let server = TcpServer::default();
        let id1 = server.next_request_id();
        let id2 = server.next_request_id();
        let id3 = server.next_request_id();

        assert_eq!(id1, 0);
        assert_eq!(id2, 1);
        assert_eq!(id3, 2);
    }

    #[test]
    fn server_error_display() {
        let io_err = ServerError::Io(io::Error::new(io::ErrorKind::AddrInUse, "address in use"));
        assert!(io_err.to_string().contains("IO error"));

        let shutdown_err = ServerError::Shutdown;
        assert_eq!(shutdown_err.to_string(), "Server shutdown");

        let limit_err = ServerError::ConnectionLimitReached;
        assert_eq!(limit_err.to_string(), "Connection limit reached");
    }

    #[test]
    fn sync_server_parses_request() {
        let server = Server::new();
        let request = b"GET /hello HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let result = server.parse_request(request);
        assert!(result.is_ok());
    }

    // ========================================================================
    // Host header validation tests
    // ========================================================================

    #[test]
    fn host_validation_missing_host_rejected() {
        let config = ServerConfig::default();
        let request = Request::new(fastapi_core::Method::Get, "/");
        let err = validate_host_header(&request, &config).unwrap_err();
        assert_eq!(err.kind, HostValidationErrorKind::Missing);
        assert_eq!(err.response().status().as_u16(), 400);
    }

    #[test]
    fn host_validation_allows_configured_host() {
        let config = ServerConfig::default().with_allowed_hosts(["example.com"]);
        let mut request = Request::new(fastapi_core::Method::Get, "/");
        request
            .headers_mut()
            .insert("Host".to_string(), b"example.com".to_vec());
        assert!(validate_host_header(&request, &config).is_ok());
    }

    #[test]
    fn host_validation_rejects_disallowed_host() {
        let config = ServerConfig::default().with_allowed_hosts(["example.com"]);
        let mut request = Request::new(fastapi_core::Method::Get, "/");
        request
            .headers_mut()
            .insert("Host".to_string(), b"evil.com".to_vec());
        let err = validate_host_header(&request, &config).unwrap_err();
        assert_eq!(err.kind, HostValidationErrorKind::NotAllowed);
    }

    #[test]
    fn host_validation_wildcard_allows_subdomains_only() {
        let config = ServerConfig::default().with_allowed_hosts(["*.example.com"]);
        let mut request = Request::new(fastapi_core::Method::Get, "/");
        request
            .headers_mut()
            .insert("Host".to_string(), b"api.example.com".to_vec());
        assert!(validate_host_header(&request, &config).is_ok());

        let mut request = Request::new(fastapi_core::Method::Get, "/");
        request
            .headers_mut()
            .insert("Host".to_string(), b"example.com".to_vec());
        let err = validate_host_header(&request, &config).unwrap_err();
        assert_eq!(err.kind, HostValidationErrorKind::NotAllowed);
    }

    #[test]
    fn host_validation_uses_x_forwarded_host_when_trusted() {
        let config = ServerConfig::default()
            .with_allowed_hosts(["example.com"])
            .with_trust_x_forwarded_host(true);
        let mut request = Request::new(fastapi_core::Method::Get, "/");
        request
            .headers_mut()
            .insert("Host".to_string(), b"internal.local".to_vec());
        request
            .headers_mut()
            .insert("X-Forwarded-Host".to_string(), b"example.com".to_vec());
        assert!(validate_host_header(&request, &config).is_ok());
    }

    #[test]
    fn host_validation_rejects_invalid_host_value() {
        let config = ServerConfig::default();
        let mut request = Request::new(fastapi_core::Method::Get, "/");
        request
            .headers_mut()
            .insert("Host".to_string(), b"bad host".to_vec());
        let err = validate_host_header(&request, &config).unwrap_err();
        assert_eq!(err.kind, HostValidationErrorKind::Invalid);
    }

    // ========================================================================
    // Keep-alive detection tests
    // ========================================================================

    #[test]
    fn keep_alive_default_http11() {
        // HTTP/1.1 defaults to keep-alive
        let mut request = Request::new(fastapi_core::Method::Get, "/path".to_string());
        request
            .headers_mut()
            .insert("Host".to_string(), b"example.com".to_vec());
        assert!(should_keep_alive(&request));
    }

    #[test]
    fn keep_alive_explicit_keep_alive() {
        let mut request = Request::new(fastapi_core::Method::Get, "/path".to_string());
        request
            .headers_mut()
            .insert("Connection".to_string(), b"keep-alive".to_vec());
        assert!(should_keep_alive(&request));
    }

    #[test]
    fn keep_alive_connection_close() {
        let mut request = Request::new(fastapi_core::Method::Get, "/path".to_string());
        request
            .headers_mut()
            .insert("Connection".to_string(), b"close".to_vec());
        assert!(!should_keep_alive(&request));
    }

    #[test]
    fn keep_alive_connection_close_case_insensitive() {
        let mut request = Request::new(fastapi_core::Method::Get, "/path".to_string());
        request
            .headers_mut()
            .insert("Connection".to_string(), b"CLOSE".to_vec());
        assert!(!should_keep_alive(&request));
    }

    #[test]
    fn keep_alive_multiple_values() {
        let mut request = Request::new(fastapi_core::Method::Get, "/path".to_string());
        request
            .headers_mut()
            .insert("Connection".to_string(), b"keep-alive, upgrade".to_vec());
        assert!(should_keep_alive(&request));
    }

    // ========================================================================
    // Timeout behavior tests
    // ========================================================================

    #[test]
    fn timeout_budget_created_with_config_deadline() {
        let config = ServerConfig::new("127.0.0.1:8080").with_request_timeout_secs(45);
        let budget = Budget::new().with_deadline(config.request_timeout);
        assert_eq!(budget.deadline, Some(Time::from_secs(45)));
    }

    #[test]
    fn timeout_duration_conversion_from_time() {
        let timeout = Time::from_secs(30);
        let duration = Duration::from_nanos(timeout.as_nanos());
        assert_eq!(duration, Duration::from_secs(30));
    }

    #[test]
    fn timeout_duration_conversion_from_time_millis() {
        let timeout = Time::from_millis(1500);
        let duration = Duration::from_nanos(timeout.as_nanos());
        assert_eq!(duration, Duration::from_millis(1500));
    }

    #[test]
    fn gateway_timeout_response_has_correct_status() {
        let response = Response::with_status(StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(response.status().as_u16(), 504);
    }

    #[test]
    fn gateway_timeout_response_with_body() {
        let response = Response::with_status(StatusCode::GATEWAY_TIMEOUT).body(
            fastapi_core::ResponseBody::Bytes(b"Request timed out".to_vec()),
        );
        assert_eq!(response.status().as_u16(), 504);
        // Verify body is set (not empty)
        assert!(response.body_ref().len() > 0);
    }

    #[test]
    fn elapsed_time_check_logic() {
        // Test the timeout check logic in isolation
        let start = Instant::now();
        let timeout_duration = Duration::from_millis(10);

        // Immediately after start, should not be timed out
        assert!(start.elapsed() <= timeout_duration);

        // Wait a bit longer than the timeout
        std::thread::sleep(Duration::from_millis(20));

        // Now should be timed out
        assert!(start.elapsed() > timeout_duration);
    }

    // ========================================================================
    // Connection limit tests
    // ========================================================================

    #[test]
    fn connection_counter_starts_at_zero() {
        let server = TcpServer::default();
        assert_eq!(server.current_connections(), 0);
    }

    #[test]
    fn try_acquire_connection_unlimited() {
        // With max_connections = 0 (unlimited), should always succeed
        let server = TcpServer::default();
        assert_eq!(server.config().max_connections, 0);

        // Acquire several connections
        for _ in 0..100 {
            assert!(server.try_acquire_connection());
        }
        assert_eq!(server.current_connections(), 100);

        // Release them all
        for _ in 0..100 {
            server.release_connection();
        }
        assert_eq!(server.current_connections(), 0);
    }

    #[test]
    fn try_acquire_connection_with_limit() {
        let config = ServerConfig::new("127.0.0.1:8080").with_max_connections(5);
        let server = TcpServer::new(config);

        // Acquire up to the limit
        for i in 0..5 {
            assert!(
                server.try_acquire_connection(),
                "Should acquire connection {i}"
            );
        }
        assert_eq!(server.current_connections(), 5);

        // Next one should fail
        assert!(!server.try_acquire_connection());
        assert_eq!(server.current_connections(), 5);

        // Release one
        server.release_connection();
        assert_eq!(server.current_connections(), 4);

        // Now we can acquire one more
        assert!(server.try_acquire_connection());
        assert_eq!(server.current_connections(), 5);
    }

    #[test]
    fn try_acquire_connection_single_connection_limit() {
        let config = ServerConfig::new("127.0.0.1:8080").with_max_connections(1);
        let server = TcpServer::new(config);

        // First acquire succeeds
        assert!(server.try_acquire_connection());
        assert_eq!(server.current_connections(), 1);

        // Second fails
        assert!(!server.try_acquire_connection());
        assert_eq!(server.current_connections(), 1);

        // After release, can acquire again
        server.release_connection();
        assert!(server.try_acquire_connection());
    }

    #[test]
    fn service_unavailable_response_has_correct_status() {
        let response = Response::with_status(StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.status().as_u16(), 503);
    }

    #[test]
    fn service_unavailable_response_with_body() {
        let response = Response::with_status(StatusCode::SERVICE_UNAVAILABLE)
            .header("connection", b"close".to_vec())
            .body(fastapi_core::ResponseBody::Bytes(
                b"503 Service Unavailable: connection limit reached".to_vec(),
            ));
        assert_eq!(response.status().as_u16(), 503);
        assert!(response.body_ref().len() > 0);
    }

    #[test]
    fn config_max_connections_default_is_zero() {
        let config = ServerConfig::default();
        assert_eq!(config.max_connections, 0);
    }

    #[test]
    fn config_max_connections_can_be_set() {
        let config = ServerConfig::new("127.0.0.1:8080").with_max_connections(100);
        assert_eq!(config.max_connections, 100);
    }

    // ========================================================================
    // Keep-alive configuration tests
    // ========================================================================

    #[test]
    fn config_keep_alive_timeout_default() {
        let config = ServerConfig::default();
        assert_eq!(
            config.keep_alive_timeout,
            Duration::from_secs(DEFAULT_KEEP_ALIVE_TIMEOUT_SECS)
        );
    }

    #[test]
    fn config_keep_alive_timeout_can_be_set() {
        let config =
            ServerConfig::new("127.0.0.1:8080").with_keep_alive_timeout(Duration::from_secs(120));
        assert_eq!(config.keep_alive_timeout, Duration::from_secs(120));
    }

    #[test]
    fn config_keep_alive_timeout_can_be_set_secs() {
        let config = ServerConfig::new("127.0.0.1:8080").with_keep_alive_timeout_secs(90);
        assert_eq!(config.keep_alive_timeout, Duration::from_secs(90));
    }

    #[test]
    fn config_max_requests_per_connection_default() {
        let config = ServerConfig::default();
        assert_eq!(
            config.max_requests_per_connection,
            DEFAULT_MAX_REQUESTS_PER_CONNECTION
        );
    }

    #[test]
    fn config_max_requests_per_connection_can_be_set() {
        let config = ServerConfig::new("127.0.0.1:8080").with_max_requests_per_connection(50);
        assert_eq!(config.max_requests_per_connection, 50);
    }

    #[test]
    fn config_max_requests_per_connection_unlimited() {
        let config = ServerConfig::new("127.0.0.1:8080").with_max_requests_per_connection(0);
        assert_eq!(config.max_requests_per_connection, 0);
    }

    #[test]
    fn response_with_keep_alive_header() {
        let response = Response::ok().header("connection", b"keep-alive".to_vec());
        let headers = response.headers();
        let connection_header = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("connection"));
        assert!(connection_header.is_some());
        assert_eq!(connection_header.unwrap().1, b"keep-alive");
    }

    #[test]
    fn response_with_close_header() {
        let response = Response::ok().header("connection", b"close".to_vec());
        let headers = response.headers();
        let connection_header = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("connection"));
        assert!(connection_header.is_some());
        assert_eq!(connection_header.unwrap().1, b"close");
    }

    // ========================================================================
    // Connection draining tests
    // ========================================================================

    #[test]
    fn config_drain_timeout_default() {
        let config = ServerConfig::default();
        assert_eq!(
            config.drain_timeout,
            Duration::from_secs(DEFAULT_DRAIN_TIMEOUT_SECS)
        );
    }

    #[test]
    fn config_drain_timeout_can_be_set() {
        let config =
            ServerConfig::new("127.0.0.1:8080").with_drain_timeout(Duration::from_secs(60));
        assert_eq!(config.drain_timeout, Duration::from_secs(60));
    }

    #[test]
    fn config_drain_timeout_can_be_set_secs() {
        let config = ServerConfig::new("127.0.0.1:8080").with_drain_timeout_secs(45);
        assert_eq!(config.drain_timeout, Duration::from_secs(45));
    }

    #[test]
    fn server_not_draining_initially() {
        let server = TcpServer::default();
        assert!(!server.is_draining());
    }

    #[test]
    fn server_start_drain_sets_flag() {
        let server = TcpServer::default();
        assert!(!server.is_draining());
        server.start_drain();
        assert!(server.is_draining());
    }

    #[test]
    fn server_start_drain_idempotent() {
        let server = TcpServer::default();
        server.start_drain();
        assert!(server.is_draining());
        server.start_drain();
        assert!(server.is_draining());
    }

    #[tokio::test]
    async fn wait_for_drain_returns_true_when_no_connections() {
        let server = TcpServer::default();
        assert_eq!(server.current_connections(), 0);
        let result = server
            .wait_for_drain(Duration::from_millis(100), Some(Duration::from_millis(1)))
            .await;
        assert!(result);
    }

    #[tokio::test]
    async fn wait_for_drain_timeout_with_connections() {
        let server = TcpServer::default();
        // Simulate active connections
        server.try_acquire_connection();
        server.try_acquire_connection();
        assert_eq!(server.current_connections(), 2);

        // Wait should timeout since connections won't drain on their own
        let result = server
            .wait_for_drain(Duration::from_millis(50), Some(Duration::from_millis(5)))
            .await;
        assert!(!result);
        assert_eq!(server.current_connections(), 2);
    }

    #[tokio::test]
    async fn drain_returns_zero_when_no_connections() {
        let server = TcpServer::new(
            ServerConfig::new("127.0.0.1:8080").with_drain_timeout(Duration::from_millis(100)),
        );
        assert_eq!(server.current_connections(), 0);
        let remaining = server.drain().await;
        assert_eq!(remaining, 0);
        assert!(server.is_draining());
    }

    #[tokio::test]
    async fn drain_returns_count_when_connections_remain() {
        let server = TcpServer::new(
            ServerConfig::new("127.0.0.1:8080").with_drain_timeout(Duration::from_millis(50)),
        );
        // Simulate active connections that won't drain
        server.try_acquire_connection();
        server.try_acquire_connection();
        server.try_acquire_connection();

        let remaining = server.drain().await;
        assert_eq!(remaining, 3);
        assert!(server.is_draining());
    }

    #[test]
    fn server_shutdown_error_display() {
        let err = ServerError::Shutdown;
        assert_eq!(err.to_string(), "Server shutdown");
    }

    // ========================================================================
    // Graceful shutdown controller tests
    // ========================================================================

    #[test]
    fn server_has_shutdown_controller() {
        let server = TcpServer::default();
        let controller = server.shutdown_controller();
        assert!(!controller.is_shutting_down());
    }

    #[test]
    fn server_subscribe_shutdown_returns_receiver() {
        let server = TcpServer::default();
        let receiver = server.subscribe_shutdown();
        assert!(!receiver.is_shutting_down());
    }

    #[test]
    fn server_shutdown_sets_draining_and_controller() {
        let server = TcpServer::default();
        assert!(!server.is_shutting_down());
        assert!(!server.is_draining());
        assert!(!server.shutdown_controller().is_shutting_down());

        server.shutdown();

        assert!(server.is_shutting_down());
        assert!(server.is_draining());
        assert!(server.shutdown_controller().is_shutting_down());
    }

    #[test]
    fn server_shutdown_notifies_receivers() {
        let server = TcpServer::default();
        let receiver1 = server.subscribe_shutdown();
        let receiver2 = server.subscribe_shutdown();

        assert!(!receiver1.is_shutting_down());
        assert!(!receiver2.is_shutting_down());

        server.shutdown();

        assert!(receiver1.is_shutting_down());
        assert!(receiver2.is_shutting_down());
    }

    #[test]
    fn server_shutdown_is_idempotent() {
        let server = TcpServer::default();
        let receiver = server.subscribe_shutdown();

        server.shutdown();
        server.shutdown();
        server.shutdown();

        assert!(server.is_shutting_down());
        assert!(receiver.is_shutting_down());
    }

    // ========================================================================
    // Keep-alive timeout tests
    // ========================================================================

    #[test]
    fn keep_alive_timeout_error_display() {
        let err = ServerError::KeepAliveTimeout;
        assert_eq!(err.to_string(), "Keep-alive timeout");
    }

    #[test]
    fn keep_alive_timeout_zero_disables_timeout() {
        let config = ServerConfig::new("127.0.0.1:8080").with_keep_alive_timeout(Duration::ZERO);
        assert!(config.keep_alive_timeout.is_zero());
    }

    #[test]
    fn keep_alive_timeout_default_is_non_zero() {
        let config = ServerConfig::default();
        assert!(!config.keep_alive_timeout.is_zero());
        assert_eq!(
            config.keep_alive_timeout,
            Duration::from_secs(DEFAULT_KEEP_ALIVE_TIMEOUT_SECS)
        );
    }

    #[test]
    fn timed_out_io_error_kind() {
        let err = io::Error::new(io::ErrorKind::TimedOut, "test timeout");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn instant_deadline_calculation() {
        let timeout = Duration::from_millis(100);
        let deadline = Instant::now() + timeout;

        // Deadline should be in the future
        assert!(deadline > Instant::now());

        // After waiting, deadline should be in the past
        std::thread::sleep(Duration::from_millis(150));
        assert!(Instant::now() >= deadline);
    }

    #[test]
    fn server_metrics_initial_state() {
        let server = TcpServer::default();
        let m = server.metrics();
        assert_eq!(m.active_connections, 0);
        assert_eq!(m.total_accepted, 0);
        assert_eq!(m.total_rejected, 0);
        assert_eq!(m.total_timed_out, 0);
        assert_eq!(m.total_requests, 0);
        assert_eq!(m.bytes_in, 0);
        assert_eq!(m.bytes_out, 0);
    }

    #[test]
    fn server_metrics_after_acquire_release() {
        let server = TcpServer::new(ServerConfig::new("127.0.0.1:0").with_max_connections(10));
        assert!(server.try_acquire_connection());
        assert!(server.try_acquire_connection());

        let m = server.metrics();
        assert_eq!(m.active_connections, 2);
        assert_eq!(m.total_accepted, 2);
        assert_eq!(m.total_rejected, 0);

        server.release_connection();
        let m = server.metrics();
        assert_eq!(m.active_connections, 1);
        assert_eq!(m.total_accepted, 2); // monotonic
    }

    #[test]
    fn server_metrics_rejection_counted() {
        let server = TcpServer::new(ServerConfig::new("127.0.0.1:0").with_max_connections(1));
        assert!(server.try_acquire_connection());
        assert!(!server.try_acquire_connection()); // rejected

        let m = server.metrics();
        assert_eq!(m.total_accepted, 1);
        assert_eq!(m.total_rejected, 1);
        assert_eq!(m.active_connections, 1);
    }

    #[test]
    fn server_metrics_bytes_tracking() {
        let server = TcpServer::default();
        server.record_bytes_in(1024);
        server.record_bytes_in(512);
        server.record_bytes_out(2048);

        let m = server.metrics();
        assert_eq!(m.bytes_in, 1536);
        assert_eq!(m.bytes_out, 2048);
    }

    #[test]
    fn server_metrics_unlimited_connections_accepted() {
        let server = TcpServer::new(ServerConfig::new("127.0.0.1:0").with_max_connections(0));
        for _ in 0..100 {
            assert!(server.try_acquire_connection());
        }
        let m = server.metrics();
        assert_eq!(m.total_accepted, 100);
        assert_eq!(m.total_rejected, 0);
        assert_eq!(m.active_connections, 100);
    }

    #[test]
    fn server_metrics_clone_eq() {
        let server = TcpServer::default();
        server.record_bytes_in(42);
        let m1 = server.metrics();
        let m2 = m1.clone();
        assert_eq!(m1, m2);
    }
}

// ============================================================================
// App Serve Extension
// ============================================================================

/// Extension trait to add serve capability to [`App`].
///
/// This trait provides the `serve()` method that wires an App to the HTTP server,
/// enabling it to handle incoming HTTP requests.
///
/// # Example
///
/// ```ignore
/// use fastapi::prelude::*;
/// use fastapi_http::AppServeExt;
///
/// let app = App::builder()
///     .get("/", |_, _| async { Response::ok().body_text("Hello!") })
///     .build();
///
/// // Run the server
/// app.serve("0.0.0.0:8080").await?;
/// ```
pub trait AppServeExt {
    /// Starts the HTTP server and begins accepting connections.
    ///
    /// This method:
    /// 1. Runs all registered startup hooks
    /// 2. Binds to the specified address
    /// 3. Accepts connections and routes requests to handlers
    /// 4. Runs shutdown hooks when the server stops
    ///
    /// # Arguments
    ///
    /// * `addr` - The address to bind to (e.g., "0.0.0.0:8080" or "127.0.0.1:3000")
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A startup hook fails with `abort: true`
    /// - Binding to the address fails
    /// - An unrecoverable IO error occurs
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi::prelude::*;
    /// use fastapi_http::AppServeExt;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let app = App::builder()
    ///         .get("/health", |_, _| async { Response::ok() })
    ///         .build();
    ///
    ///     app.serve("0.0.0.0:8080").await?;
    ///     Ok(())
    /// }
    /// ```
    fn serve(self, addr: impl Into<String>) -> impl Future<Output = Result<(), ServeError>> + Send;

    /// Starts the HTTP server with custom configuration.
    ///
    /// This method allows fine-grained control over server behavior including
    /// timeouts, connection limits, and keep-alive settings.
    ///
    /// # Arguments
    ///
    /// * `config` - Server configuration options
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi::prelude::*;
    /// use fastapi_http::{AppServeExt, ServerConfig};
    ///
    /// let config = ServerConfig::new("0.0.0.0:8080")
    ///     .with_request_timeout_secs(60)
    ///     .with_max_connections(1000)
    ///     .with_keep_alive_timeout_secs(120);
    ///
    /// app.serve_with_config(config).await?;
    /// ```
    fn serve_with_config(
        self,
        config: ServerConfig,
    ) -> impl Future<Output = Result<(), ServeError>> + Send;
}

/// Error returned when starting or running the server fails.
#[derive(Debug)]
pub enum ServeError {
    /// A startup hook failed with abort.
    Startup(fastapi_core::StartupHookError),
    /// Server error during operation.
    Server(ServerError),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Startup(e) => write!(f, "startup hook failed: {}", e.message),
            Self::Server(e) => write!(f, "server error: {e}"),
        }
    }
}

impl std::error::Error for ServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Startup(_) => None,
            Self::Server(e) => Some(e),
        }
    }
}

impl From<ServerError> for ServeError {
    fn from(e: ServerError) -> Self {
        Self::Server(e)
    }
}

impl AppServeExt for App {
    fn serve(self, addr: impl Into<String>) -> impl Future<Output = Result<(), ServeError>> + Send {
        let config = ServerConfig::new(addr);
        self.serve_with_config(config)
    }

    #[allow(clippy::manual_async_fn)] // Using impl Future for trait compatibility
    fn serve_with_config(
        self,
        config: ServerConfig,
    ) -> impl Future<Output = Result<(), ServeError>> + Send {
        async move {
            // Run startup hooks
            match self.run_startup_hooks().await {
                fastapi_core::StartupOutcome::Success => {}
                fastapi_core::StartupOutcome::PartialSuccess { warnings } => {
                    // Log warnings but continue (non-fatal)
                    eprintln!("Warning: {warnings} startup hook(s) had non-fatal errors");
                }
                fastapi_core::StartupOutcome::Aborted(e) => {
                    return Err(ServeError::Startup(e));
                }
            }

            // Create the TCP server
            let server = TcpServer::new(config);

            // Wrap app in Arc for sharing with handler
            // App implements Handler trait, so we can use serve_handler
            let app = Arc::new(self);
            let handler: Arc<dyn fastapi_core::Handler> =
                Arc::clone(&app) as Arc<dyn fastapi_core::Handler>;

            // Create a root Cx for the server
            let cx = Cx::for_testing();

            // Print startup banner
            let bind_addr = &server.config().bind_addr;
            println!("🚀 Server starting on http://{bind_addr}");

            // Run the server using the Handler-based serve method
            let result = server.serve_handler(&cx, handler).await;

            // Run shutdown hooks (use the original Arc<App>)
            app.run_shutdown_hooks().await;

            result.map_err(ServeError::from)
        }
    }
}

/// Convenience function to serve an App on the given address.
///
/// This is equivalent to calling `app.serve(addr)` but can be more
/// ergonomic in some contexts.
///
/// # Example
///
/// ```ignore
/// use fastapi::prelude::*;
/// use fastapi_http::serve;
///
/// let app = App::builder()
///     .get("/", |_, _| async { Response::ok() })
///     .build();
///
/// serve(app, "0.0.0.0:8080").await?;
/// ```
pub async fn serve(app: App, addr: impl Into<String>) -> Result<(), ServeError> {
    app.serve(addr).await
}

/// Convenience function to serve an App with custom configuration.
///
/// # Example
///
/// ```ignore
/// use fastapi::prelude::*;
/// use fastapi_http::{serve_with_config, ServerConfig};
///
/// let app = App::builder()
///     .get("/", |_, _| async { Response::ok() })
///     .build();
///
/// let config = ServerConfig::new("0.0.0.0:8080")
///     .with_max_connections(500);
///
/// serve_with_config(app, config).await?;
/// ```
pub async fn serve_with_config(app: App, config: ServerConfig) -> Result<(), ServeError> {
    app.serve_with_config(config).await
}
