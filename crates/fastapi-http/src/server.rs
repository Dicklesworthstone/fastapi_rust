//! HTTP server with asupersync integration.
//!
//! This module provides a TCP server that uses asupersync for structured
//! concurrency and cancel-correct request handling.
//!
// NOTE: This server implementation is used by `serve`/`serve_with_config` and is
// intentionally asupersync-only (no tokio). Some ancillary types are still
// evolving as the runtime's I/O surface matures.
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
use crate::expect::{
    CONTINUE_RESPONSE, ExpectHandler, ExpectResult, PreBodyValidator, PreBodyValidators,
};
use crate::http2;
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
use fastapi_core::{Method, Request, RequestContext, Response, StatusCode};
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

/// Server configuration for the HTTP/1.1 server.
///
/// Controls bind address, timeouts, connection limits, and HTTP parsing behavior.
/// All timeouts use sensible defaults suitable for production use.
///
/// # Defaults
///
/// | Setting | Default |
/// |---------|---------|
/// | `request_timeout` | 30s |
/// | `max_connections` | 0 (unlimited) |
/// | `read_buffer_size` | 8192 bytes |
/// | `tcp_nodelay` | `true` |
/// | `keep_alive_timeout` | 75s |
/// | `max_requests_per_connection` | 100 |
/// | `drain_timeout` | 30s |
///
/// # Example
///
/// ```ignore
/// use fastapi_http::{ServerConfig, serve_with_config};
///
/// let config = ServerConfig::new("0.0.0.0:8000")
///     .with_request_timeout_secs(60)
///     .with_max_connections(1000)
///     .with_keep_alive_timeout_secs(120);
/// ```
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
    /// Pre-body validation hooks (run after parsing headers but before any body is read).
    ///
    /// This is used to gate `Expect: 100-continue` and to reject requests early based on
    /// headers alone (auth/content-type/content-length/etc).
    pub pre_body_validators: PreBodyValidators,
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
            pre_body_validators: PreBodyValidators::new(),
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
    /// Patterns are normalized to lowercase for case-insensitive matching.
    #[must_use]
    pub fn with_allowed_hosts<I, S>(mut self, hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        // Pre-lowercase patterns to avoid allocation during matching
        self.allowed_hosts = hosts
            .into_iter()
            .map(|s| s.into().to_ascii_lowercase())
            .collect();
        self
    }

    /// Adds a single allowed host.
    ///
    /// The pattern is normalized to lowercase for case-insensitive matching.
    #[must_use]
    pub fn allow_host(mut self, host: impl Into<String>) -> Self {
        // Pre-lowercase pattern to avoid allocation during matching
        self.allowed_hosts.push(host.into().to_ascii_lowercase());
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

    /// Replace all configured pre-body validators.
    #[must_use]
    pub fn with_pre_body_validators(mut self, validators: PreBodyValidators) -> Self {
        self.pre_body_validators = validators;
        self
    }

    /// Add a pre-body validator.
    #[must_use]
    pub fn with_pre_body_validator<V: PreBodyValidator + 'static>(mut self, validator: V) -> Self {
        self.pre_body_validators.add(validator);
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
    /// HTTP/2 error.
    Http2(http2::Http2Error),
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
            Self::Http2(e) => write!(f, "HTTP/2 error: {e}"),
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
            Self::Http2(e) => Some(e),
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
    // Note: str::len() returns byte length (RFC 1035 specifies 253 octets)
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
    // Note: patterns are pre-lowercased at config time, so no allocation needed here
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // suffix is already lowercase (pre-processed at config time)
        if host.host == suffix {
            return false;
        }
        return host.host.len() > suffix.len() + 1
            && host.host.ends_with(suffix)
            && host.host.as_bytes()[host.host.len() - suffix.len() - 1] == b'.';
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

fn header_str<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
    req.headers()
        .get(name)
        .and_then(|v| std::str::from_utf8(v).ok())
        .map(str::trim)
}

fn header_has_token(req: &Request, name: &str, token: &str) -> bool {
    let Some(v) = header_str(req, name) else {
        return false;
    };
    v.split(',')
        .map(str::trim)
        .any(|t| t.eq_ignore_ascii_case(token))
}

fn connection_has_token(req: &Request, token: &str) -> bool {
    header_has_token(req, "connection", token)
}

fn is_websocket_upgrade_request(req: &Request) -> bool {
    if req.method() != Method::Get {
        return false;
    }
    if !header_has_token(req, "upgrade", "websocket") {
        return false;
    }
    connection_has_token(req, "upgrade")
}

fn has_request_body_headers(req: &Request) -> bool {
    if req.headers().contains("transfer-encoding") {
        return true;
    }
    if let Some(v) = header_str(req, "content-length") {
        if v.is_empty() {
            return true;
        }
        match v.parse::<usize>() {
            Ok(0) => false,
            Ok(_) => true,
            Err(_) => true,
        }
    } else {
        false
    }
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

impl From<http2::Http2Error> for ServerError {
    fn from(e: http2::Http2Error) -> Self {
        Self::Http2(e)
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
    let (proto, buffered) = sniff_protocol(&mut stream, config.keep_alive_timeout).await?;
    if proto == SniffedProtocol::Http2PriorKnowledge {
        return process_connection_http2(cx, request_counter, stream, config, handler).await;
    }

    let mut parser = StatefulParser::new().with_limits(config.parse_limits.clone());
    if !buffered.is_empty() {
        parser.feed(&buffered)?;
    }
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

        // Run header-only validators before honoring Expect: 100-continue or reading any body bytes.
        if let Err(response) = config.pre_body_validators.validate_all(&request) {
            let response = response.header("connection", b"close".to_vec());
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

async fn process_connection_http2<H, Fut>(
    cx: &Cx,
    request_counter: &AtomicU64,
    stream: TcpStream,
    config: &ServerConfig,
    handler: H,
) -> Result<(), ServerError>
where
    H: Fn(RequestContext, &mut Request) -> Fut,
    Fut: Future<Output = Response>,
{
    const FLAG_END_HEADERS: u8 = 0x4;
    const FLAG_ACK: u8 = 0x1;

    let mut framed = http2::FramedH2::new(stream, Vec::new());
    let mut hpack = http2::HpackDecoder::new();
    let recv_max_frame_size: u32 = 16 * 1024;
    let mut peer_max_frame_size: u32 = 16 * 1024;
    let mut flow_control = http2::H2FlowControl::new();

    let first = framed.read_frame(recv_max_frame_size).await?;
    if first.header.frame_type() != http2::FrameType::Settings
        || first.header.stream_id != 0
        || (first.header.flags & FLAG_ACK) != 0
    {
        return Err(http2::Http2Error::Protocol("expected client SETTINGS after preface").into());
    }
    apply_http2_settings_with_fc(
        &mut hpack,
        &mut peer_max_frame_size,
        Some(&mut flow_control),
        &first.payload,
    )?;

    framed
        .write_frame(http2::FrameType::Settings, 0, 0, &[])
        .await?;
    framed
        .write_frame(http2::FrameType::Settings, FLAG_ACK, 0, &[])
        .await?;

    let default_body_limit = config.parse_limits.max_request_size;
    let mut last_stream_id: u32 = 0;

    loop {
        if cx.is_cancel_requested() {
            let _ = send_goaway(&mut framed, last_stream_id, h2_error_code::NO_ERROR).await;
            return Ok(());
        }

        let frame = framed.read_frame(recv_max_frame_size).await?;
        match frame.header.frame_type() {
            http2::FrameType::Settings => {
                let is_ack = validate_settings_frame(
                    frame.header.stream_id,
                    frame.header.flags,
                    &frame.payload,
                )?;
                if is_ack {
                    continue;
                }
                apply_http2_settings_with_fc(
                    &mut hpack,
                    &mut peer_max_frame_size,
                    Some(&mut flow_control),
                    &frame.payload,
                )?;
                framed
                    .write_frame(http2::FrameType::Settings, FLAG_ACK, 0, &[])
                    .await?;
            }
            http2::FrameType::Ping => {
                if frame.header.stream_id != 0 || frame.payload.len() != 8 {
                    return Err(http2::Http2Error::Protocol("invalid PING frame").into());
                }
                if (frame.header.flags & FLAG_ACK) == 0 {
                    framed
                        .write_frame(http2::FrameType::Ping, FLAG_ACK, 0, &frame.payload)
                        .await?;
                }
            }
            http2::FrameType::Goaway => {
                validate_goaway_payload(&frame.payload)?;
                return Ok(());
            }
            http2::FrameType::PushPromise => {
                return Err(
                    http2::Http2Error::Protocol("PUSH_PROMISE not supported by server").into(),
                );
            }
            http2::FrameType::Headers => {
                if frame.header.stream_id == 0 {
                    return Err(
                        http2::Http2Error::Protocol("HEADERS must not be on stream 0").into(),
                    );
                }
                let stream_id = frame.header.stream_id;
                last_stream_id = stream_id;
                let (end_stream, mut header_block) =
                    extract_header_block_fragment(frame.header.flags, &frame.payload)?;

                if (frame.header.flags & FLAG_END_HEADERS) == 0 {
                    loop {
                        let cont = framed.read_frame(recv_max_frame_size).await?;
                        if cont.header.frame_type() != http2::FrameType::Continuation
                            || cont.header.stream_id != stream_id
                        {
                            return Err(http2::Http2Error::Protocol(
                                "expected CONTINUATION for header block",
                            )
                            .into());
                        }
                        header_block.extend_from_slice(&cont.payload);
                        if (cont.header.flags & FLAG_END_HEADERS) != 0 {
                            break;
                        }
                    }
                }

                let headers = hpack
                    .decode(&header_block)
                    .map_err(http2::Http2Error::from)?;
                let mut request = request_from_h2_headers(headers)?;

                if !end_stream {
                    let mut body = Vec::new();
                    let mut stream_reset = false;
                    let mut stream_received: u32 = 0;
                    loop {
                        let f = framed.read_frame(recv_max_frame_size).await?;
                        match f.header.frame_type() {
                            http2::FrameType::Data if f.header.stream_id == 0 => {
                                return Err(http2::Http2Error::Protocol(
                                    "DATA must not be on stream 0",
                                )
                                .into());
                            }
                            http2::FrameType::Data if f.header.stream_id == stream_id => {
                                let (data, data_end_stream) =
                                    extract_data_payload(f.header.flags, &f.payload)?;
                                if body.len().saturating_add(data.len()) > default_body_limit {
                                    return Err(http2::Http2Error::Protocol(
                                        "request body exceeds configured limit",
                                    )
                                    .into());
                                }
                                body.extend_from_slice(data);

                                // Flow control: track received data and send
                                // WINDOW_UPDATEs to prevent sender stalling.
                                let data_len = u32::try_from(data.len()).unwrap_or(u32::MAX);
                                stream_received += data_len;
                                let conn_inc = flow_control.data_received_connection(data_len);
                                let stream_inc = flow_control.stream_window_update(stream_received);
                                if stream_inc > 0 {
                                    stream_received = 0;
                                }
                                send_window_updates(&mut framed, conn_inc, stream_id, stream_inc)
                                    .await?;

                                if data_end_stream {
                                    break;
                                }
                            }
                            http2::FrameType::RstStream => {
                                validate_rst_stream_payload(f.header.stream_id, &f.payload)?;
                                if f.header.stream_id == stream_id {
                                    stream_reset = true;
                                    break;
                                }
                            }
                            http2::FrameType::PushPromise => {
                                return Err(http2::Http2Error::Protocol(
                                    "PUSH_PROMISE not supported by server",
                                )
                                .into());
                            }
                            http2::FrameType::Settings
                            | http2::FrameType::Ping
                            | http2::FrameType::Goaway
                            | http2::FrameType::WindowUpdate
                            | http2::FrameType::Priority
                            | http2::FrameType::Unknown => {
                                if f.header.frame_type() == http2::FrameType::Goaway {
                                    validate_goaway_payload(&f.payload)?;
                                    return Ok(());
                                }
                                if f.header.frame_type() == http2::FrameType::Priority {
                                    validate_priority_payload(f.header.stream_id, &f.payload)?;
                                }
                                if f.header.frame_type() == http2::FrameType::WindowUpdate {
                                    validate_window_update_payload(&f.payload)?;
                                    // Track send-side window updates from peer.
                                    if f.payload.len() >= 4 {
                                        let increment = u32::from_be_bytes([
                                            f.payload[0],
                                            f.payload[1],
                                            f.payload[2],
                                            f.payload[3],
                                        ]) & 0x7FFF_FFFF;
                                        if f.header.stream_id == 0 {
                                            apply_send_conn_window_update(
                                                &mut flow_control,
                                                increment,
                                            )?;
                                        }
                                    }
                                }
                                if f.header.frame_type() == http2::FrameType::Ping {
                                    if f.header.stream_id != 0 || f.payload.len() != 8 {
                                        return Err(http2::Http2Error::Protocol(
                                            "invalid PING frame",
                                        )
                                        .into());
                                    }
                                    if (f.header.flags & FLAG_ACK) == 0 {
                                        framed
                                            .write_frame(
                                                http2::FrameType::Ping,
                                                FLAG_ACK,
                                                0,
                                                &f.payload,
                                            )
                                            .await?;
                                    }
                                }
                                if f.header.frame_type() == http2::FrameType::Settings {
                                    let is_ack = validate_settings_frame(
                                        f.header.stream_id,
                                        f.header.flags,
                                        &f.payload,
                                    )?;
                                    if !is_ack {
                                        apply_http2_settings_with_fc(
                                            &mut hpack,
                                            &mut peer_max_frame_size,
                                            Some(&mut flow_control),
                                            &f.payload,
                                        )?;
                                        framed
                                            .write_frame(
                                                http2::FrameType::Settings,
                                                FLAG_ACK,
                                                0,
                                                &[],
                                            )
                                            .await?;
                                    }
                                }
                            }
                            _ => {
                                return Err(http2::Http2Error::Protocol(
                                    "unsupported frame while reading request body",
                                )
                                .into());
                            }
                        }
                    }
                    if stream_reset {
                        continue;
                    }
                    request.set_body(fastapi_core::Body::Bytes(body));
                }

                let request_id = request_counter.fetch_add(1, Ordering::Relaxed);
                let request_budget = Budget::new().with_deadline(config.request_timeout);
                let request_cx = Cx::for_testing_with_budget(request_budget);
                let ctx = RequestContext::new(request_cx, request_id);

                if let Err(err) = validate_host_header(&request, config) {
                    let response = err.response();
                    process_connection_http2_write_response(
                        &mut framed,
                        response,
                        stream_id,
                        peer_max_frame_size,
                        recv_max_frame_size,
                        Some(&mut flow_control),
                    )
                    .await?;
                    continue;
                }

                if let Err(response) = config.pre_body_validators.validate_all(&request) {
                    process_connection_http2_write_response(
                        &mut framed,
                        response,
                        stream_id,
                        peer_max_frame_size,
                        recv_max_frame_size,
                        Some(&mut flow_control),
                    )
                    .await?;
                    continue;
                }

                let response = handler(ctx, &mut request).await;
                process_connection_http2_write_response(
                    &mut framed,
                    response,
                    stream_id,
                    peer_max_frame_size,
                    recv_max_frame_size,
                    Some(&mut flow_control),
                )
                .await?;

                if let Some(tasks) = App::take_background_tasks(&mut request) {
                    tasks.execute_all().await;
                }
            }
            _ => {}
        }
    }
}

async fn process_connection_http2_write_response(
    framed: &mut http2::FramedH2,
    response: Response,
    stream_id: u32,
    peer_max_frame_size: u32,
    recv_max_frame_size: u32,
    mut flow_control: Option<&mut http2::H2FlowControl>,
) -> Result<(), ServerError> {
    use std::future::poll_fn;

    const FLAG_END_STREAM: u8 = 0x1;
    const FLAG_END_HEADERS: u8 = 0x4;

    let (status, mut headers, mut body) = response.into_parts();
    if !status.allows_body() {
        body = fastapi_core::ResponseBody::Empty;
    }

    let mut add_content_length = matches!(body, fastapi_core::ResponseBody::Bytes(_));
    for (name, _) in &headers {
        if name.eq_ignore_ascii_case("content-length") {
            add_content_length = false;
            break;
        }
    }
    if add_content_length {
        headers.push((
            "content-length".to_string(),
            body.len().to_string().into_bytes(),
        ));
    }

    let mut block: Vec<u8> = Vec::new();
    let status_bytes = status.as_u16().to_string().into_bytes();
    http2::hpack_encode_literal_without_indexing(&mut block, b":status", &status_bytes);
    for (name, value) in &headers {
        if is_h2_forbidden_header_name(name) {
            continue;
        }
        let n = name.to_ascii_lowercase();
        http2::hpack_encode_literal_without_indexing(&mut block, n.as_bytes(), value);
    }

    let max = usize::try_from(peer_max_frame_size).unwrap_or(16 * 1024);
    let mut headers_flags = FLAG_END_HEADERS;
    if body.is_empty() {
        headers_flags |= FLAG_END_STREAM;
    }

    if block.len() <= max {
        framed
            .write_frame(http2::FrameType::Headers, headers_flags, stream_id, &block)
            .await?;
    } else {
        // Split into HEADERS + CONTINUATION.
        let mut first_flags = 0u8;
        if body.is_empty() {
            first_flags |= FLAG_END_STREAM;
        }
        let (first, rest) = block.split_at(max);
        framed
            .write_frame(http2::FrameType::Headers, first_flags, stream_id, first)
            .await?;
        let mut remaining = rest;
        while remaining.len() > max {
            let (chunk, r) = remaining.split_at(max);
            framed
                .write_frame(http2::FrameType::Continuation, 0, stream_id, chunk)
                .await?;
            remaining = r;
        }
        framed
            .write_frame(
                http2::FrameType::Continuation,
                FLAG_END_HEADERS,
                stream_id,
                remaining,
            )
            .await?;
    }

    // Track per-stream send window (peer's receive window for this stream).
    let mut stream_send_window: i64 = flow_control
        .as_ref()
        .map_or(i64::MAX, |fc| i64::from(fc.peer_initial_window_size()));

    match body {
        fastapi_core::ResponseBody::Empty => Ok(()),
        fastapi_core::ResponseBody::Bytes(bytes) => {
            if bytes.is_empty() {
                return Ok(());
            }
            let mut remaining = bytes.as_slice();
            while !remaining.is_empty() {
                let is_last = remaining.len() <= max;
                let flags = if is_last { FLAG_END_STREAM } else { 0 };
                let send_len = remaining.len().min(max);

                let send_len = h2_fc_clamp_send(
                    framed,
                    &mut flow_control,
                    &mut stream_send_window,
                    stream_id,
                    send_len,
                    recv_max_frame_size,
                )
                .await?;

                let (chunk, r) = remaining.split_at(send_len);
                let flags = if r.is_empty() { FLAG_END_STREAM } else { flags };
                framed
                    .write_frame(http2::FrameType::Data, flags, stream_id, chunk)
                    .await?;
                remaining = r;
            }
            Ok(())
        }
        fastapi_core::ResponseBody::Stream(mut s) => {
            loop {
                let next = poll_fn(|cx| Pin::new(&mut s).poll_next(cx)).await;
                match next {
                    Some(chunk) => {
                        let mut remaining = chunk.as_slice();
                        while !remaining.is_empty() {
                            let send_len = remaining.len().min(max);
                            let send_len = h2_fc_clamp_send(
                                framed,
                                &mut flow_control,
                                &mut stream_send_window,
                                stream_id,
                                send_len,
                                recv_max_frame_size,
                            )
                            .await?;

                            let (c, r) = remaining.split_at(send_len);
                            framed
                                .write_frame(http2::FrameType::Data, 0, stream_id, c)
                                .await?;
                            remaining = r;
                        }
                    }
                    None => {
                        framed
                            .write_frame(http2::FrameType::Data, FLAG_END_STREAM, stream_id, &[])
                            .await?;
                        break;
                    }
                }
            }
            Ok(())
        }
    }
}

/// Clamp `desired` bytes against the send-side flow control windows. If windows
/// are exhausted, reads frames from the peer (draining WINDOW_UPDATEs, handling
/// PING/SETTINGS) until enough window is available. Returns the number of bytes
/// that can be sent now (always > 0 on success).
async fn h2_fc_clamp_send(
    framed: &mut http2::FramedH2,
    flow_control: &mut Option<&mut http2::H2FlowControl>,
    stream_send_window: &mut i64,
    stream_id: u32,
    desired: usize,
    recv_max_frame_size: u32,
) -> Result<usize, ServerError> {
    let fc = match flow_control.as_mut() {
        Some(fc) => fc,
        None => return Ok(desired),
    };

    loop {
        let conn_avail = usize::try_from(fc.send_conn_window().max(0)).unwrap_or(0);
        let stream_avail = usize::try_from((*stream_send_window).max(0)).unwrap_or(0);
        let allowed = desired.min(conn_avail).min(stream_avail);

        if allowed > 0 {
            let send = allowed;
            fc.consume_send_conn_window(u32::try_from(send).unwrap_or(u32::MAX));
            *stream_send_window -= i64::try_from(send).unwrap_or(i64::MAX);
            return Ok(send);
        }

        // Window exhausted -- read peer frames until we get a WINDOW_UPDATE.
        let frame = framed.read_frame(recv_max_frame_size).await?;
        match frame.header.frame_type() {
            http2::FrameType::WindowUpdate => {
                apply_peer_window_update_for_send(
                    fc,
                    stream_send_window,
                    stream_id,
                    frame.header.stream_id,
                    &frame.payload,
                )?;
            }
            http2::FrameType::Ping => {
                if frame.header.stream_id != 0 || frame.payload.len() != 8 {
                    return Err(ServerError::Http2(http2::Http2Error::Protocol(
                        "invalid PING frame",
                    )));
                }
                if frame.header.flags & 0x1 == 0 {
                    framed
                        .write_frame(http2::FrameType::Ping, 0x1, 0, &frame.payload)
                        .await?;
                }
            }
            http2::FrameType::Settings => {
                let is_ack = validate_settings_frame(
                    frame.header.stream_id,
                    frame.header.flags,
                    &frame.payload,
                )?;
                if !is_ack {
                    apply_peer_settings_for_send(fc, stream_send_window, &frame.payload)?;
                    // ACK the peer's SETTINGS.
                    framed
                        .write_frame(http2::FrameType::Settings, 0x1, 0, &[])
                        .await?;
                }
            }
            http2::FrameType::Goaway => {
                validate_goaway_payload(&frame.payload)?;
                return Err(ServerError::Http2(http2::Http2Error::Protocol(
                    "received GOAWAY while writing response",
                )));
            }
            http2::FrameType::RstStream => {
                validate_rst_stream_payload(frame.header.stream_id, &frame.payload)?;
                if frame.header.stream_id == stream_id {
                    return Err(ServerError::Http2(http2::Http2Error::Protocol(
                        "stream reset by peer during response",
                    )));
                }
            }
            _ => { /* ignore unknown/irrelevant frames */ }
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
            total_timed_out: self
                .metrics_counters
                .total_timed_out
                .load(Ordering::Relaxed),
            total_requests: self.request_counter.load(Ordering::Relaxed),
            bytes_in: self.metrics_counters.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.metrics_counters.bytes_out.load(Ordering::Relaxed),
        }
    }

    /// Records bytes read from a client.
    fn record_bytes_in(&self, n: u64) {
        self.metrics_counters
            .bytes_in
            .fetch_add(n, Ordering::Relaxed);
    }

    /// Records bytes written to a client.
    fn record_bytes_out(&self, n: u64) {
        self.metrics_counters
            .bytes_out
            .fetch_add(n, Ordering::Relaxed);
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
            self.metrics_counters
                .total_accepted
                .fetch_add(1, Ordering::Relaxed);
            return true;
        }

        // Try to increment if under limit
        let mut current = self.connection_counter.load(Ordering::Relaxed);
        loop {
            if current >= max as u64 {
                self.metrics_counters
                    .total_rejected
                    .fetch_add(1, Ordering::Relaxed);
                return false;
            }
            match self.connection_counter.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.metrics_counters
                        .total_accepted
                        .fetch_add(1, Ordering::Relaxed);
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
            // NOTE: We use blocking sleep here intentionally:
            // 1. This is only called during graceful shutdown (not a hot path)
            // 2. The default poll interval is 10ms (minimal CPU impact)
            // 3. During shutdown, blocking briefly is acceptable
            // 4. Using async sleep requires threading Time (or Cx) through this API
            //
            // If this becomes a bottleneck, consider:
            // - Using asupersync::runtime::yield_now() in a tighter loop
            // - Adding a Cx parameter to access async sleep
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
    /// the `Handler` trait (like `App`).
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

    /// Runs the server for a concrete [`App`].
    ///
    /// This enables protocol-aware features that require connection ownership,
    /// such as WebSocket upgrades.
    pub async fn serve_app(&self, cx: &Cx, app: Arc<App>) -> Result<(), ServerError> {
        let bind_addr = self.config.bind_addr.clone();
        let listener = TcpListener::bind(bind_addr).await?;
        let local_addr = listener.local_addr()?;

        cx.trace(&format!("Server listening on {local_addr}"));
        self.accept_loop_app(cx, listener, app).await
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

    /// Runs the server on a specific listener for a concrete [`App`].
    ///
    /// This enables protocol-aware features that require connection ownership,
    /// such as WebSocket upgrades, while allowing callers (tests/embedders) to
    /// control the bind step and observe the selected local address.
    pub async fn serve_on_app(
        &self,
        cx: &Cx,
        listener: TcpListener,
        app: Arc<App>,
    ) -> Result<(), ServerError> {
        self.accept_loop_app(cx, listener, app).await
    }

    async fn accept_loop_app(
        &self,
        cx: &Cx,
        listener: TcpListener,
        app: Arc<App>,
    ) -> Result<(), ServerError> {
        loop {
            if cx.is_cancel_requested() {
                cx.trace("Server shutdown requested");
                return Ok(());
            }
            if self.is_draining() {
                cx.trace("Server draining, stopping accept loop");
                return Err(ServerError::Shutdown);
            }

            let (mut stream, peer_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => {
                    cx.trace(&format!("Accept error: {e}"));
                    if is_fatal_accept_error(&e) {
                        return Err(ServerError::Io(e));
                    }
                    continue;
                }
            };

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

            let result = self
                .handle_connection_app(cx, stream, peer_addr, app.as_ref())
                .await;

            self.release_connection();

            if let Err(e) = result {
                cx.trace(&format!("Connection error from {peer_addr}: {e}"));
            }
        }
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
                // Current default: print to stderr. A structured sink can be wired via fastapi-core::logging.
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

    async fn handle_connection_app(
        &self,
        cx: &Cx,
        mut stream: TcpStream,
        peer_addr: SocketAddr,
        app: &App,
    ) -> Result<(), ServerError> {
        let (proto, buffered) = sniff_protocol(&mut stream, self.config.keep_alive_timeout).await?;
        if !buffered.is_empty() {
            self.record_bytes_in(buffered.len() as u64);
        }

        if proto == SniffedProtocol::Http2PriorKnowledge {
            return self
                .handle_connection_app_http2(cx, stream, peer_addr, app)
                .await;
        }

        let mut parser = StatefulParser::new().with_limits(self.config.parse_limits.clone());
        if !buffered.is_empty() {
            parser.feed(&buffered)?;
        }
        let mut read_buffer = vec![0u8; self.config.read_buffer_size];
        let mut response_writer = ResponseWriter::new();
        let mut requests_on_connection: usize = 0;
        let max_requests = self.config.max_requests_per_connection;

        loop {
            if cx.is_cancel_requested() {
                return Ok(());
            }

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
                                self.metrics_counters
                                    .total_timed_out
                                    .fetch_add(1, Ordering::Relaxed);
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

            let request_id = self.request_counter.fetch_add(1, Ordering::Relaxed);

            // Per-request budget for HTTP requests.
            let request_budget = Budget::new().with_deadline(self.config.request_timeout);
            let request_cx = Cx::for_testing_with_budget(request_budget);
            let overrides = app.dependency_overrides();
            let ctx = RequestContext::with_overrides_and_body_limit(
                request_cx,
                request_id,
                overrides,
                app.config().max_body_size,
            );

            // Validate Host header
            if let Err(err) = validate_host_header(&request, &self.config) {
                ctx.trace(&format!(
                    "Rejecting request from {peer_addr}: {}",
                    err.detail
                ));
                let response = err.response().header("connection", b"close".to_vec());
                let response_write = response_writer.write(response);
                write_response(&mut stream, response_write).await?;
                return Ok(());
            }

            // Header-only validators before any body reads / 100-continue.
            if let Err(response) = self.config.pre_body_validators.validate_all(&request) {
                let response = response.header("connection", b"close".to_vec());
                let response_write = response_writer.write(response);
                write_response(&mut stream, response_write).await?;
                return Ok(());
            }

            // WebSocket upgrade: only attempt when request looks like a WS handshake.
            //
            // NOTE: This consumes the connection: after a successful 101 upgrade, we hand the
            // TcpStream to the websocket handler and stop HTTP keep-alive processing.
            if is_websocket_upgrade_request(&request)
                && app.websocket_route_count() > 0
                && app.has_websocket_route(request.path())
            {
                // WebSocket handshake must not have a request body.
                if has_request_body_headers(&request) {
                    let response = Response::with_status(StatusCode::BAD_REQUEST)
                        .header("connection", b"close".to_vec())
                        .body(fastapi_core::ResponseBody::Bytes(
                            b"Bad Request: websocket handshake must not include a body".to_vec(),
                        ));
                    let response_write = response_writer.write(response);
                    write_response(&mut stream, response_write).await?;
                    return Ok(());
                }

                let Some(key) = header_str(&request, "sec-websocket-key") else {
                    let response = Response::with_status(StatusCode::BAD_REQUEST)
                        .header("connection", b"close".to_vec())
                        .body(fastapi_core::ResponseBody::Bytes(
                            b"Bad Request: missing Sec-WebSocket-Key".to_vec(),
                        ));
                    let response_write = response_writer.write(response);
                    write_response(&mut stream, response_write).await?;
                    return Ok(());
                };
                let accept = match fastapi_core::websocket_accept_from_key(key) {
                    Ok(v) => v,
                    Err(_) => {
                        let response = Response::with_status(StatusCode::BAD_REQUEST)
                            .header("connection", b"close".to_vec())
                            .body(fastapi_core::ResponseBody::Bytes(
                                b"Bad Request: invalid Sec-WebSocket-Key".to_vec(),
                            ));
                        let response_write = response_writer.write(response);
                        write_response(&mut stream, response_write).await?;
                        return Ok(());
                    }
                };

                if header_str(&request, "sec-websocket-version") != Some("13") {
                    let response = Response::with_status(StatusCode::BAD_REQUEST)
                        .header("sec-websocket-version", b"13".to_vec())
                        .header("connection", b"close".to_vec())
                        .body(fastapi_core::ResponseBody::Bytes(
                            b"Bad Request: unsupported Sec-WebSocket-Version".to_vec(),
                        ));
                    let response_write = response_writer.write(response);
                    write_response(&mut stream, response_write).await?;
                    return Ok(());
                }

                let response = Response::with_status(StatusCode::SWITCHING_PROTOCOLS)
                    .header("upgrade", b"websocket".to_vec())
                    .header("connection", b"Upgrade".to_vec())
                    .header("sec-websocket-accept", accept.into_bytes());
                let response_write = response_writer.write(response);
                if let ResponseWrite::Full(ref bytes) = response_write {
                    self.record_bytes_out(bytes.len() as u64);
                }
                write_response(&mut stream, response_write).await?;

                // Hand off any already-read bytes to the websocket layer.
                let buffered = parser.take_buffered();

                // WebSocket connections are long-lived; do not inherit the per-request deadline.
                let ws_root_cx = Cx::for_testing_with_budget(Budget::new());
                let ws_ctx = RequestContext::with_overrides_and_body_limit(
                    ws_root_cx,
                    request_id,
                    app.dependency_overrides(),
                    app.config().max_body_size,
                );

                let ws = fastapi_core::WebSocket::new(stream, buffered);
                let _ = app.handle_websocket(&ws_ctx, &mut request, ws).await;
                return Ok(());
            }

            // Handle Expect: 100-continue
            match ExpectHandler::check_expect(&request) {
                ExpectResult::NoExpectation => {}
                ExpectResult::ExpectsContinue => {
                    ctx.trace("Sending 100 Continue for Expect: 100-continue");
                    write_raw_response(&mut stream, CONTINUE_RESPONSE).await?;
                }
                ExpectResult::UnknownExpectation(value) => {
                    ctx.trace(&format!("Rejecting unknown Expect value: {}", value));
                    let response = ExpectHandler::expectation_failed(format!(
                        "Unsupported Expect value: {value}"
                    ));
                    let response_write = response_writer.write(response);
                    write_response(&mut stream, response_write).await?;
                    return Ok(());
                }
            }

            let client_wants_keep_alive = should_keep_alive(&request);
            let server_will_keep_alive = client_wants_keep_alive
                && (max_requests == 0 || requests_on_connection < max_requests);

            let request_start = Instant::now();
            let timeout_duration = Duration::from_nanos(self.config.request_timeout.as_nanos());

            let response = app.handle(&ctx, &mut request).await;
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
            if let ResponseWrite::Full(ref bytes) = response_write {
                self.record_bytes_out(bytes.len() as u64);
            }
            write_response(&mut stream, response_write).await?;

            if let Some(tasks) = App::take_background_tasks(&mut request) {
                tasks.execute_all().await;
            }

            if !server_will_keep_alive {
                return Ok(());
            }
        }
    }

    async fn handle_connection_app_http2(
        &self,
        cx: &Cx,
        stream: TcpStream,
        _peer_addr: SocketAddr,
        app: &App,
    ) -> Result<(), ServerError> {
        const FLAG_END_STREAM: u8 = 0x1;
        const FLAG_END_HEADERS: u8 = 0x4;
        const FLAG_ACK: u8 = 0x1;

        let mut framed = http2::FramedH2::new(stream, Vec::new());
        let mut hpack = http2::HpackDecoder::new();
        let recv_max_frame_size: u32 = 16 * 1024; // RFC 7540 default receive limit.
        let mut peer_max_frame_size: u32 = 16 * 1024;
        let mut flow_control = http2::H2FlowControl::new();

        let first = framed.read_frame(recv_max_frame_size).await?;
        self.record_bytes_in((http2::FrameHeader::LEN + first.payload.len()) as u64);

        if first.header.frame_type() != http2::FrameType::Settings
            || first.header.stream_id != 0
            || (first.header.flags & FLAG_ACK) != 0
        {
            return Err(
                http2::Http2Error::Protocol("expected client SETTINGS after preface").into(),
            );
        }

        apply_http2_settings_with_fc(
            &mut hpack,
            &mut peer_max_frame_size,
            Some(&mut flow_control),
            &first.payload,
        )?;

        // Send server SETTINGS (empty for now) and ACK the client's SETTINGS.
        framed
            .write_frame(http2::FrameType::Settings, 0, 0, &[])
            .await?;
        self.record_bytes_out(http2::FrameHeader::LEN as u64);

        framed
            .write_frame(http2::FrameType::Settings, FLAG_ACK, 0, &[])
            .await?;
        self.record_bytes_out(http2::FrameHeader::LEN as u64);
        let mut last_stream_id: u32 = 0;

        loop {
            if cx.is_cancel_requested() {
                let _ = send_goaway(&mut framed, last_stream_id, h2_error_code::NO_ERROR).await;
                return Ok(());
            }

            let frame = framed.read_frame(recv_max_frame_size).await?;
            self.record_bytes_in((http2::FrameHeader::LEN + frame.payload.len()) as u64);

            match frame.header.frame_type() {
                http2::FrameType::Settings => {
                    let is_ack = validate_settings_frame(
                        frame.header.stream_id,
                        frame.header.flags,
                        &frame.payload,
                    )?;
                    if is_ack {
                        // ACK for our SETTINGS.
                        continue;
                    }
                    apply_http2_settings_with_fc(
                        &mut hpack,
                        &mut peer_max_frame_size,
                        Some(&mut flow_control),
                        &frame.payload,
                    )?;
                    // ACK peer SETTINGS.
                    framed
                        .write_frame(http2::FrameType::Settings, FLAG_ACK, 0, &[])
                        .await?;
                    self.record_bytes_out(http2::FrameHeader::LEN as u64);
                }
                http2::FrameType::Ping => {
                    // Respond to pings to avoid clients stalling.
                    if frame.header.stream_id != 0 || frame.payload.len() != 8 {
                        return Err(http2::Http2Error::Protocol("invalid PING frame").into());
                    }
                    if (frame.header.flags & FLAG_ACK) == 0 {
                        framed
                            .write_frame(http2::FrameType::Ping, FLAG_ACK, 0, &frame.payload)
                            .await?;
                        self.record_bytes_out((http2::FrameHeader::LEN + 8) as u64);
                    }
                }
                http2::FrameType::Goaway => {
                    validate_goaway_payload(&frame.payload)?;
                    return Ok(());
                }
                http2::FrameType::PushPromise => {
                    return Err(http2::Http2Error::Protocol(
                        "PUSH_PROMISE not supported by server",
                    )
                    .into());
                }
                http2::FrameType::Headers => {
                    if frame.header.stream_id == 0 {
                        return Err(
                            http2::Http2Error::Protocol("HEADERS must not be on stream 0").into(),
                        );
                    }

                    let stream_id = frame.header.stream_id;
                    last_stream_id = stream_id;
                    let (end_stream, mut header_block) =
                        extract_header_block_fragment(frame.header.flags, &frame.payload)?;

                    // CONTINUATION frames until END_HEADERS.
                    if (frame.header.flags & FLAG_END_HEADERS) == 0 {
                        loop {
                            let cont = framed.read_frame(recv_max_frame_size).await?;
                            self.record_bytes_in(
                                (http2::FrameHeader::LEN + cont.payload.len()) as u64,
                            );
                            if cont.header.frame_type() != http2::FrameType::Continuation
                                || cont.header.stream_id != stream_id
                            {
                                return Err(http2::Http2Error::Protocol(
                                    "expected CONTINUATION for header block",
                                )
                                .into());
                            }
                            header_block.extend_from_slice(&cont.payload);
                            if (cont.header.flags & FLAG_END_HEADERS) != 0 {
                                break;
                            }
                        }
                    }

                    let headers = hpack
                        .decode(&header_block)
                        .map_err(http2::Http2Error::from)?;
                    let mut request = request_from_h2_headers(headers)?;
                    request.set_version(fastapi_core::HttpVersion::Http2);

                    // If there is a body, read DATA frames until END_STREAM.
                    if !end_stream {
                        let max = app.config().max_body_size;
                        let mut body = Vec::new();
                        let mut stream_reset = false;
                        let mut stream_received: u32 = 0;
                        loop {
                            let f = framed.read_frame(recv_max_frame_size).await?;
                            self.record_bytes_in(
                                (http2::FrameHeader::LEN + f.payload.len()) as u64,
                            );
                            match f.header.frame_type() {
                                http2::FrameType::Data if f.header.stream_id == 0 => {
                                    return Err(http2::Http2Error::Protocol(
                                        "DATA must not be on stream 0",
                                    )
                                    .into());
                                }
                                http2::FrameType::Data if f.header.stream_id == stream_id => {
                                    let (data, data_end_stream) =
                                        extract_data_payload(f.header.flags, &f.payload)?;
                                    if body.len().saturating_add(data.len()) > max {
                                        return Err(http2::Http2Error::Protocol(
                                            "request body exceeds configured max_body_size",
                                        )
                                        .into());
                                    }
                                    body.extend_from_slice(data);

                                    // Flow control: track received data and send
                                    // WINDOW_UPDATEs to prevent sender stalling.
                                    let data_len = u32::try_from(data.len()).unwrap_or(u32::MAX);
                                    stream_received += data_len;
                                    let conn_inc = flow_control.data_received_connection(data_len);
                                    let stream_inc =
                                        flow_control.stream_window_update(stream_received);
                                    if stream_inc > 0 {
                                        stream_received = 0;
                                    }
                                    send_window_updates(
                                        &mut framed,
                                        conn_inc,
                                        stream_id,
                                        stream_inc,
                                    )
                                    .await?;

                                    if data_end_stream {
                                        break;
                                    }
                                }
                                http2::FrameType::RstStream => {
                                    validate_rst_stream_payload(f.header.stream_id, &f.payload)?;
                                    if f.header.stream_id == stream_id {
                                        stream_reset = true;
                                        break;
                                    }
                                }
                                http2::FrameType::PushPromise => {
                                    return Err(http2::Http2Error::Protocol(
                                        "PUSH_PROMISE not supported by server",
                                    )
                                    .into());
                                }
                                http2::FrameType::Settings
                                | http2::FrameType::Ping
                                | http2::FrameType::Goaway
                                | http2::FrameType::WindowUpdate
                                | http2::FrameType::Priority
                                | http2::FrameType::Unknown => {
                                    if f.header.frame_type() == http2::FrameType::Goaway {
                                        validate_goaway_payload(&f.payload)?;
                                        return Ok(());
                                    }
                                    if f.header.frame_type() == http2::FrameType::Priority {
                                        validate_priority_payload(f.header.stream_id, &f.payload)?;
                                    }
                                    if f.header.frame_type() == http2::FrameType::WindowUpdate {
                                        validate_window_update_payload(&f.payload)?;
                                        // Track send-side window updates from peer.
                                        if f.payload.len() >= 4 {
                                            let increment = u32::from_be_bytes([
                                                f.payload[0],
                                                f.payload[1],
                                                f.payload[2],
                                                f.payload[3],
                                            ]) & 0x7FFF_FFFF;
                                            if f.header.stream_id == 0 {
                                                apply_send_conn_window_update(
                                                    &mut flow_control,
                                                    increment,
                                                )?;
                                            }
                                        }
                                    }
                                    if f.header.frame_type() == http2::FrameType::Ping {
                                        if f.header.stream_id != 0 || f.payload.len() != 8 {
                                            return Err(http2::Http2Error::Protocol(
                                                "invalid PING frame",
                                            )
                                            .into());
                                        }
                                        if (f.header.flags & FLAG_ACK) == 0 {
                                            framed
                                                .write_frame(
                                                    http2::FrameType::Ping,
                                                    FLAG_ACK,
                                                    0,
                                                    &f.payload,
                                                )
                                                .await?;
                                            self.record_bytes_out(
                                                (http2::FrameHeader::LEN + 8) as u64,
                                            );
                                        }
                                    }
                                    if f.header.frame_type() == http2::FrameType::Settings {
                                        let is_ack = validate_settings_frame(
                                            f.header.stream_id,
                                            f.header.flags,
                                            &f.payload,
                                        )?;
                                        if !is_ack {
                                            apply_http2_settings_with_fc(
                                                &mut hpack,
                                                &mut peer_max_frame_size,
                                                Some(&mut flow_control),
                                                &f.payload,
                                            )?;
                                            framed
                                                .write_frame(
                                                    http2::FrameType::Settings,
                                                    FLAG_ACK,
                                                    0,
                                                    &[],
                                                )
                                                .await?;
                                            self.record_bytes_out(http2::FrameHeader::LEN as u64);
                                        }
                                    }
                                }
                                _ => {
                                    return Err(http2::Http2Error::Protocol(
                                        "unsupported frame while reading request body",
                                    )
                                    .into());
                                }
                            }
                        }
                        if stream_reset {
                            continue;
                        }
                        request.set_body(fastapi_core::Body::Bytes(body));
                    }

                    let request_id = self.request_counter.fetch_add(1, Ordering::Relaxed);
                    let request_budget = Budget::new().with_deadline(self.config.request_timeout);
                    let request_cx = Cx::for_testing_with_budget(request_budget);
                    let overrides = app.dependency_overrides();
                    let ctx = RequestContext::with_overrides_and_body_limit(
                        request_cx,
                        request_id,
                        overrides,
                        app.config().max_body_size,
                    );

                    if let Err(err) = validate_host_header(&request, &self.config) {
                        ctx.trace(&format!("Rejecting HTTP/2 request: {}", err.detail));
                        let response = err.response();
                        self.write_h2_response(
                            &mut framed,
                            response,
                            stream_id,
                            peer_max_frame_size,
                            recv_max_frame_size,
                            Some(&mut flow_control),
                        )
                        .await?;
                        continue;
                    }

                    if let Err(response) = self.config.pre_body_validators.validate_all(&request) {
                        self.write_h2_response(
                            &mut framed,
                            response,
                            stream_id,
                            peer_max_frame_size,
                            recv_max_frame_size,
                            Some(&mut flow_control),
                        )
                        .await?;
                        continue;
                    }

                    let response = app.handle(&ctx, &mut request).await;

                    // Send response on the same stream.
                    self.write_h2_response(
                        &mut framed,
                        response,
                        stream_id,
                        peer_max_frame_size,
                        recv_max_frame_size,
                        Some(&mut flow_control),
                    )
                    .await?;

                    if let Some(tasks) = App::take_background_tasks(&mut request) {
                        tasks.execute_all().await;
                    }

                    // Yield to keep cancellation responsive.
                    asupersync::runtime::yield_now().await;
                }
                http2::FrameType::WindowUpdate => {
                    validate_window_update_payload(&frame.payload)?;
                    if frame.payload.len() >= 4 {
                        let increment = u32::from_be_bytes([
                            frame.payload[0],
                            frame.payload[1],
                            frame.payload[2],
                            frame.payload[3],
                        ]) & 0x7FFF_FFFF;
                        if frame.header.stream_id == 0 {
                            apply_send_conn_window_update(&mut flow_control, increment)?;
                        }
                    }
                }
                _ => {
                    // Ignore other frame types for now.
                }
            }
        }
    }

    async fn write_h2_response(
        &self,
        framed: &mut http2::FramedH2,
        response: Response,
        stream_id: u32,
        peer_max_frame_size: u32,
        recv_max_frame_size: u32,
        mut flow_control: Option<&mut http2::H2FlowControl>,
    ) -> Result<(), ServerError> {
        use std::future::poll_fn;

        const FLAG_END_STREAM: u8 = 0x1;
        const FLAG_END_HEADERS: u8 = 0x4;

        let (status, mut headers, mut body) = response.into_parts();
        if !status.allows_body() {
            body = fastapi_core::ResponseBody::Empty;
        }

        let mut add_content_length = matches!(body, fastapi_core::ResponseBody::Bytes(_));
        for (name, _) in &headers {
            if name.eq_ignore_ascii_case("content-length") {
                add_content_length = false;
                break;
            }
        }

        if add_content_length {
            let len = body.len();
            headers.push(("content-length".to_string(), len.to_string().into_bytes()));
        }

        // Encode headers: :status + response headers (filtered for HTTP/2).
        let mut block: Vec<u8> = Vec::new();
        let status_bytes = status.as_u16().to_string().into_bytes();
        http2::hpack_encode_literal_without_indexing(&mut block, b":status", &status_bytes);

        for (name, value) in &headers {
            if is_h2_forbidden_header_name(name) {
                continue;
            }
            let n = name.to_ascii_lowercase();
            http2::hpack_encode_literal_without_indexing(&mut block, n.as_bytes(), value);
        }

        // Write HEADERS + CONTINUATION if needed.
        let max = usize::try_from(peer_max_frame_size).unwrap_or(16 * 1024);
        if block.len() <= max {
            let mut flags = FLAG_END_HEADERS;
            if body.is_empty() {
                flags |= FLAG_END_STREAM;
            }
            framed
                .write_frame(http2::FrameType::Headers, flags, stream_id, &block)
                .await?;
            self.record_bytes_out((http2::FrameHeader::LEN + block.len()) as u64);
        } else {
            let mut flags = 0u8;
            if body.is_empty() {
                flags |= FLAG_END_STREAM;
            }
            let (first, rest) = block.split_at(max);
            framed
                .write_frame(http2::FrameType::Headers, flags, stream_id, first)
                .await?;
            self.record_bytes_out((http2::FrameHeader::LEN + first.len()) as u64);

            let mut remaining = rest;
            while remaining.len() > max {
                let (chunk, r) = remaining.split_at(max);
                framed
                    .write_frame(http2::FrameType::Continuation, 0, stream_id, chunk)
                    .await?;
                self.record_bytes_out((http2::FrameHeader::LEN + chunk.len()) as u64);
                remaining = r;
            }
            framed
                .write_frame(
                    http2::FrameType::Continuation,
                    FLAG_END_HEADERS,
                    stream_id,
                    remaining,
                )
                .await?;
            self.record_bytes_out((http2::FrameHeader::LEN + remaining.len()) as u64);
        }

        // Track per-stream send window (peer's receive window for this stream).
        let mut stream_send_window: i64 = flow_control
            .as_ref()
            .map_or(i64::MAX, |fc| i64::from(fc.peer_initial_window_size()));

        // Write body with send-side flow control.
        match body {
            fastapi_core::ResponseBody::Empty => Ok(()),
            fastapi_core::ResponseBody::Bytes(bytes) => {
                if bytes.is_empty() {
                    return Ok(());
                }
                let mut remaining = bytes.as_slice();
                while !remaining.is_empty() {
                    let send_len = remaining.len().min(max);
                    let send_len = h2_fc_clamp_send(
                        framed,
                        &mut flow_control,
                        &mut stream_send_window,
                        stream_id,
                        send_len,
                        recv_max_frame_size,
                    )
                    .await?;

                    let (chunk, r) = remaining.split_at(send_len);
                    let flags = if r.is_empty() { FLAG_END_STREAM } else { 0 };
                    framed
                        .write_frame(http2::FrameType::Data, flags, stream_id, chunk)
                        .await?;
                    self.record_bytes_out((http2::FrameHeader::LEN + chunk.len()) as u64);
                    remaining = r;
                }
                Ok(())
            }
            fastapi_core::ResponseBody::Stream(mut s) => {
                loop {
                    let next = poll_fn(|cx| Pin::new(&mut s).poll_next(cx)).await;
                    match next {
                        Some(chunk) => {
                            let mut remaining = chunk.as_slice();
                            while !remaining.is_empty() {
                                let send_len = remaining.len().min(max);
                                let send_len = h2_fc_clamp_send(
                                    framed,
                                    &mut flow_control,
                                    &mut stream_send_window,
                                    stream_id,
                                    send_len,
                                    recv_max_frame_size,
                                )
                                .await?;

                                let (c, r) = remaining.split_at(send_len);
                                framed
                                    .write_frame(http2::FrameType::Data, 0, stream_id, c)
                                    .await?;
                                self.record_bytes_out((http2::FrameHeader::LEN + c.len()) as u64);
                                remaining = r;
                            }
                        }
                        None => {
                            framed
                                .write_frame(
                                    http2::FrameType::Data,
                                    FLAG_END_STREAM,
                                    stream_id,
                                    &[],
                                )
                                .await?;
                            self.record_bytes_out(http2::FrameHeader::LEN as u64);
                            break;
                        }
                    }
                }
                Ok(())
            }
        }
    }

    async fn handle_connection_handler_http2(
        &self,
        cx: &Cx,
        stream: TcpStream,
        handler: &dyn fastapi_core::Handler,
    ) -> Result<(), ServerError> {
        const FLAG_END_HEADERS: u8 = 0x4;
        const FLAG_ACK: u8 = 0x1;

        let mut framed = http2::FramedH2::new(stream, Vec::new());
        let mut hpack = http2::HpackDecoder::new();
        let recv_max_frame_size: u32 = 16 * 1024;
        let mut peer_max_frame_size: u32 = 16 * 1024;
        let mut flow_control = http2::H2FlowControl::new();

        let first = framed.read_frame(recv_max_frame_size).await?;
        self.record_bytes_in((http2::FrameHeader::LEN + first.payload.len()) as u64);

        if first.header.frame_type() != http2::FrameType::Settings
            || first.header.stream_id != 0
            || (first.header.flags & FLAG_ACK) != 0
        {
            return Err(
                http2::Http2Error::Protocol("expected client SETTINGS after preface").into(),
            );
        }

        apply_http2_settings_with_fc(
            &mut hpack,
            &mut peer_max_frame_size,
            Some(&mut flow_control),
            &first.payload,
        )?;

        framed
            .write_frame(http2::FrameType::Settings, 0, 0, &[])
            .await?;
        self.record_bytes_out(http2::FrameHeader::LEN as u64);

        framed
            .write_frame(http2::FrameType::Settings, FLAG_ACK, 0, &[])
            .await?;
        self.record_bytes_out(http2::FrameHeader::LEN as u64);

        let default_body_limit = self.config.parse_limits.max_request_size;
        let mut last_stream_id: u32 = 0;

        loop {
            if cx.is_cancel_requested() {
                let _ = send_goaway(&mut framed, last_stream_id, h2_error_code::NO_ERROR).await;
                return Ok(());
            }

            let frame = framed.read_frame(recv_max_frame_size).await?;
            self.record_bytes_in((http2::FrameHeader::LEN + frame.payload.len()) as u64);

            match frame.header.frame_type() {
                http2::FrameType::Settings => {
                    let is_ack = validate_settings_frame(
                        frame.header.stream_id,
                        frame.header.flags,
                        &frame.payload,
                    )?;
                    if is_ack {
                        continue;
                    }
                    apply_http2_settings_with_fc(
                        &mut hpack,
                        &mut peer_max_frame_size,
                        Some(&mut flow_control),
                        &frame.payload,
                    )?;
                    framed
                        .write_frame(http2::FrameType::Settings, FLAG_ACK, 0, &[])
                        .await?;
                    self.record_bytes_out(http2::FrameHeader::LEN as u64);
                }
                http2::FrameType::Ping => {
                    if frame.header.stream_id != 0 || frame.payload.len() != 8 {
                        return Err(http2::Http2Error::Protocol("invalid PING frame").into());
                    }
                    if (frame.header.flags & FLAG_ACK) == 0 {
                        framed
                            .write_frame(http2::FrameType::Ping, FLAG_ACK, 0, &frame.payload)
                            .await?;
                        self.record_bytes_out((http2::FrameHeader::LEN + 8) as u64);
                    }
                }
                http2::FrameType::Goaway => {
                    validate_goaway_payload(&frame.payload)?;
                    return Ok(());
                }
                http2::FrameType::PushPromise => {
                    return Err(http2::Http2Error::Protocol(
                        "PUSH_PROMISE not supported by server",
                    )
                    .into());
                }
                http2::FrameType::Headers => {
                    if frame.header.stream_id == 0 {
                        return Err(
                            http2::Http2Error::Protocol("HEADERS must not be on stream 0").into(),
                        );
                    }

                    let stream_id = frame.header.stream_id;
                    last_stream_id = stream_id;
                    let (end_stream, mut header_block) =
                        extract_header_block_fragment(frame.header.flags, &frame.payload)?;

                    if (frame.header.flags & FLAG_END_HEADERS) == 0 {
                        loop {
                            let cont = framed.read_frame(recv_max_frame_size).await?;
                            self.record_bytes_in(
                                (http2::FrameHeader::LEN + cont.payload.len()) as u64,
                            );
                            if cont.header.frame_type() != http2::FrameType::Continuation
                                || cont.header.stream_id != stream_id
                            {
                                return Err(http2::Http2Error::Protocol(
                                    "expected CONTINUATION for header block",
                                )
                                .into());
                            }
                            header_block.extend_from_slice(&cont.payload);
                            if (cont.header.flags & FLAG_END_HEADERS) != 0 {
                                break;
                            }
                        }
                    }

                    let headers = hpack
                        .decode(&header_block)
                        .map_err(http2::Http2Error::from)?;
                    let mut request = request_from_h2_headers(headers)?;

                    if !end_stream {
                        let mut body = Vec::new();
                        let mut stream_reset = false;
                        let mut stream_received: u32 = 0;
                        loop {
                            let f = framed.read_frame(recv_max_frame_size).await?;
                            self.record_bytes_in(
                                (http2::FrameHeader::LEN + f.payload.len()) as u64,
                            );
                            match f.header.frame_type() {
                                http2::FrameType::Data if f.header.stream_id == 0 => {
                                    return Err(http2::Http2Error::Protocol(
                                        "DATA must not be on stream 0",
                                    )
                                    .into());
                                }
                                http2::FrameType::Data if f.header.stream_id == stream_id => {
                                    let (data, data_end_stream) =
                                        extract_data_payload(f.header.flags, &f.payload)?;
                                    if body.len().saturating_add(data.len()) > default_body_limit {
                                        return Err(http2::Http2Error::Protocol(
                                            "request body exceeds configured limit",
                                        )
                                        .into());
                                    }
                                    body.extend_from_slice(data);

                                    // Flow control: track received data and send
                                    // WINDOW_UPDATEs to prevent sender stalling.
                                    let data_len = u32::try_from(data.len()).unwrap_or(u32::MAX);
                                    stream_received += data_len;
                                    let conn_inc = flow_control.data_received_connection(data_len);
                                    let stream_inc =
                                        flow_control.stream_window_update(stream_received);
                                    if stream_inc > 0 {
                                        stream_received = 0;
                                    }
                                    send_window_updates(
                                        &mut framed,
                                        conn_inc,
                                        stream_id,
                                        stream_inc,
                                    )
                                    .await?;

                                    if data_end_stream {
                                        break;
                                    }
                                }
                                http2::FrameType::RstStream => {
                                    validate_rst_stream_payload(f.header.stream_id, &f.payload)?;
                                    if f.header.stream_id == stream_id {
                                        stream_reset = true;
                                        break;
                                    }
                                }
                                http2::FrameType::PushPromise => {
                                    return Err(http2::Http2Error::Protocol(
                                        "PUSH_PROMISE not supported by server",
                                    )
                                    .into());
                                }
                                http2::FrameType::Settings
                                | http2::FrameType::Ping
                                | http2::FrameType::Goaway
                                | http2::FrameType::WindowUpdate
                                | http2::FrameType::Priority
                                | http2::FrameType::Unknown => {
                                    if f.header.frame_type() == http2::FrameType::Goaway {
                                        validate_goaway_payload(&f.payload)?;
                                        return Ok(());
                                    }
                                    if f.header.frame_type() == http2::FrameType::Priority {
                                        validate_priority_payload(f.header.stream_id, &f.payload)?;
                                    }
                                    if f.header.frame_type() == http2::FrameType::WindowUpdate {
                                        validate_window_update_payload(&f.payload)?;
                                        // Track send-side window updates from peer.
                                        if f.payload.len() >= 4 {
                                            let increment = u32::from_be_bytes([
                                                f.payload[0],
                                                f.payload[1],
                                                f.payload[2],
                                                f.payload[3],
                                            ]) & 0x7FFF_FFFF;
                                            if f.header.stream_id == 0 {
                                                apply_send_conn_window_update(
                                                    &mut flow_control,
                                                    increment,
                                                )?;
                                            }
                                        }
                                    }
                                    if f.header.frame_type() == http2::FrameType::Ping {
                                        if f.header.stream_id != 0 || f.payload.len() != 8 {
                                            return Err(http2::Http2Error::Protocol(
                                                "invalid PING frame",
                                            )
                                            .into());
                                        }
                                        if (f.header.flags & FLAG_ACK) == 0 {
                                            framed
                                                .write_frame(
                                                    http2::FrameType::Ping,
                                                    FLAG_ACK,
                                                    0,
                                                    &f.payload,
                                                )
                                                .await?;
                                            self.record_bytes_out(
                                                (http2::FrameHeader::LEN + 8) as u64,
                                            );
                                        }
                                    }
                                    if f.header.frame_type() == http2::FrameType::Settings {
                                        let is_ack = validate_settings_frame(
                                            f.header.stream_id,
                                            f.header.flags,
                                            &f.payload,
                                        )?;
                                        if !is_ack {
                                            apply_http2_settings_with_fc(
                                                &mut hpack,
                                                &mut peer_max_frame_size,
                                                Some(&mut flow_control),
                                                &f.payload,
                                            )?;
                                            framed
                                                .write_frame(
                                                    http2::FrameType::Settings,
                                                    FLAG_ACK,
                                                    0,
                                                    &[],
                                                )
                                                .await?;
                                            self.record_bytes_out(http2::FrameHeader::LEN as u64);
                                        }
                                    }
                                }
                                _ => {
                                    return Err(http2::Http2Error::Protocol(
                                        "unsupported frame while reading request body",
                                    )
                                    .into());
                                }
                            }
                        }
                        if stream_reset {
                            continue;
                        }
                        request.set_body(fastapi_core::Body::Bytes(body));
                    }

                    let request_id = self.request_counter.fetch_add(1, Ordering::Relaxed);
                    let request_budget = Budget::new().with_deadline(self.config.request_timeout);
                    let request_cx = Cx::for_testing_with_budget(request_budget);

                    let overrides = handler
                        .dependency_overrides()
                        .unwrap_or_else(|| Arc::new(fastapi_core::DependencyOverrides::new()));

                    let ctx = RequestContext::with_overrides_and_body_limit(
                        request_cx,
                        request_id,
                        overrides,
                        default_body_limit,
                    );

                    if let Err(err) = validate_host_header(&request, &self.config) {
                        let response = err.response();
                        self.write_h2_response(
                            &mut framed,
                            response,
                            stream_id,
                            peer_max_frame_size,
                            recv_max_frame_size,
                            Some(&mut flow_control),
                        )
                        .await?;
                        continue;
                    }
                    if let Err(response) = self.config.pre_body_validators.validate_all(&request) {
                        self.write_h2_response(
                            &mut framed,
                            response,
                            stream_id,
                            peer_max_frame_size,
                            recv_max_frame_size,
                            Some(&mut flow_control),
                        )
                        .await?;
                        continue;
                    }

                    let response = handler.call(&ctx, &mut request).await;
                    self.write_h2_response(
                        &mut framed,
                        response,
                        stream_id,
                        peer_max_frame_size,
                        recv_max_frame_size,
                        Some(&mut flow_control),
                    )
                    .await?;
                }
                http2::FrameType::WindowUpdate => {
                    validate_window_update_payload(&frame.payload)?;
                    if frame.payload.len() >= 4 {
                        let increment = u32::from_be_bytes([
                            frame.payload[0],
                            frame.payload[1],
                            frame.payload[2],
                            frame.payload[3],
                        ]) & 0x7FFF_FFFF;
                        if frame.header.stream_id == 0 {
                            apply_send_conn_window_update(&mut flow_control, increment)?;
                        }
                    }
                }
                _ => {}
            }
        }
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
        let (proto, buffered) = sniff_protocol(&mut stream, self.config.keep_alive_timeout).await?;
        if !buffered.is_empty() {
            self.record_bytes_in(buffered.len() as u64);
        }
        if proto == SniffedProtocol::Http2PriorKnowledge {
            return self
                .handle_connection_handler_http2(cx, stream, handler)
                .await;
        }

        let mut parser = StatefulParser::new().with_limits(self.config.parse_limits.clone());
        if !buffered.is_empty() {
            parser.feed(&buffered)?;
        }
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
                                self.metrics_counters
                                    .total_timed_out
                                    .fetch_add(1, Ordering::Relaxed);
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

            // Handle inline (single-threaded accept loop).
            //
            // For concurrent connection handling with structured concurrency, use
            // `TcpServer::serve_concurrent()` which spawns tasks via asupersync `Scope`.
            let request_id = self.next_request_id();
            let request_budget = Budget::new().with_deadline(self.config.request_timeout);

            // Create a RequestContext for this request with the configured timeout budget.
            //
            // Note: today this uses a testing context budget helper; the intent is to
            // construct request contexts as children of a per-connection region.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SniffedProtocol {
    Http1,
    Http2PriorKnowledge,
}

/// Sniff whether the connection is HTTP/2 prior-knowledge (h2c preface).
///
/// Returns the inferred protocol and the bytes already consumed from the stream.
async fn sniff_protocol(
    stream: &mut TcpStream,
    keep_alive_timeout: Duration,
) -> io::Result<(SniffedProtocol, Vec<u8>)> {
    let mut buffered: Vec<u8> = Vec::new();
    let preface = http2::PREFACE;

    while buffered.len() < preface.len() {
        let mut tmp = vec![0u8; preface.len() - buffered.len()];
        let n = if keep_alive_timeout.is_zero() {
            read_into_buffer(stream, &mut tmp).await?
        } else {
            read_with_timeout(stream, &mut tmp, keep_alive_timeout).await?
        };
        if n == 0 {
            // EOF before any meaningful determination; treat as HTTP/1 with whatever we saw.
            return Ok((SniffedProtocol::Http1, buffered));
        }

        buffered.extend_from_slice(&tmp[..n]);
        if !preface.starts_with(&buffered) {
            return Ok((SniffedProtocol::Http1, buffered));
        }
    }

    Ok((SniffedProtocol::Http2PriorKnowledge, buffered))
}

fn apply_http2_settings(
    hpack: &mut http2::HpackDecoder,
    max_frame_size: &mut u32,
    payload: &[u8],
) -> Result<(), http2::Http2Error> {
    apply_http2_settings_with_fc(hpack, max_frame_size, None, payload)
}

fn apply_http2_settings_with_fc(
    hpack: &mut http2::HpackDecoder,
    max_frame_size: &mut u32,
    mut flow_control: Option<&mut http2::H2FlowControl>,
    payload: &[u8],
) -> Result<(), http2::Http2Error> {
    // SETTINGS payload is a sequence of (u16 id, u32 value) pairs.
    if payload.len() % 6 != 0 {
        return Err(http2::Http2Error::Protocol(
            "SETTINGS length must be a multiple of 6",
        ));
    }

    for chunk in payload.chunks_exact(6) {
        let id = u16::from_be_bytes([chunk[0], chunk[1]]);
        let value = u32::from_be_bytes([chunk[2], chunk[3], chunk[4], chunk[5]]);
        match id {
            0x1 => {
                // SETTINGS_HEADER_TABLE_SIZE
                hpack.set_dynamic_table_max_size(value as usize);
            }
            0x3 => {
                // SETTINGS_INITIAL_WINDOW_SIZE (RFC 7540 §6.5.2)
                // Must not exceed 2^31 - 1.
                if value > 0x7FFF_FFFF {
                    return Err(http2::Http2Error::Protocol(
                        "SETTINGS_INITIAL_WINDOW_SIZE exceeds maximum",
                    ));
                }
                if let Some(ref mut fc) = flow_control {
                    fc.set_initial_window_size(value);
                    // The peer's INITIAL_WINDOW_SIZE also controls the send
                    // window for streams the peer will receive data on (our
                    // response streams).
                    fc.set_peer_initial_window_size(value);
                }
            }
            0x5 => {
                // SETTINGS_MAX_FRAME_SIZE (RFC 7540: 16384..=16777215)
                if !(16_384..=16_777_215).contains(&value) {
                    return Err(http2::Http2Error::Protocol(
                        "invalid SETTINGS_MAX_FRAME_SIZE",
                    ));
                }
                *max_frame_size = value;
            }
            0x2 => {
                // SETTINGS_ENABLE_PUSH (RFC 7540 §6.5.2): must be 0 or 1.
                if value > 1 {
                    return Err(http2::Http2Error::Protocol(
                        "SETTINGS_ENABLE_PUSH must be 0 or 1",
                    ));
                }
                // We don't implement server push, so just validate.
            }
            0x4 => {
                // SETTINGS_MAX_CONCURRENT_STREAMS: informational for now.
                // We don't multiplex streams yet, so just accept the value.
            }
            0x6 => {
                // SETTINGS_MAX_HEADER_LIST_SIZE
                hpack.set_max_header_list_size(value as usize);
            }
            _ => {
                // Ignore unknown/unsupported settings (RFC 7540 §6.5.2).
            }
        }
    }
    Ok(())
}

fn validate_settings_frame(
    stream_id: u32,
    flags: u8,
    payload: &[u8],
) -> Result<bool, http2::Http2Error> {
    const FLAG_ACK: u8 = 0x1;
    if stream_id != 0 {
        return Err(http2::Http2Error::Protocol("SETTINGS must be on stream 0"));
    }

    let is_ack = (flags & FLAG_ACK) != 0;
    if is_ack && !payload.is_empty() {
        return Err(http2::Http2Error::Protocol(
            "SETTINGS ACK frame must have empty payload",
        ));
    }

    Ok(is_ack)
}

fn validate_window_update_payload(payload: &[u8]) -> Result<(), http2::Http2Error> {
    if payload.len() != 4 {
        return Err(http2::Http2Error::Protocol(
            "WINDOW_UPDATE payload must be 4 bytes",
        ));
    }

    let raw = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let increment = raw & 0x7FFF_FFFF;
    if increment == 0 {
        return Err(http2::Http2Error::Protocol(
            "WINDOW_UPDATE increment must be non-zero",
        ));
    }

    Ok(())
}

/// Maximum flow-control window size (2^31 - 1) per RFC 7540 §6.9.1.
const MAX_FLOW_CONTROL_WINDOW: i64 = 0x7FFF_FFFF;

/// Apply a connection-level WINDOW_UPDATE from the peer with overflow detection.
/// Returns `Err(FLOW_CONTROL_ERROR)` if the window would exceed 2^31-1.
fn apply_send_conn_window_update(
    fc: &mut http2::H2FlowControl,
    increment: u32,
) -> Result<(), http2::Http2Error> {
    let new_window = fc.send_conn_window() + i64::from(increment);
    if new_window > MAX_FLOW_CONTROL_WINDOW {
        return Err(http2::Http2Error::Protocol(
            "WINDOW_UPDATE causes flow-control window to exceed 2^31-1",
        ));
    }
    fc.peer_window_update_connection(increment);
    Ok(())
}

fn apply_peer_window_update_for_send(
    flow_control: &mut http2::H2FlowControl,
    stream_send_window: &mut i64,
    current_stream_id: u32,
    frame_stream_id: u32,
    payload: &[u8],
) -> Result<(), http2::Http2Error> {
    validate_window_update_payload(payload)?;

    let increment =
        u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) & 0x7FFF_FFFF;
    if frame_stream_id == 0 {
        apply_send_conn_window_update(flow_control, increment)?;
    } else if frame_stream_id == current_stream_id {
        let new_window = *stream_send_window + i64::from(increment);
        if new_window > MAX_FLOW_CONTROL_WINDOW {
            return Err(http2::Http2Error::Protocol(
                "WINDOW_UPDATE causes flow-control window to exceed 2^31-1",
            ));
        }
        *stream_send_window = new_window;
    }

    Ok(())
}

fn apply_peer_settings_for_send(
    flow_control: &mut http2::H2FlowControl,
    stream_send_window: &mut i64,
    payload: &[u8],
) -> Result<(), http2::Http2Error> {
    if payload.len() % 6 != 0 {
        return Err(http2::Http2Error::Protocol(
            "SETTINGS length must be a multiple of 6",
        ));
    }

    for chunk in payload.chunks_exact(6) {
        let id = u16::from_be_bytes([chunk[0], chunk[1]]);
        let value = u32::from_be_bytes([chunk[2], chunk[3], chunk[4], chunk[5]]);

        if id == 0x3 {
            // SETTINGS_INITIAL_WINDOW_SIZE applies to all existing stream send windows.
            if value > 0x7FFF_FFFF {
                return Err(http2::Http2Error::Protocol(
                    "SETTINGS_INITIAL_WINDOW_SIZE exceeds maximum",
                ));
            }
            let old = i64::from(flow_control.peer_initial_window_size());
            let new = i64::from(value);
            let delta = new - old;
            flow_control.set_peer_initial_window_size(value);
            let updated = *stream_send_window + delta;
            if updated > MAX_FLOW_CONTROL_WINDOW {
                return Err(http2::Http2Error::Protocol(
                    "SETTINGS_INITIAL_WINDOW_SIZE change causes stream window to exceed 2^31-1",
                ));
            }
            *stream_send_window = updated;
        }
    }

    Ok(())
}

/// Build the 4-byte WINDOW_UPDATE payload for a given increment.
fn window_update_payload(increment: u32) -> [u8; 4] {
    (increment & 0x7FFF_FFFF).to_be_bytes()
}

/// Send WINDOW_UPDATE frames for both connection and stream levels after
/// receiving DATA. Returns early on zero increments.
async fn send_window_updates(
    framed: &mut http2::FramedH2,
    conn_increment: u32,
    stream_id: u32,
    stream_increment: u32,
) -> Result<(), http2::Http2Error> {
    if conn_increment > 0 {
        let payload = window_update_payload(conn_increment);
        framed
            .write_frame(http2::FrameType::WindowUpdate, 0, 0, &payload)
            .await?;
    }
    if stream_increment > 0 {
        let payload = window_update_payload(stream_increment);
        framed
            .write_frame(http2::FrameType::WindowUpdate, 0, stream_id, &payload)
            .await?;
    }
    Ok(())
}

/// HTTP/2 error codes (RFC 7540 §7).
#[allow(dead_code)]
mod h2_error_code {
    pub const NO_ERROR: u32 = 0x0;
    pub const PROTOCOL_ERROR: u32 = 0x1;
    pub const FLOW_CONTROL_ERROR: u32 = 0x3;
    pub const SETTINGS_TIMEOUT: u32 = 0x4;
    pub const STREAM_CLOSED: u32 = 0x5;
    pub const FRAME_SIZE_ERROR: u32 = 0x6;
    pub const REFUSED_STREAM: u32 = 0x7;
    pub const CANCEL: u32 = 0x8;
    pub const ENHANCE_YOUR_CALM: u32 = 0xb;
}

/// Validate an incoming GOAWAY frame: payload must be at least 8 bytes (RFC 7540 §6.8).
fn validate_goaway_payload(payload: &[u8]) -> Result<(), http2::Http2Error> {
    if payload.len() < 8 {
        return Err(http2::Http2Error::Protocol(
            "GOAWAY payload must be at least 8 bytes",
        ));
    }
    Ok(())
}

/// Build the GOAWAY frame payload: last-stream-id (4 bytes) + error-code (4 bytes).
fn goaway_payload(last_stream_id: u32, error_code: u32) -> [u8; 8] {
    let mut buf = [0u8; 8];
    buf[..4].copy_from_slice(&(last_stream_id & 0x7FFF_FFFF).to_be_bytes());
    buf[4..].copy_from_slice(&error_code.to_be_bytes());
    buf
}

/// Send a GOAWAY frame on the connection. GOAWAY is always sent on stream 0.
async fn send_goaway(
    framed: &mut http2::FramedH2,
    last_stream_id: u32,
    error_code: u32,
) -> Result<(), http2::Http2Error> {
    let payload = goaway_payload(last_stream_id, error_code);
    framed
        .write_frame(http2::FrameType::Goaway, 0, 0, &payload)
        .await
}

fn validate_rst_stream_payload(stream_id: u32, payload: &[u8]) -> Result<(), http2::Http2Error> {
    if stream_id == 0 {
        return Err(http2::Http2Error::Protocol(
            "RST_STREAM must not be on stream 0",
        ));
    }
    if payload.len() != 4 {
        return Err(http2::Http2Error::Protocol(
            "RST_STREAM payload must be 4 bytes",
        ));
    }
    Ok(())
}

fn validate_priority_payload(stream_id: u32, payload: &[u8]) -> Result<(), http2::Http2Error> {
    if stream_id == 0 {
        return Err(http2::Http2Error::Protocol(
            "PRIORITY must not be on stream 0",
        ));
    }

    if payload.len() != 5 {
        return Err(http2::Http2Error::Protocol(
            "PRIORITY payload must be 5 bytes",
        ));
    }

    let dependency_raw = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let dependency_stream_id = dependency_raw & 0x7FFF_FFFF;
    if dependency_stream_id == stream_id {
        return Err(http2::Http2Error::Protocol(
            "PRIORITY stream dependency must not reference itself",
        ));
    }

    Ok(())
}

fn extract_header_block_fragment(
    flags: u8,
    payload: &[u8],
) -> Result<(bool, Vec<u8>), http2::Http2Error> {
    const FLAG_END_STREAM: u8 = 0x1;
    const FLAG_PADDED: u8 = 0x8;
    const FLAG_PRIORITY: u8 = 0x20;

    let end_stream = (flags & FLAG_END_STREAM) != 0;
    let mut idx = 0usize;

    let pad_len = if (flags & FLAG_PADDED) != 0 {
        if payload.is_empty() {
            return Err(http2::Http2Error::Protocol(
                "HEADERS PADDED set with empty payload",
            ));
        }
        let v = payload[0] as usize;
        idx += 1;
        v
    } else {
        0
    };

    if (flags & FLAG_PRIORITY) != 0 {
        // 5 bytes priority fields: dep(4) + weight(1)
        if payload.len().saturating_sub(idx) < 5 {
            return Err(http2::Http2Error::Protocol(
                "HEADERS PRIORITY set but too short",
            ));
        }
        idx += 5;
    }

    if payload.len() < idx {
        return Err(http2::Http2Error::Protocol("invalid HEADERS payload"));
    }
    let frag = &payload[idx..];
    if frag.len() < pad_len {
        return Err(http2::Http2Error::Protocol(
            "invalid HEADERS padding length",
        ));
    }
    let end = frag.len() - pad_len;
    Ok((end_stream, frag[..end].to_vec()))
}

fn extract_data_payload(flags: u8, payload: &[u8]) -> Result<(&[u8], bool), http2::Http2Error> {
    const FLAG_END_STREAM: u8 = 0x1;
    const FLAG_PADDED: u8 = 0x8;

    let end_stream = (flags & FLAG_END_STREAM) != 0;
    if (flags & FLAG_PADDED) == 0 {
        return Ok((payload, end_stream));
    }
    if payload.is_empty() {
        return Err(http2::Http2Error::Protocol(
            "DATA PADDED set with empty payload",
        ));
    }
    let pad_len = payload[0] as usize;
    let data = &payload[1..];
    if data.len() < pad_len {
        return Err(http2::Http2Error::Protocol("invalid DATA padding length"));
    }
    Ok((&data[..data.len() - pad_len], end_stream))
}

fn request_from_h2_headers(headers: http2::HeaderList) -> Result<Request, http2::Http2Error> {
    let mut method: Option<fastapi_core::Method> = None;
    let mut path: Option<String> = None;
    let mut authority: Option<Vec<u8>> = None;
    let mut saw_regular_headers = false;

    let mut req_headers: Vec<(String, Vec<u8>)> = Vec::new();

    for (name, value) in headers {
        if name.starts_with(b":") {
            if saw_regular_headers {
                return Err(http2::Http2Error::Protocol(
                    "pseudo-headers must appear before regular headers",
                ));
            }
            match name.as_slice() {
                b":method" => {
                    if method.is_some() {
                        return Err(http2::Http2Error::Protocol(
                            "duplicate :method pseudo-header",
                        ));
                    }
                    method = Some(
                        fastapi_core::Method::from_bytes(&value)
                            .ok_or(http2::Http2Error::Protocol("invalid :method"))?,
                    );
                }
                b":path" => {
                    if path.is_some() {
                        return Err(http2::Http2Error::Protocol("duplicate :path pseudo-header"));
                    }
                    let s = std::str::from_utf8(&value)
                        .map_err(|_| http2::Http2Error::Protocol("non-utf8 :path"))?;
                    path = Some(s.to_string());
                }
                b":authority" => {
                    if authority.is_some() {
                        return Err(http2::Http2Error::Protocol(
                            "duplicate :authority pseudo-header",
                        ));
                    }
                    authority = Some(value);
                }
                b":scheme" => {}
                _ => return Err(http2::Http2Error::Protocol("unknown pseudo-header")),
            }
            continue;
        }

        saw_regular_headers = true;
        let n = std::str::from_utf8(&name)
            .map_err(|_| http2::Http2Error::Protocol("non-utf8 header name"))?;
        req_headers.push((n.to_string(), value));
    }

    let method = method.ok_or(http2::Http2Error::Protocol("missing :method"))?;
    let raw_path = path.ok_or(http2::Http2Error::Protocol("missing :path"))?;
    let (path_only, query) = match raw_path.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (raw_path, None),
    };

    let mut req = Request::with_version(method, path_only, fastapi_core::HttpVersion::Http2);
    req.set_query(query);

    if let Some(auth) = authority {
        req.headers_mut().insert("host", auth);
    }

    for (n, v) in req_headers {
        req.headers_mut().insert(n, v);
    }

    Ok(req)
}

fn is_h2_forbidden_header_name(name: &str) -> bool {
    // RFC 7540: connection-specific headers are not permitted in HTTP/2.
    // We conservatively drop common hop-by-hop headers here.
    name.eq_ignore_ascii_case("connection")
        || name.eq_ignore_ascii_case("keep-alive")
        || name.eq_ignore_ascii_case("proxy-connection")
        || name.eq_ignore_ascii_case("transfer-encoding")
        || name.eq_ignore_ascii_case("upgrade")
        || name.eq_ignore_ascii_case("te")
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
    use std::future::Future;

    fn block_on<F: Future>(f: F) -> F::Output {
        let rt = asupersync::runtime::RuntimeBuilder::current_thread()
            .build()
            .expect("test runtime must build");
        rt.block_on(f)
    }

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

    #[test]
    fn window_update_payload_validation_accepts_non_zero_increment() {
        let payload = 1u32.to_be_bytes();
        assert!(validate_window_update_payload(&payload).is_ok());
    }

    #[test]
    fn window_update_payload_validation_rejects_bad_length() {
        let err = validate_window_update_payload(&[0, 0, 0]).unwrap_err();
        assert!(
            err.to_string()
                .contains("WINDOW_UPDATE payload must be 4 bytes")
        );
    }

    #[test]
    fn window_update_payload_validation_rejects_zero_increment() {
        let payload = 0u32.to_be_bytes();
        let err = validate_window_update_payload(&payload).unwrap_err();
        assert!(
            err.to_string()
                .contains("WINDOW_UPDATE increment must be non-zero")
        );
    }

    #[test]
    fn settings_frame_validation_accepts_non_ack_payload() {
        let payload = [0u8; 6];
        let is_ack = validate_settings_frame(0, 0, &payload).unwrap();
        assert!(!is_ack);
    }

    #[test]
    fn settings_frame_validation_accepts_empty_ack_payload() {
        let is_ack = validate_settings_frame(0, 0x1, &[]).unwrap();
        assert!(is_ack);
    }

    #[test]
    fn settings_frame_validation_rejects_non_zero_stream() {
        let err = validate_settings_frame(1, 0, &[]).unwrap_err();
        assert!(err.to_string().contains("SETTINGS must be on stream 0"));
    }

    #[test]
    fn settings_frame_validation_rejects_non_empty_ack_payload() {
        let err = validate_settings_frame(0, 0x1, &[0, 0, 0, 0, 0, 0]).unwrap_err();
        assert!(
            err.to_string()
                .contains("SETTINGS ACK frame must have empty payload")
        );
    }

    #[test]
    fn settings_enable_push_accepts_zero() {
        // SETTINGS_ENABLE_PUSH (id=0x2), value=0.
        let payload = [0x00, 0x02, 0x00, 0x00, 0x00, 0x00];
        let mut hpack = http2::HpackDecoder::new();
        let mut max_frame_size = 16384u32;
        assert!(apply_http2_settings(&mut hpack, &mut max_frame_size, &payload).is_ok());
    }

    #[test]
    fn settings_enable_push_accepts_one() {
        let payload = [0x00, 0x02, 0x00, 0x00, 0x00, 0x01];
        let mut hpack = http2::HpackDecoder::new();
        let mut max_frame_size = 16384u32;
        assert!(apply_http2_settings(&mut hpack, &mut max_frame_size, &payload).is_ok());
    }

    #[test]
    fn settings_enable_push_rejects_invalid_value() {
        let payload = [0x00, 0x02, 0x00, 0x00, 0x00, 0x02];
        let mut hpack = http2::HpackDecoder::new();
        let mut max_frame_size = 16384u32;
        let err = apply_http2_settings(&mut hpack, &mut max_frame_size, &payload).unwrap_err();
        assert!(
            err.to_string()
                .contains("SETTINGS_ENABLE_PUSH must be 0 or 1")
        );
    }

    #[test]
    fn rst_stream_payload_validation_accepts_valid_payload() {
        let payload = 8u32.to_be_bytes();
        assert!(validate_rst_stream_payload(1, &payload).is_ok());
    }

    #[test]
    fn rst_stream_payload_validation_rejects_stream_zero() {
        let payload = 8u32.to_be_bytes();
        let err = validate_rst_stream_payload(0, &payload).unwrap_err();
        assert!(
            err.to_string()
                .contains("RST_STREAM must not be on stream 0")
        );
    }

    #[test]
    fn rst_stream_payload_validation_rejects_bad_length() {
        let err = validate_rst_stream_payload(1, &[0, 0, 0]).unwrap_err();
        assert!(
            err.to_string()
                .contains("RST_STREAM payload must be 4 bytes")
        );
    }

    #[test]
    fn priority_payload_validation_accepts_valid_priority() {
        let payload = [0, 0, 0, 0, 16];
        assert!(validate_priority_payload(1, &payload).is_ok());
    }

    #[test]
    fn priority_payload_validation_rejects_stream_zero() {
        let payload = [0, 0, 0, 0, 16];
        let err = validate_priority_payload(0, &payload).unwrap_err();
        assert!(err.to_string().contains("PRIORITY must not be on stream 0"));
    }

    #[test]
    fn priority_payload_validation_rejects_bad_length() {
        let err = validate_priority_payload(1, &[0, 0, 0, 0]).unwrap_err();
        assert!(err.to_string().contains("PRIORITY payload must be 5 bytes"));
    }

    #[test]
    fn priority_payload_validation_rejects_self_dependency() {
        let payload = 1u32.to_be_bytes();
        let mut with_weight = [0u8; 5];
        with_weight[..4].copy_from_slice(&payload);
        with_weight[4] = 16;
        let err = validate_priority_payload(1, &with_weight).unwrap_err();
        assert!(
            err.to_string()
                .contains("PRIORITY stream dependency must not reference itself")
        );
    }

    #[test]
    fn goaway_payload_validation_accepts_valid_payload() {
        let payload = goaway_payload(0, 0);
        assert!(validate_goaway_payload(&payload).is_ok());
    }

    #[test]
    fn goaway_payload_validation_accepts_payload_with_debug_data() {
        let mut payload = Vec::from(goaway_payload(1, 0).as_slice());
        payload.extend_from_slice(b"debug info");
        assert!(validate_goaway_payload(&payload).is_ok());
    }

    #[test]
    fn goaway_payload_validation_rejects_short_payload() {
        let err = validate_goaway_payload(&[0, 0, 0]).unwrap_err();
        assert!(
            err.to_string()
                .contains("GOAWAY payload must be at least 8 bytes")
        );
    }

    #[test]
    fn goaway_payload_validation_rejects_empty() {
        let err = validate_goaway_payload(&[]).unwrap_err();
        assert!(
            err.to_string()
                .contains("GOAWAY payload must be at least 8 bytes")
        );
    }

    #[test]
    fn h2_send_window_update_ignores_other_streams() {
        let mut flow_control = http2::H2FlowControl::new();
        let mut stream_window = 123i64;
        let payload = 7u32.to_be_bytes();

        apply_peer_window_update_for_send(&mut flow_control, &mut stream_window, 3, 5, &payload)
            .expect("window update on different stream should be ignored");

        assert_eq!(stream_window, 123);
    }

    #[test]
    fn h2_send_window_update_applies_connection_and_current_stream() {
        let mut flow_control = http2::H2FlowControl::new();
        let mut stream_window = 10i64;

        let conn_before = flow_control.send_conn_window();
        let conn_payload = 11u32.to_be_bytes();
        apply_peer_window_update_for_send(
            &mut flow_control,
            &mut stream_window,
            9,
            0,
            &conn_payload,
        )
        .expect("connection window update should be applied");
        assert_eq!(flow_control.send_conn_window(), conn_before + 11);
        assert_eq!(stream_window, 10);

        let stream_payload = 13u32.to_be_bytes();
        apply_peer_window_update_for_send(
            &mut flow_control,
            &mut stream_window,
            9,
            9,
            &stream_payload,
        )
        .expect("stream window update should be applied to current stream");
        assert_eq!(stream_window, 23);
    }

    #[test]
    fn h2_send_settings_updates_current_stream_window_delta() {
        let mut flow_control = http2::H2FlowControl::new();
        let mut stream_window = 50i64;

        let payload = [0x00, 0x03, 0x00, 0x01, 0x11, 0x70]; // id=3, value=70000
        apply_peer_settings_for_send(&mut flow_control, &mut stream_window, &payload)
            .expect("valid SETTINGS_INITIAL_WINDOW_SIZE should apply");

        assert_eq!(flow_control.peer_initial_window_size(), 70_000);
        assert_eq!(stream_window, 4_515); // 50 + (70000 - 65535)
    }

    #[test]
    fn h2_send_settings_rejects_invalid_payload_len() {
        let mut flow_control = http2::H2FlowControl::new();
        let mut stream_window = 0i64;
        let err = apply_peer_settings_for_send(&mut flow_control, &mut stream_window, &[0, 1, 2])
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("SETTINGS length must be a multiple of 6")
        );
    }

    #[test]
    fn h2_send_settings_rejects_initial_window_too_large() {
        let mut flow_control = http2::H2FlowControl::new();
        let mut stream_window = 0i64;
        let payload = [0x00, 0x03, 0x80, 0x00, 0x00, 0x00]; // id=3, value=2^31
        let err = apply_peer_settings_for_send(&mut flow_control, &mut stream_window, &payload)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("SETTINGS_INITIAL_WINDOW_SIZE exceeds maximum")
        );
    }

    #[test]
    fn h2_send_settings_window_delta_overflow_is_flow_control_error() {
        let mut flow_control = http2::H2FlowControl::new();
        // Start with a stream window near the maximum.
        let mut stream_window: i64 = 0x7FFF_FFFF - 10;
        // Increase INITIAL_WINDOW_SIZE by more than 10 from default (65535).
        // Delta = new - old = 0x7FFF_FFFF - 65535 = 2147418112
        // New stream_window = (2^31-1 - 10) + 2147418112 > 2^31-1
        let new_initial: u32 = 0x7FFF_FFFF;
        let payload = [
            0x00,
            0x03,
            new_initial.to_be_bytes()[0],
            new_initial.to_be_bytes()[1],
            new_initial.to_be_bytes()[2],
            new_initial.to_be_bytes()[3],
        ];
        let err = apply_peer_settings_for_send(&mut flow_control, &mut stream_window, &payload)
            .unwrap_err();
        assert!(err.to_string().contains("stream window to exceed 2^31-1"));
    }

    #[test]
    fn request_from_h2_headers_rejects_unknown_pseudo_header() {
        let headers: http2::HeaderList = vec![
            (b":method".to_vec(), b"GET".to_vec()),
            (b":path".to_vec(), b"/".to_vec()),
            (b":weird".to_vec(), b"value".to_vec()),
        ];
        let err = request_from_h2_headers(headers).unwrap_err();
        assert!(err.to_string().contains("unknown pseudo-header"));
    }

    #[test]
    fn request_from_h2_headers_rejects_pseudo_after_regular_header() {
        let headers: http2::HeaderList = vec![
            (b":method".to_vec(), b"GET".to_vec()),
            (b":path".to_vec(), b"/".to_vec()),
            (b"x-test".to_vec(), b"ok".to_vec()),
            (b":authority".to_vec(), b"example.com".to_vec()),
        ];
        let err = request_from_h2_headers(headers).unwrap_err();
        assert!(
            err.to_string()
                .contains("pseudo-headers must appear before regular headers")
        );
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
    // WebSocket upgrade request detection tests
    // ========================================================================

    #[test]
    fn websocket_upgrade_detection_accepts_token_lists_case_insensitive() {
        let mut request = Request::new(fastapi_core::Method::Get, "/ws");
        request
            .headers_mut()
            .insert("Upgrade".to_string(), b"h2c, WebSocket".to_vec());
        request
            .headers_mut()
            .insert("Connection".to_string(), b"keep-alive, UPGRADE".to_vec());

        assert!(is_websocket_upgrade_request(&request));
    }

    #[test]
    fn websocket_upgrade_detection_rejects_missing_connection_upgrade_token() {
        let mut request = Request::new(fastapi_core::Method::Get, "/ws");
        request
            .headers_mut()
            .insert("Upgrade".to_string(), b"websocket".to_vec());
        request
            .headers_mut()
            .insert("Connection".to_string(), b"keep-alive".to_vec());

        assert!(!is_websocket_upgrade_request(&request));
    }

    #[test]
    fn websocket_upgrade_detection_rejects_non_get_method() {
        let mut request = Request::new(fastapi_core::Method::Post, "/ws");
        request
            .headers_mut()
            .insert("Upgrade".to_string(), b"websocket".to_vec());
        request
            .headers_mut()
            .insert("Connection".to_string(), b"upgrade".to_vec());

        assert!(!is_websocket_upgrade_request(&request));
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

    #[test]
    fn wait_for_drain_returns_true_when_no_connections() {
        block_on(async {
            let server = TcpServer::default();
            assert_eq!(server.current_connections(), 0);
            let result = server
                .wait_for_drain(Duration::from_millis(100), Some(Duration::from_millis(1)))
                .await;
            assert!(result);
        });
    }

    #[test]
    fn wait_for_drain_timeout_with_connections() {
        block_on(async {
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
        });
    }

    #[test]
    fn drain_returns_zero_when_no_connections() {
        block_on(async {
            let server = TcpServer::new(
                ServerConfig::new("127.0.0.1:8080").with_drain_timeout(Duration::from_millis(100)),
            );
            assert_eq!(server.current_connections(), 0);
            let remaining = server.drain().await;
            assert_eq!(remaining, 0);
            assert!(server.is_draining());
        });
    }

    #[test]
    fn drain_returns_count_when_connections_remain() {
        block_on(async {
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
        });
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
    /// fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let app = App::builder()
    ///         .get("/health", |_, _| async { Response::ok() })
    ///         .build();
    ///
    ///     let rt = asupersync::runtime::RuntimeBuilder::current_thread().build()?;
    ///     rt.block_on(async {
    ///         app.serve("0.0.0.0:8080").await?;
    ///         Ok::<(), fastapi_http::ServeError>(())
    ///     })?;
    ///
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

            // Wrap app in Arc for sharing.
            let app = Arc::new(self);

            // Create a root Cx for the server
            let cx = Cx::for_testing();

            // Print startup banner
            let bind_addr = &server.config().bind_addr;
            println!("🚀 Server starting on http://{bind_addr}");

            // Run the server with App-aware routing (enables protocol upgrades like WebSocket).
            let result = server.serve_app(&cx, Arc::clone(&app)).await;

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
