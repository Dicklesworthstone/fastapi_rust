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

use crate::parser::{ParseError, ParseLimits, Parser};
use crate::response::{ResponseWrite, ResponseWriter};
use asupersync::net::{TcpListener, TcpStream};
use asupersync::{Budget, Cx, Time};
use fastapi_core::{Request, RequestContext, Response};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Default request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Default read buffer size in bytes.
pub const DEFAULT_READ_BUFFER_SIZE: usize = 8192;

/// Default maximum connections (0 = unlimited).
pub const DEFAULT_MAX_CONNECTIONS: usize = 0;

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
    /// Enable TCP_NODELAY.
    pub tcp_nodelay: bool,
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
            tcp_nodelay: true,
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

    /// Enables or disables TCP_NODELAY.
    #[must_use]
    pub fn with_tcp_nodelay(mut self, enabled: bool) -> Self {
        self.tcp_nodelay = enabled;
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
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Parse(e) => write!(f, "Parse error: {e}"),
            Self::Shutdown => write!(f, "Server shutdown"),
            Self::ConnectionLimitReached => write!(f, "Connection limit reached"),
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

/// TCP server with asupersync integration.
///
/// This server manages the lifecycle of connections and requests using
/// asupersync's structured concurrency primitives. Each connection runs
/// in its own region, and each request gets its own task with a budget.
#[derive(Debug)]
pub struct TcpServer {
    config: ServerConfig,
    request_counter: AtomicU64,
}

impl TcpServer {
    /// Creates a new TCP server with the given configuration.
    #[must_use]
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            request_counter: AtomicU64::new(0),
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
        H: Fn(RequestContext, Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let listener = TcpListener::bind(&self.config.bind_addr).await?;
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
        H: Fn(RequestContext, Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.accept_loop(cx, listener, handler).await
    }

    /// The main accept loop.
    async fn accept_loop<H, Fut>(
        &self,
        cx: &Cx,
        listener: TcpListener,
        handler: H,
    ) -> Result<(), ServerError>
    where
        H: Fn(RequestContext, Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let handler = Arc::new(handler);

        loop {
            // Check for cancellation at each iteration.
            if cx.is_cancel_requested() {
                cx.trace("Server shutdown requested");
                return Ok(());
            }

            // Accept a connection.
            let (stream, peer_addr) = match listener.accept().await {
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

            // Configure the connection.
            if self.config.tcp_nodelay {
                let _ = stream.set_nodelay(true);
            }

            cx.trace(&format!("Accepted connection from {peer_addr}"));

            // Handle the connection.
            // In a full implementation, we would spawn this in a sub-region.
            // For now, we handle it inline (blocking accept loop).
            //
            // TODO: When asupersync has spawn support, use:
            // scope.spawn(cx, |conn_cx| {
            //     self.handle_connection(conn_cx, stream, peer_addr, handler.clone())
            // });
            let request_id = self.next_request_id();
            let _request_budget = Budget::new().with_deadline(self.config.request_timeout);

            // Create a RequestContext for this request.
            // In the full implementation, the Cx would be derived from the connection region.
            let request_cx = Cx::for_testing(); // Placeholder until spawn works
            let ctx = RequestContext::new(request_cx, request_id);

            // Handle the request.
            if let Err(e) = self
                .handle_connection(&ctx, stream, peer_addr, &*handler)
                .await
            {
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
        _ctx: &RequestContext,
        _stream: TcpStream,
        _peer_addr: SocketAddr,
        _handler: &H,
    ) -> Result<(), ServerError>
    where
        H: Fn(RequestContext, Request) -> Fut + Send + Sync,
        Fut: Future<Output = Response> + Send,
    {
        // TODO: Implement once asupersync TcpStream has read/write methods.
        // This is scaffolding for the TCP server integration (Phase 1).
        //
        // The implementation needs:
        // - AsyncReadExt::read() on TcpStream
        // - AsyncWriteExt::write_all() on TcpStream
        // - ResponseWrite to have an as_bytes() method or similar
        //
        // For now, this is a stub to allow compilation.
        todo!("TCP connection handling pending asupersync I/O completion")
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
            .with_tcp_nodelay(false);

        assert_eq!(config.bind_addr, "0.0.0.0:3000");
        assert_eq!(config.request_timeout, Time::from_secs(60));
        assert_eq!(config.max_connections, 1000);
        assert!(!config.tcp_nodelay);
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
}
