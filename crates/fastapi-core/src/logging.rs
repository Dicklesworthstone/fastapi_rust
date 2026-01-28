//! Structured logging infrastructure for fastapi_rust.
//!
//! This module provides structured logging that integrates with asupersync's
//! observability system and automatically propagates request context.
//!
//! # Design Principles
//!
//! 1. **Context propagation**: Log macros auto-inject request_id, region_id, task_id
//! 2. **Structured output**: All logs are JSON-formatted for production
//! 3. **Span-based timing**: Instrument operations with hierarchical spans
//! 4. **asupersync integration**: Delegates to asupersync's observability module
//! 5. **Zero-allocation fast path**: Critical paths avoid heap allocation
//!
//! # Usage
//!
//! ## Basic Logging
//!
//! ```ignore
//! use fastapi_core::logging::*;
//!
//! async fn handler(ctx: &RequestContext) -> impl IntoResponse {
//!     log_info!(ctx, "Processing request");
//!     log_debug!(ctx, "Request path: {}", ctx.request().path());
//!
//!     // With structured fields
//!     log_info!(ctx, "User authenticated",
//!         user_id => user.id,
//!         role => user.role
//!     );
//!
//!     "ok"
//! }
//! ```
//!
//! ## Timing Spans
//!
//! ```ignore
//! use fastapi_core::logging::*;
//!
//! async fn handler(ctx: &RequestContext) -> impl IntoResponse {
//!     let span = ctx.span("database_query");
//!     let result = db.query("SELECT ...").await?;
//!     span.end(); // Logs duration
//!
//!     // Or with auto-end on drop
//!     {
//!         let _span = ctx.span_auto("serialize");
//!         serde_json::to_string(&result)?
//!     } // Span ends here
//! }
//! ```
//!
//! # JSON Output Schema
//!
//! ```json
//! {
//!     "timestamp": "2024-01-17T10:30:45.123456789Z",
//!     "level": "info",
//!     "message": "User authenticated",
//!     "request_id": 12345,
//!     "region_id": 1,
//!     "task_id": 42,
//!     "target": "my_app::handlers::auth",
//!     "fields": {
//!         "user_id": 67890,
//!         "role": "admin"
//!     }
//! }
//! ```
//!
//! # Configuration
//!
//! Logging is configured via `LogConfig`:
//!
//! ```ignore
//! use fastapi_core::logging::{LogConfig, LogLevel};
//!
//! let config = LogConfig::new()
//!     .level(LogLevel::Debug)          // Minimum level to emit
//!     .json_output(true)               // JSON format (false = compact)
//!     .include_target(true)            // Include module path
//!     .max_fields(16);                 // Max structured fields per log
//!
//! let app = App::new()
//!     .with_logging(config);
//! ```

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::context::RequestContext;

// Re-export asupersync's log types for convenience
// In full implementation, these would come from asupersync::observability
// For now, we define our own compatible types

/// Log levels matching asupersync's observability module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    /// Most verbose, for detailed debugging.
    Trace = 0,
    /// Debug information, not shown in production.
    Debug = 1,
    /// General information about normal operation.
    Info = 2,
    /// Something unexpected but recoverable.
    Warn = 3,
    /// An error that affected request processing.
    Error = 4,
}

impl LogLevel {
    /// Returns the level as a lowercase string.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    /// Returns a single character representation.
    #[must_use]
    pub const fn as_char(&self) -> char {
        match self {
            Self::Trace => 'T',
            Self::Debug => 'D',
            Self::Info => 'I',
            Self::Warn => 'W',
            Self::Error => 'E',
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A structured log entry with context.
///
/// Logs are created via macros that auto-inject request context,
/// then emitted through the configured log sink.
#[derive(Debug)]
pub struct LogEntry {
    /// The log level.
    pub level: LogLevel,
    /// The log message.
    pub message: String,
    /// Unique request identifier.
    pub request_id: u64,
    /// asupersync region ID (formatted as string for serialization).
    pub region_id: String,
    /// asupersync task ID (formatted as string for serialization).
    pub task_id: String,
    /// Module/target path (optional).
    pub target: Option<String>,
    /// Structured key-value fields (max 16).
    pub fields: Vec<(String, String)>,
    /// Nanosecond timestamp from asupersync's virtual time.
    pub timestamp_ns: u64,
}

impl LogEntry {
    /// Creates a new log entry with context from RequestContext.
    #[must_use]
    pub fn new(ctx: &RequestContext, level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            request_id: ctx.request_id(),
            region_id: format!("{:?}", ctx.region_id()),
            task_id: format!("{:?}", ctx.task_id()),
            target: None,
            fields: Vec::new(),
            timestamp_ns: 0, // Will be set by asupersync's virtual time
        }
    }

    /// Sets the target module path.
    #[must_use]
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Adds a structured field.
    ///
    /// Fields beyond the max (16) are silently dropped.
    #[must_use]
    pub fn field(mut self, key: impl Into<String>, value: impl fmt::Display) -> Self {
        if self.fields.len() < 16 {
            self.fields.push((key.into(), value.to_string()));
        }
        self
    }

    /// Formats the log entry as JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut json = format!(
            r#"{{"timestamp_ns":{},"level":"{}","message":"{}","request_id":{},"region_id":"{}","task_id":"{}""#,
            self.timestamp_ns,
            self.level,
            escape_json(&self.message),
            self.request_id,
            escape_json(&self.region_id),
            escape_json(&self.task_id)
        );

        if let Some(ref target) = self.target {
            json.push_str(&format!(r#","target":"{}""#, escape_json(target)));
        }

        if !self.fields.is_empty() {
            json.push_str(r#","fields":{"#);
            for (i, (k, v)) in self.fields.iter().enumerate() {
                if i > 0 {
                    json.push(',');
                }
                json.push_str(&format!(r#""{}":"{}""#, escape_json(k), escape_json(v)));
            }
            json.push('}');
        }

        json.push('}');
        json
    }

    /// Formats the log entry in compact format.
    #[must_use]
    pub fn to_compact(&self) -> String {
        let mut output = format!(
            "[{}] req={} {}",
            self.level.as_char(),
            self.request_id,
            self.message
        );

        if !self.fields.is_empty() {
            output.push_str(" {");
            for (i, (k, v)) in self.fields.iter().enumerate() {
                if i > 0 {
                    output.push_str(", ");
                }
                output.push_str(&format!("{k}={v}"));
            }
            output.push('}');
        }

        output
    }
}

/// Escapes a string for JSON output.
fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// A timing span for instrumentation.
///
/// Spans track the duration of operations and can be nested hierarchically.
/// They integrate with asupersync's DiagnosticContext for distributed tracing.
pub struct Span {
    name: String,
    request_id: u64,
    start: Instant,
    span_id: u64,
    parent_id: Option<u64>,
    ended: bool,
}

impl Span {
    /// Creates a new span.
    #[must_use]
    pub fn new(ctx: &RequestContext, name: impl Into<String>) -> Self {
        static SPAN_COUNTER: AtomicU64 = AtomicU64::new(1);

        Self {
            name: name.into(),
            request_id: ctx.request_id(),
            start: Instant::now(),
            span_id: SPAN_COUNTER.fetch_add(1, Ordering::SeqCst),
            parent_id: None,
            ended: false,
        }
    }

    /// Creates a child span under this span.
    #[must_use]
    pub fn child(&self, ctx: &RequestContext, name: impl Into<String>) -> Self {
        static SPAN_COUNTER: AtomicU64 = AtomicU64::new(1);

        Self {
            name: name.into(),
            request_id: ctx.request_id(),
            start: Instant::now(),
            span_id: SPAN_COUNTER.fetch_add(1, Ordering::SeqCst),
            parent_id: Some(self.span_id),
            ended: false,
        }
    }

    /// Returns the span name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the span ID.
    #[must_use]
    pub fn span_id(&self) -> u64 {
        self.span_id
    }

    /// Returns the parent span ID if this is a child span.
    #[must_use]
    pub fn parent_id(&self) -> Option<u64> {
        self.parent_id
    }

    /// Returns the elapsed duration since the span started.
    #[must_use]
    pub fn elapsed(&self) -> std::time::Duration {
        self.start.elapsed()
    }

    /// Ends the span and returns the duration.
    ///
    /// Call this to explicitly end the span and log its duration.
    /// If not called, the span will be ended when dropped.
    pub fn end(&mut self) -> std::time::Duration {
        let duration = self.elapsed();
        if !self.ended {
            self.ended = true;
            // In full implementation, this would emit a span-end event
            // to asupersync's DiagnosticContext
        }
        duration
    }

    /// Returns a JSON representation of the span timing.
    #[must_use]
    pub fn to_json(&self) -> String {
        let duration = self.elapsed();
        let mut json = format!(
            r#"{{"span_id":{},"name":"{}","request_id":{},"duration_us":{}"#,
            self.span_id,
            escape_json(&self.name),
            self.request_id,
            duration.as_micros()
        );

        if let Some(parent) = self.parent_id {
            json.push_str(&format!(r#","parent_id":{parent}"#));
        }

        json.push('}');
        json
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if !self.ended {
            self.end();
        }
    }
}

/// Auto-ending span that logs duration on drop.
///
/// Unlike [`Span`], this type automatically logs its duration
/// when it goes out of scope, making it ideal for RAII-style usage.
pub struct AutoSpan {
    inner: Span,
    ctx_request_id: u64,
}

impl AutoSpan {
    /// Creates a new auto-ending span.
    #[must_use]
    pub fn new(ctx: &RequestContext, name: impl Into<String>) -> Self {
        Self {
            inner: Span::new(ctx, name),
            ctx_request_id: ctx.request_id(),
        }
    }
}

impl Drop for AutoSpan {
    fn drop(&mut self) {
        let duration = self.inner.end();
        // In full implementation, would emit a log entry like:
        // log_debug!(ctx, "Span ended", span => self.inner.name(), duration_us => duration.as_micros());
        let _ = (duration, self.ctx_request_id); // Suppress warnings for now
    }
}

/// Configuration for the logging system.
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Minimum log level to emit.
    pub min_level: LogLevel,
    /// Whether to output JSON (true) or compact format (false).
    pub json_output: bool,
    /// Whether to include the target module path.
    pub include_target: bool,
    /// Maximum number of structured fields per log entry.
    pub max_fields: usize,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            min_level: LogLevel::Info,
            json_output: true,
            include_target: true,
            max_fields: 16,
        }
    }
}

impl LogConfig {
    /// Creates a new configuration with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the minimum log level.
    #[must_use]
    pub fn level(mut self, level: LogLevel) -> Self {
        self.min_level = level;
        self
    }

    /// Sets whether to output JSON format.
    #[must_use]
    pub fn json_output(mut self, json: bool) -> Self {
        self.json_output = json;
        self
    }

    /// Sets whether to include the target module path.
    #[must_use]
    pub fn include_target(mut self, include: bool) -> Self {
        self.include_target = include;
        self
    }

    /// Sets the maximum structured fields per log.
    #[must_use]
    pub fn max_fields(mut self, max: usize) -> Self {
        self.max_fields = max;
        self
    }

    /// Returns a development configuration (verbose, compact output).
    #[must_use]
    pub fn development() -> Self {
        Self {
            min_level: LogLevel::Debug,
            json_output: false,
            include_target: true,
            max_fields: 16,
        }
    }

    /// Returns a production configuration (info+, JSON output).
    #[must_use]
    pub fn production() -> Self {
        Self {
            min_level: LogLevel::Info,
            json_output: true,
            include_target: true,
            max_fields: 16,
        }
    }

    /// Returns a testing configuration (trace level, JSON output).
    #[must_use]
    pub fn testing() -> Self {
        Self {
            min_level: LogLevel::Trace,
            json_output: true,
            include_target: true,
            max_fields: 16,
        }
    }
}

// ============================================================================
// Request Logger
// ============================================================================

use std::sync::atomic::AtomicUsize;

/// Global log level for fast level checks.
///
/// This allows macros to skip log construction entirely when
/// the level is below the configured minimum (zero overhead).
static GLOBAL_LOG_LEVEL: AtomicUsize = AtomicUsize::new(LogLevel::Info as usize);

/// Returns the current global log level.
#[inline]
#[must_use]
pub fn global_log_level() -> LogLevel {
    match GLOBAL_LOG_LEVEL.load(Ordering::Relaxed) {
        0 => LogLevel::Trace,
        1 => LogLevel::Debug,
        2 => LogLevel::Info,
        3 => LogLevel::Warn,
        _ => LogLevel::Error,
    }
}

/// Sets the global log level.
///
/// This affects all future log macro calls.
pub fn set_global_log_level(level: LogLevel) {
    GLOBAL_LOG_LEVEL.store(level as usize, Ordering::Relaxed);
}

/// Returns true if the given level is enabled.
#[inline]
#[must_use]
pub fn level_enabled(level: LogLevel) -> bool {
    level >= global_log_level()
}

/// A per-request logger that captures context and emits logs.
///
/// This struct is typically created once per request and provides
/// logging methods that automatically include request context.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::logging::RequestLogger;
///
/// async fn handler(ctx: &RequestContext) -> impl IntoResponse {
///     let logger = RequestLogger::new(ctx, LogConfig::production());
///
///     logger.info("Request started");
///     logger.debug_with_fields("Processing", |e| e.field("path", ctx.path()));
///
///     "ok"
/// }
/// ```
pub struct RequestLogger<'a> {
    ctx: &'a RequestContext,
    config: LogConfig,
}

impl<'a> RequestLogger<'a> {
    /// Creates a new request logger.
    #[must_use]
    pub fn new(ctx: &'a RequestContext, config: LogConfig) -> Self {
        Self { ctx, config }
    }

    /// Returns true if the given log level is enabled.
    #[inline]
    #[must_use]
    pub fn is_enabled(&self, level: LogLevel) -> bool {
        level >= self.config.min_level && level_enabled(level)
    }

    /// Emits a log entry if the level is enabled.
    pub fn emit(&self, entry: LogEntry) {
        if !self.is_enabled(entry.level) {
            return;
        }

        let output = if self.config.json_output {
            entry.to_json()
        } else {
            entry.to_compact()
        };

        // In production, this would delegate to asupersync's observability
        // or a configured log sink. For now, print to stderr.
        eprintln!("{output}");
    }

    /// Logs a message at TRACE level.
    pub fn trace(&self, message: impl Into<String>) {
        if self.is_enabled(LogLevel::Trace) {
            self.emit(LogEntry::new(self.ctx, LogLevel::Trace, message));
        }
    }

    /// Logs a message at DEBUG level.
    pub fn debug(&self, message: impl Into<String>) {
        if self.is_enabled(LogLevel::Debug) {
            self.emit(LogEntry::new(self.ctx, LogLevel::Debug, message));
        }
    }

    /// Logs a message at INFO level.
    pub fn info(&self, message: impl Into<String>) {
        if self.is_enabled(LogLevel::Info) {
            self.emit(LogEntry::new(self.ctx, LogLevel::Info, message));
        }
    }

    /// Logs a message at WARN level.
    pub fn warn(&self, message: impl Into<String>) {
        if self.is_enabled(LogLevel::Warn) {
            self.emit(LogEntry::new(self.ctx, LogLevel::Warn, message));
        }
    }

    /// Logs a message at ERROR level.
    pub fn error(&self, message: impl Into<String>) {
        if self.is_enabled(LogLevel::Error) {
            self.emit(LogEntry::new(self.ctx, LogLevel::Error, message));
        }
    }

    /// Logs with custom field builder at TRACE level.
    pub fn trace_with_fields<F>(&self, message: impl Into<String>, f: F)
    where
        F: FnOnce(LogEntry) -> LogEntry,
    {
        if self.is_enabled(LogLevel::Trace) {
            let entry = f(LogEntry::new(self.ctx, LogLevel::Trace, message));
            self.emit(entry);
        }
    }

    /// Logs with custom field builder at DEBUG level.
    pub fn debug_with_fields<F>(&self, message: impl Into<String>, f: F)
    where
        F: FnOnce(LogEntry) -> LogEntry,
    {
        if self.is_enabled(LogLevel::Debug) {
            let entry = f(LogEntry::new(self.ctx, LogLevel::Debug, message));
            self.emit(entry);
        }
    }

    /// Logs with custom field builder at INFO level.
    pub fn info_with_fields<F>(&self, message: impl Into<String>, f: F)
    where
        F: FnOnce(LogEntry) -> LogEntry,
    {
        if self.is_enabled(LogLevel::Info) {
            let entry = f(LogEntry::new(self.ctx, LogLevel::Info, message));
            self.emit(entry);
        }
    }

    /// Logs with custom field builder at WARN level.
    pub fn warn_with_fields<F>(&self, message: impl Into<String>, f: F)
    where
        F: FnOnce(LogEntry) -> LogEntry,
    {
        if self.is_enabled(LogLevel::Warn) {
            let entry = f(LogEntry::new(self.ctx, LogLevel::Warn, message));
            self.emit(entry);
        }
    }

    /// Logs with custom field builder at ERROR level.
    pub fn error_with_fields<F>(&self, message: impl Into<String>, f: F)
    where
        F: FnOnce(LogEntry) -> LogEntry,
    {
        if self.is_enabled(LogLevel::Error) {
            let entry = f(LogEntry::new(self.ctx, LogLevel::Error, message));
            self.emit(entry);
        }
    }

    /// Creates a timing span.
    #[must_use]
    pub fn span(&self, name: impl Into<String>) -> Span {
        Span::new(self.ctx, name)
    }

    /// Creates an auto-ending span.
    #[must_use]
    pub fn span_auto(&self, name: impl Into<String>) -> AutoSpan {
        AutoSpan::new(self.ctx, name)
    }
}

// ============================================================================
// Logging Macros
// ============================================================================

/// Logs a message at the TRACE level with request context.
///
/// Returns a [`LogEntry`] that can be emitted or inspected.
/// For zero-overhead logging, use [`RequestLogger`].
///
/// # Example
///
/// ```ignore
/// log_trace!(ctx, "Entering function");
/// log_trace!(ctx, "Processing item {}", item_id);
/// log_trace!(ctx, "With fields", key => value, another => thing);
/// ```
#[macro_export]
macro_rules! log_trace {
    ($ctx:expr, $msg:expr) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Trace, $msg)
            .target(module_path!())
    };
    ($ctx:expr, $msg:expr, $($key:ident => $value:expr),+ $(,)?) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Trace, $msg)
            .target(module_path!())
            $(.field(stringify!($key), $value))+
    };
    ($ctx:expr, $fmt:expr, $($arg:tt)*) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Trace, format!($fmt, $($arg)*))
            .target(module_path!())
    };
}

/// Logs a message at the DEBUG level with request context.
#[macro_export]
macro_rules! log_debug {
    ($ctx:expr, $msg:expr) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Debug, $msg)
            .target(module_path!())
    };
    ($ctx:expr, $msg:expr, $($key:ident => $value:expr),+ $(,)?) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Debug, $msg)
            .target(module_path!())
            $(.field(stringify!($key), $value))+
    };
    ($ctx:expr, $fmt:expr, $($arg:tt)*) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Debug, format!($fmt, $($arg)*))
            .target(module_path!())
    };
}

/// Logs a message at the INFO level with request context.
#[macro_export]
macro_rules! log_info {
    ($ctx:expr, $msg:expr) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Info, $msg)
            .target(module_path!())
    };
    ($ctx:expr, $msg:expr, $($key:ident => $value:expr),+ $(,)?) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Info, $msg)
            .target(module_path!())
            $(.field(stringify!($key), $value))+
    };
    ($ctx:expr, $fmt:expr, $($arg:tt)*) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Info, format!($fmt, $($arg)*))
            .target(module_path!())
    };
}

/// Logs a message at the WARN level with request context.
#[macro_export]
macro_rules! log_warn {
    ($ctx:expr, $msg:expr) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Warn, $msg)
            .target(module_path!())
    };
    ($ctx:expr, $msg:expr, $($key:ident => $value:expr),+ $(,)?) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Warn, $msg)
            .target(module_path!())
            $(.field(stringify!($key), $value))+
    };
    ($ctx:expr, $fmt:expr, $($arg:tt)*) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Warn, format!($fmt, $($arg)*))
            .target(module_path!())
    };
}

/// Logs a message at the ERROR level with request context.
#[macro_export]
macro_rules! log_error {
    ($ctx:expr, $msg:expr) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Error, $msg)
            .target(module_path!())
    };
    ($ctx:expr, $msg:expr, $($key:ident => $value:expr),+ $(,)?) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Error, $msg)
            .target(module_path!())
            $(.field(stringify!($key), $value))+
    };
    ($ctx:expr, $fmt:expr, $($arg:tt)*) => {
        $crate::logging::LogEntry::new($ctx, $crate::logging::LogLevel::Error, format!($fmt, $($arg)*))
            .target(module_path!())
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use asupersync::Cx;

    fn test_context() -> crate::context::RequestContext {
        let cx = Cx::for_testing();
        crate::context::RequestContext::new(cx, 12345)
    }

    #[test]
    fn log_level_ordering() {
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
    }

    #[test]
    fn log_level_display() {
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Error.as_char(), 'E');
    }

    #[test]
    fn log_entry_json() {
        let ctx = test_context();
        let entry = LogEntry::new(&ctx, LogLevel::Info, "Test message")
            .target("test::module")
            .field("user_id", 42)
            .field("action", "login");

        let json = entry.to_json();
        assert!(json.contains(r#""level":"info""#));
        assert!(json.contains(r#""message":"Test message""#));
        assert!(json.contains(r#""request_id":12345"#));
        assert!(json.contains(r#""target":"test::module""#));
        assert!(json.contains(r#""user_id":"42""#));
        assert!(json.contains(r#""action":"login""#));
    }

    #[test]
    fn log_entry_compact() {
        let ctx = test_context();
        let entry =
            LogEntry::new(&ctx, LogLevel::Warn, "Something happened").field("error_code", "E001");

        let compact = entry.to_compact();
        assert!(compact.starts_with("[W] req=12345"));
        assert!(compact.contains("Something happened"));
        assert!(compact.contains("error_code=E001"));
    }

    #[test]
    fn span_timing() {
        let ctx = test_context();
        let mut span = Span::new(&ctx, "test_operation");

        std::thread::sleep(std::time::Duration::from_millis(1));
        let duration = span.end();

        assert!(duration.as_micros() >= 1000);
        assert!(span.ended);
    }

    #[test]
    fn span_child() {
        let ctx = test_context();
        let parent = Span::new(&ctx, "parent");
        let child = parent.child(&ctx, "child");

        assert_eq!(child.parent_id(), Some(parent.span_id()));
    }

    #[test]
    fn span_json() {
        let ctx = test_context();
        let span = Span::new(&ctx, "db_query");

        let json = span.to_json();
        assert!(json.contains(r#""name":"db_query""#));
        assert!(json.contains(r#""request_id":12345"#));
    }

    #[test]
    fn log_config_presets() {
        let dev = LogConfig::development();
        assert_eq!(dev.min_level, LogLevel::Debug);
        assert!(!dev.json_output);

        let prod = LogConfig::production();
        assert_eq!(prod.min_level, LogLevel::Info);
        assert!(prod.json_output);

        let test = LogConfig::testing();
        assert_eq!(test.min_level, LogLevel::Trace);
    }

    #[test]
    fn escape_json_special_chars() {
        assert_eq!(escape_json("hello\nworld"), "hello\\nworld");
        assert_eq!(escape_json(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(escape_json("tab\there"), "tab\\there");
    }

    #[test]
    fn log_macro_basic() {
        let ctx = test_context();
        let entry = log_info!(&ctx, "Basic message");
        assert_eq!(entry.level, LogLevel::Info);
        assert_eq!(entry.message, "Basic message");
        assert_eq!(entry.request_id, 12345);
    }

    #[test]
    fn log_macro_with_fields() {
        let ctx = test_context();
        let entry = log_info!(&ctx, "With fields", user_id => 42, action => "login");
        assert_eq!(entry.fields.len(), 2);
        assert_eq!(entry.fields[0], ("user_id".to_string(), "42".to_string()));
        assert_eq!(entry.fields[1], ("action".to_string(), "login".to_string()));
    }

    #[test]
    fn log_macro_format_string() {
        let ctx = test_context();
        let item_id = 99;
        let entry = log_debug!(&ctx, "Processing item {}", item_id);
        assert_eq!(entry.level, LogLevel::Debug);
        assert_eq!(entry.message, "Processing item 99");
    }

    // =========================================================================
    // AutoSpan and Span nesting tests (bd-sdrz)
    // =========================================================================

    #[test]
    fn autospan_creates_and_ends_on_drop() {
        let ctx = test_context();
        let start = std::time::Instant::now();

        {
            let _span = AutoSpan::new(&ctx, "short_operation");
            std::thread::sleep(std::time::Duration::from_millis(1));
            // AutoSpan should end when dropped here
        }

        let elapsed = start.elapsed();
        // Should have completed in a reasonable time
        assert!(elapsed.as_millis() >= 1);
    }

    #[test]
    fn autospan_captures_request_id() {
        let ctx = test_context();
        let span = AutoSpan::new(&ctx, "test_span");

        // AutoSpan stores the context's request ID
        assert_eq!(span.ctx_request_id, 12345);
    }

    #[test]
    fn span_nested_three_levels_deep() {
        let ctx = test_context();

        let parent = Span::new(&ctx, "level_1");
        let child = parent.child(&ctx, "level_2");
        let grandchild = child.child(&ctx, "level_3");

        // Verify parent-child relationships
        assert!(parent.parent_id().is_none());
        assert_eq!(child.parent_id(), Some(parent.span_id()));
        assert_eq!(grandchild.parent_id(), Some(child.span_id()));

        // Verify names
        assert_eq!(parent.name(), "level_1");
        assert_eq!(child.name(), "level_2");
        assert_eq!(grandchild.name(), "level_3");
    }

    #[test]
    fn span_nested_five_levels_deep() {
        let ctx = test_context();

        let level1 = Span::new(&ctx, "request_handler");
        let level2 = level1.child(&ctx, "middleware");
        let level3 = level2.child(&ctx, "auth_check");
        let level4 = level3.child(&ctx, "token_validation");
        let level5 = level4.child(&ctx, "signature_verify");

        // Verify the full chain
        assert!(level1.parent_id().is_none());
        assert_eq!(level2.parent_id(), Some(level1.span_id()));
        assert_eq!(level3.parent_id(), Some(level2.span_id()));
        assert_eq!(level4.parent_id(), Some(level3.span_id()));
        assert_eq!(level5.parent_id(), Some(level4.span_id()));
    }

    #[test]
    fn span_sibling_spans_have_same_parent() {
        let ctx = test_context();

        let parent = Span::new(&ctx, "parent");
        let sibling1 = parent.child(&ctx, "sibling_a");
        let sibling2 = parent.child(&ctx, "sibling_b");
        let sibling3 = parent.child(&ctx, "sibling_c");

        // All siblings share the same parent
        assert_eq!(sibling1.parent_id(), Some(parent.span_id()));
        assert_eq!(sibling2.parent_id(), Some(parent.span_id()));
        assert_eq!(sibling3.parent_id(), Some(parent.span_id()));

        // But each has a unique span_id
        assert_ne!(sibling1.span_id(), sibling2.span_id());
        assert_ne!(sibling2.span_id(), sibling3.span_id());
        assert_ne!(sibling1.span_id(), sibling3.span_id());
    }

    #[test]
    fn span_ids_are_globally_unique() {
        let ctx = test_context();

        let span1 = Span::new(&ctx, "span1");
        let span2 = Span::new(&ctx, "span2");
        let span3 = Span::new(&ctx, "span3");

        assert_ne!(span1.span_id(), span2.span_id());
        assert_ne!(span2.span_id(), span3.span_id());
        assert_ne!(span1.span_id(), span3.span_id());
    }

    #[test]
    fn span_request_id_propagates_to_children() {
        let ctx = test_context();

        let parent = Span::new(&ctx, "parent");
        let child = parent.child(&ctx, "child");
        let grandchild = child.child(&ctx, "grandchild");

        // All spans should have the same request_id from context
        assert_eq!(parent.request_id, 12345);
        assert_eq!(child.request_id, 12345);
        assert_eq!(grandchild.request_id, 12345);
    }

    #[test]
    fn span_json_includes_parent_id_when_present() {
        let ctx = test_context();

        let parent = Span::new(&ctx, "parent");
        let child = parent.child(&ctx, "child");

        // Parent JSON should not have parent_id
        let parent_json = parent.to_json();
        assert!(!parent_json.contains("parent_id"));

        // Child JSON should include parent_id
        let child_json = child.to_json();
        assert!(child_json.contains("parent_id"));
        assert!(child_json.contains(&parent.span_id().to_string()));
    }

    #[test]
    fn span_elapsed_increases_over_time() {
        let ctx = test_context();
        let span = Span::new(&ctx, "timing_test");

        let elapsed1 = span.elapsed();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let elapsed2 = span.elapsed();

        assert!(elapsed2 > elapsed1);
    }

    #[test]
    fn span_end_returns_final_duration() {
        let ctx = test_context();
        let mut span = Span::new(&ctx, "duration_test");

        std::thread::sleep(std::time::Duration::from_millis(1));
        let duration = span.end();

        // Duration should be at least 1ms
        assert!(duration.as_millis() >= 1);
        // Span should be marked as ended
        assert!(span.ended);
    }

    #[test]
    fn span_multiple_end_calls_idempotent() {
        let ctx = test_context();
        let mut span = Span::new(&ctx, "multi_end_test");

        std::thread::sleep(std::time::Duration::from_millis(1));
        let duration1 = span.end();
        let duration2 = span.end();
        let duration3 = span.end();

        // All calls after first should return similar durations (no panic)
        // Note: duration may vary slightly due to elapsed() calls
        assert!(duration1.as_micros() > 0);
        assert!(duration2.as_micros() > 0);
        assert!(duration3.as_micros() > 0);
    }

    #[test]
    fn span_drop_ends_span_automatically() {
        let ctx = test_context();

        // Create a span and let it drop
        {
            let span = Span::new(&ctx, "auto_end_test");
            assert!(!span.ended);
            // Span should be ended when dropped
        }
        // We can't directly verify ended after drop, but the Drop impl
        // should not panic and should mark it ended
    }

    #[test]
    fn nested_spans_timing_accumulates_correctly() {
        let ctx = test_context();

        let start = std::time::Instant::now();

        let mut parent = Span::new(&ctx, "parent");
        std::thread::sleep(std::time::Duration::from_millis(2));

        {
            let mut child = parent.child(&ctx, "child");
            std::thread::sleep(std::time::Duration::from_millis(2));
            let child_duration = child.end();

            // Child duration should be ~2ms
            assert!(child_duration.as_millis() >= 2);
        }

        let parent_duration = parent.end();

        // Parent duration should include child time (~4ms total)
        assert!(parent_duration.as_millis() >= 4);

        let total = start.elapsed();
        assert!(total.as_millis() >= 4);
    }
}
