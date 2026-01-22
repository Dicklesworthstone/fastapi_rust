//! Request/response logging component.
//!
//! Provides colorized HTTP request/response logging with timing information.

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;
use std::time::Duration;

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";

/// HTTP methods supported for logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    /// GET request.
    Get,
    /// POST request.
    Post,
    /// PUT request.
    Put,
    /// DELETE request.
    Delete,
    /// PATCH request.
    Patch,
    /// OPTIONS request.
    Options,
    /// HEAD request.
    Head,
    /// TRACE request.
    Trace,
    /// CONNECT request.
    Connect,
}

impl HttpMethod {
    /// Get the method name as a string.
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
            Self::Connect => "CONNECT",
        }
    }

    /// Get the color for this method from the theme.
    fn color(self, theme: &FastApiTheme) -> crate::themes::Color {
        match self {
            Self::Get => theme.http_get,
            Self::Post => theme.http_post,
            Self::Put => theme.http_put,
            Self::Delete => theme.http_delete,
            Self::Patch => theme.http_patch,
            Self::Options => theme.http_options,
            Self::Head => theme.http_head,
            // Fallback for less common methods
            Self::Trace | Self::Connect => theme.muted,
        }
    }
}

impl std::str::FromStr for HttpMethod {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "GET" => Ok(Self::Get),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "DELETE" => Ok(Self::Delete),
            "PATCH" => Ok(Self::Patch),
            "OPTIONS" => Ok(Self::Options),
            "HEAD" => Ok(Self::Head),
            "TRACE" => Ok(Self::Trace),
            "CONNECT" => Ok(Self::Connect),
            _ => Err(()),
        }
    }
}

/// Response timing information.
#[derive(Debug, Clone, Copy)]
pub struct ResponseTiming {
    /// Total request duration.
    pub total: Duration,
}

impl ResponseTiming {
    /// Create a new timing with the given duration.
    #[must_use]
    pub const fn new(total: Duration) -> Self {
        Self { total }
    }

    /// Format the timing as a human-readable string.
    #[must_use]
    pub fn format(&self) -> String {
        let micros = self.total.as_micros();
        if micros < 1000 {
            format!("{micros}µs")
        } else if micros < 1_000_000 {
            let whole = micros / 1000;
            let frac = (micros % 1000) / 10;
            format!("{whole}.{frac:02}ms")
        } else {
            let whole = micros / 1_000_000;
            let frac = (micros % 1_000_000) / 10_000;
            format!("{whole}.{frac:02}s")
        }
    }
}

/// A log entry for request/response logging.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// HTTP method.
    pub method: HttpMethod,
    /// Request path.
    pub path: String,
    /// Query string (if any).
    pub query: Option<String>,
    /// Response status code.
    pub status: u16,
    /// Response timing.
    pub timing: Option<ResponseTiming>,
    /// Client IP address.
    pub client_ip: Option<String>,
    /// Request ID.
    pub request_id: Option<String>,
}

impl LogEntry {
    /// Create a new log entry.
    #[must_use]
    pub fn new(method: HttpMethod, path: impl Into<String>, status: u16) -> Self {
        Self {
            method,
            path: path.into(),
            query: None,
            status,
            timing: None,
            client_ip: None,
            request_id: None,
        }
    }

    /// Set the query string.
    #[must_use]
    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }

    /// Set the response timing.
    #[must_use]
    pub fn timing(mut self, timing: ResponseTiming) -> Self {
        self.timing = Some(timing);
        self
    }

    /// Set the client IP.
    #[must_use]
    pub fn client_ip(mut self, ip: impl Into<String>) -> Self {
        self.client_ip = Some(ip.into());
        self
    }

    /// Set the request ID.
    #[must_use]
    pub fn request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }
}

/// Request/response logger.
#[derive(Debug, Clone)]
pub struct RequestLogger {
    mode: OutputMode,
    theme: FastApiTheme,
    /// Show client IP in logs.
    pub show_client_ip: bool,
    /// Show request ID in logs.
    pub show_request_id: bool,
    /// Show query string in logs.
    pub show_query: bool,
}

impl RequestLogger {
    /// Create a new logger with the specified mode.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            show_client_ip: false,
            show_request_id: false,
            show_query: true,
        }
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Format a log entry to a string.
    #[must_use]
    pub fn format(&self, entry: &LogEntry) -> String {
        match self.mode {
            OutputMode::Plain => self.format_plain(entry),
            OutputMode::Minimal => self.format_minimal(entry),
            OutputMode::Rich => self.format_rich(entry),
        }
    }

    fn format_plain(&self, entry: &LogEntry) -> String {
        let mut parts = Vec::new();

        // Method
        parts.push(format!("{:7}", entry.method.as_str()));

        // Path with query
        let path = if self.show_query {
            match &entry.query {
                Some(q) => format!("{}?{}", entry.path, q),
                None => entry.path.clone(),
            }
        } else {
            entry.path.clone()
        };
        parts.push(path);

        // Status
        parts.push(format!("{}", entry.status));

        // Timing
        if let Some(timing) = &entry.timing {
            parts.push(timing.format());
        }

        // Client IP
        if self.show_client_ip {
            if let Some(ip) = &entry.client_ip {
                parts.push(format!("[{ip}]"));
            }
        }

        // Request ID
        if self.show_request_id {
            if let Some(id) = &entry.request_id {
                parts.push(format!("({id})"));
            }
        }

        parts.join(" ")
    }

    fn format_minimal(&self, entry: &LogEntry) -> String {
        let method_color = entry.method.color(&self.theme).to_ansi_fg();
        let status_color = self.status_color(entry.status).to_ansi_fg();

        let mut parts = Vec::new();

        // Method with color
        parts.push(format!(
            "{method_color}{:7}{ANSI_RESET}",
            entry.method.as_str()
        ));

        // Path with query
        let path = if self.show_query {
            match &entry.query {
                Some(q) => format!("{}?{}", entry.path, q),
                None => entry.path.clone(),
            }
        } else {
            entry.path.clone()
        };
        parts.push(path);

        // Status with color
        parts.push(format!("{status_color}{}{ANSI_RESET}", entry.status));

        // Timing
        if let Some(timing) = &entry.timing {
            let muted = self.theme.muted.to_ansi_fg();
            parts.push(format!("{muted}{}{ANSI_RESET}", timing.format()));
        }

        parts.join(" ")
    }

    fn format_rich(&self, entry: &LogEntry) -> String {
        let status_color = self.status_color(entry.status).to_ansi_fg();
        let muted = self.theme.muted.to_ansi_fg();

        let mut parts = Vec::new();

        // Method badge
        let method_bg = entry.method.color(&self.theme).to_ansi_bg();
        parts.push(format!(
            "{method_bg}{ANSI_BOLD} {:7} {ANSI_RESET}",
            entry.method.as_str()
        ));

        // Path with query highlighting
        if self.show_query {
            match &entry.query {
                Some(q) => {
                    let accent = self.theme.accent.to_ansi_fg();
                    parts.push(format!("{}{accent}?{q}{ANSI_RESET}", entry.path));
                }
                None => parts.push(entry.path.clone()),
            }
        } else {
            parts.push(entry.path.clone());
        }

        // Status code with icon
        let status_icon = Self::status_icon(entry.status);
        parts.push(format!(
            "{status_color}{status_icon} {}{ANSI_RESET}",
            entry.status
        ));

        // Timing
        if let Some(timing) = &entry.timing {
            parts.push(format!("{muted}{}{ANSI_RESET}", timing.format()));
        }

        // Client IP
        if self.show_client_ip {
            if let Some(ip) = &entry.client_ip {
                parts.push(format!("{muted}[{ip}]{ANSI_RESET}"));
            }
        }

        // Request ID
        if self.show_request_id {
            if let Some(id) = &entry.request_id {
                parts.push(format!("{muted}({id}){ANSI_RESET}"));
            }
        }

        parts.join(" ")
    }

    fn status_color(&self, status: u16) -> crate::themes::Color {
        match status {
            100..=199 => self.theme.status_1xx,
            200..=299 => self.theme.status_2xx,
            300..=399 => self.theme.status_3xx,
            400..=499 => self.theme.status_4xx,
            500..=599 => self.theme.status_5xx,
            _ => self.theme.muted,
        }
    }

    fn status_icon(status: u16) -> &'static str {
        match status {
            100..=199 => "ℹ",
            200..=299 => "✓",
            300..=399 => "→",
            400..=499 => "⚠",
            500..=599 => "✗",
            _ => "?",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_method_as_str() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
        assert_eq!(HttpMethod::Delete.as_str(), "DELETE");
    }

    #[test]
    fn test_http_method_from_str() {
        assert_eq!("get".parse::<HttpMethod>().ok(), Some(HttpMethod::Get));
        assert_eq!("POST".parse::<HttpMethod>().ok(), Some(HttpMethod::Post));
        assert!("invalid".parse::<HttpMethod>().is_err());
    }

    #[test]
    fn test_response_timing_format() {
        assert_eq!(
            ResponseTiming::new(Duration::from_micros(500)).format(),
            "500µs"
        );
        assert_eq!(
            ResponseTiming::new(Duration::from_micros(1500)).format(),
            "1.50ms"
        );
        assert_eq!(
            ResponseTiming::new(Duration::from_secs(2)).format(),
            "2.00s"
        );
    }

    #[test]
    fn test_log_entry_builder() {
        let entry = LogEntry::new(HttpMethod::Get, "/api/users", 200)
            .query("page=1")
            .timing(ResponseTiming::new(Duration::from_millis(50)))
            .client_ip("127.0.0.1")
            .request_id("req-123");

        assert_eq!(entry.method, HttpMethod::Get);
        assert_eq!(entry.path, "/api/users");
        assert_eq!(entry.query, Some("page=1".to_string()));
        assert_eq!(entry.status, 200);
    }

    #[test]
    fn test_logger_plain_format() {
        let logger = RequestLogger::new(OutputMode::Plain);
        let entry = LogEntry::new(HttpMethod::Get, "/api/users", 200)
            .timing(ResponseTiming::new(Duration::from_millis(50)));

        let output = logger.format(&entry);

        assert!(output.contains("GET"));
        assert!(output.contains("/api/users"));
        assert!(output.contains("200"));
        assert!(!output.contains("\x1b[")); // No ANSI codes
    }

    #[test]
    fn test_logger_plain_with_query() {
        let logger = RequestLogger::new(OutputMode::Plain);
        let entry = LogEntry::new(HttpMethod::Get, "/api/users", 200).query("page=1&limit=10");

        let output = logger.format(&entry);

        assert!(output.contains("/api/users?page=1&limit=10"));
    }

    #[test]
    fn test_logger_rich_has_ansi() {
        let logger = RequestLogger::new(OutputMode::Rich);
        let entry = LogEntry::new(HttpMethod::Post, "/api/create", 201);

        let output = logger.format(&entry);

        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_logger_with_client_ip() {
        let mut logger = RequestLogger::new(OutputMode::Plain);
        logger.show_client_ip = true;

        let entry = LogEntry::new(HttpMethod::Get, "/", 200).client_ip("192.168.1.1");

        let output = logger.format(&entry);

        assert!(output.contains("[192.168.1.1]"));
    }

    #[test]
    fn test_logger_with_request_id() {
        let mut logger = RequestLogger::new(OutputMode::Plain);
        logger.show_request_id = true;

        let entry = LogEntry::new(HttpMethod::Get, "/", 200).request_id("abc-123");

        let output = logger.format(&entry);

        assert!(output.contains("(abc-123)"));
    }
}
