//! HTTP request and response inspector component.
//!
//! Provides detailed visual inspection of HTTP requests and responses
//! for debugging purposes. Unlike the simple one-line logging in `logging.rs`,
//! this module provides comprehensive multi-line output showing headers,
//! body previews, and timing information.
//!
//! # Feature Gating
//!
//! This module is designed for debug output. In production, inspectors
//! should only be called when debug mode is explicitly enabled.
//!
//! ```rust,ignore
//! if output.is_debug_enabled() {
//!     let inspector = RequestInspector::new(OutputMode::Rich);
//!     println!("{}", inspector.inspect(&request_info));
//! }
//! ```

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;
use std::collections::HashMap;
use std::fmt::Write;
use std::time::Duration;

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_DIM: &str = "\x1b[2m";

/// Maximum body preview length in bytes.
const DEFAULT_BODY_PREVIEW_LEN: usize = 512;

/// HTTP request information for inspection.
#[derive(Debug, Clone)]
pub struct RequestInfo {
    /// HTTP method (GET, POST, etc.).
    pub method: String,
    /// Request path.
    pub path: String,
    /// Query string (without leading ?).
    pub query: Option<String>,
    /// HTTP version (e.g., "HTTP/1.1").
    pub http_version: String,
    /// Request headers as key-value pairs.
    pub headers: Vec<(String, String)>,
    /// Body preview (may be truncated).
    pub body_preview: Option<String>,
    /// Total body size in bytes.
    pub body_size: Option<usize>,
    /// Whether the body was truncated.
    pub body_truncated: bool,
    /// Content-Type header value.
    pub content_type: Option<String>,
    /// Parse duration (time to parse the request).
    pub parse_duration: Option<Duration>,
    /// Client IP address.
    pub client_ip: Option<String>,
    /// Request ID.
    pub request_id: Option<String>,
}

impl RequestInfo {
    /// Create a new request info with minimal data.
    #[must_use]
    pub fn new(method: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            query: None,
            http_version: "HTTP/1.1".to_string(),
            headers: Vec::new(),
            body_preview: None,
            body_size: None,
            body_truncated: false,
            content_type: None,
            parse_duration: None,
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

    /// Set the HTTP version.
    #[must_use]
    pub fn http_version(mut self, version: impl Into<String>) -> Self {
        self.http_version = version.into();
        self
    }

    /// Add a header.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set headers from an iterator.
    #[must_use]
    pub fn headers(
        mut self,
        headers: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.headers = headers
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self
    }

    /// Set the body preview.
    #[must_use]
    pub fn body_preview(mut self, preview: impl Into<String>, total_size: usize) -> Self {
        let preview_str = preview.into();
        self.body_truncated = preview_str.len() < total_size;
        self.body_preview = Some(preview_str);
        self.body_size = Some(total_size);
        self
    }

    /// Set the content type.
    #[must_use]
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set the parse duration.
    #[must_use]
    pub fn parse_duration(mut self, duration: Duration) -> Self {
        self.parse_duration = Some(duration);
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

/// HTTP response information for inspection.
#[derive(Debug, Clone)]
pub struct ResponseInfo {
    /// HTTP status code.
    pub status: u16,
    /// Status reason phrase (e.g., "OK", "Not Found").
    pub reason: Option<String>,
    /// Response headers.
    pub headers: Vec<(String, String)>,
    /// Body preview (may be truncated).
    pub body_preview: Option<String>,
    /// Total body size in bytes.
    pub body_size: Option<usize>,
    /// Whether the body was truncated.
    pub body_truncated: bool,
    /// Content-Type header value.
    pub content_type: Option<String>,
    /// Total response time.
    pub response_time: Option<Duration>,
}

impl ResponseInfo {
    /// Create a new response info.
    #[must_use]
    pub fn new(status: u16) -> Self {
        Self {
            status,
            reason: None,
            headers: Vec::new(),
            body_preview: None,
            body_size: None,
            body_truncated: false,
            content_type: None,
            response_time: None,
        }
    }

    /// Set the reason phrase.
    #[must_use]
    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Add a header.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set headers from an iterator.
    #[must_use]
    pub fn headers(
        mut self,
        headers: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.headers = headers
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self
    }

    /// Set the body preview.
    #[must_use]
    pub fn body_preview(mut self, preview: impl Into<String>, total_size: usize) -> Self {
        let preview_str = preview.into();
        self.body_truncated = preview_str.len() < total_size;
        self.body_preview = Some(preview_str);
        self.body_size = Some(total_size);
        self
    }

    /// Set the content type.
    #[must_use]
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set the response time.
    #[must_use]
    pub fn response_time(mut self, duration: Duration) -> Self {
        self.response_time = Some(duration);
        self
    }

    /// Get the default reason phrase for the status code.
    #[must_use]
    pub fn default_reason(&self) -> &'static str {
        match self.status {
            100 => "Continue",
            101 => "Switching Protocols",
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            204 => "No Content",
            301 => "Moved Permanently",
            302 => "Found",
            304 => "Not Modified",
            307 => "Temporary Redirect",
            308 => "Permanent Redirect",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            422 => "Unprocessable Entity",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "Unknown",
        }
    }
}

/// HTTP request inspector.
///
/// Provides detailed visual display of HTTP requests for debugging.
#[derive(Debug, Clone)]
pub struct RequestInspector {
    mode: OutputMode,
    theme: FastApiTheme,
    /// Maximum body preview length.
    pub max_body_preview: usize,
    /// Whether to show all headers.
    pub show_all_headers: bool,
    /// Whether to show timing information.
    pub show_timing: bool,
}

impl RequestInspector {
    /// Create a new request inspector.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            max_body_preview: DEFAULT_BODY_PREVIEW_LEN,
            show_all_headers: true,
            show_timing: true,
        }
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Inspect a request and return formatted output.
    #[must_use]
    pub fn inspect(&self, info: &RequestInfo) -> String {
        match self.mode {
            OutputMode::Plain => self.inspect_plain(info),
            OutputMode::Minimal => self.inspect_minimal(info),
            OutputMode::Rich => self.inspect_rich(info),
        }
    }

    fn inspect_plain(&self, info: &RequestInfo) -> String {
        let mut lines = Vec::new();

        // Request line
        lines.push("=== HTTP Request ===".to_string());
        let full_path = match &info.query {
            Some(q) => format!("{}?{}", info.path, q),
            None => info.path.clone(),
        };
        lines.push(format!("{} {} {}", info.method, full_path, info.http_version));

        // Metadata
        if let Some(ip) = &info.client_ip {
            lines.push(format!("Client: {ip}"));
        }
        if let Some(id) = &info.request_id {
            lines.push(format!("Request-ID: {id}"));
        }
        if self.show_timing {
            if let Some(duration) = info.parse_duration {
                lines.push(format!("Parse time: {}", format_duration(duration)));
            }
        }

        // Headers
        if !info.headers.is_empty() {
            lines.push(String::new());
            lines.push("Headers:".to_string());
            for (name, value) in &info.headers {
                lines.push(format!("  {name}: {value}"));
            }
        }

        // Body
        if let Some(preview) = &info.body_preview {
            lines.push(String::new());
            let size_info = match info.body_size {
                Some(size) if info.body_truncated => format!(" ({size} bytes, truncated)"),
                Some(size) => format!(" ({size} bytes)"),
                None => String::new(),
            };
            lines.push(format!("Body{size_info}:"));
            lines.push(format!("  {preview}"));
        }

        lines.push("====================".to_string());
        lines.join("\n")
    }

    fn inspect_minimal(&self, info: &RequestInfo) -> String {
        let method_color = self.method_color(&info.method).to_ansi_fg();
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();

        let mut lines = Vec::new();

        // Request line with color
        lines.push(format!("{muted}=== HTTP Request ==={ANSI_RESET}"));
        let full_path = match &info.query {
            Some(q) => format!("{}?{}", info.path, q),
            None => info.path.clone(),
        };
        lines.push(format!(
            "{method_color}{}{ANSI_RESET} {full_path} {muted}{}{ANSI_RESET}",
            info.method, info.http_version
        ));

        // Metadata
        if let Some(id) = &info.request_id {
            lines.push(format!("{muted}Request-ID:{ANSI_RESET} {accent}{id}{ANSI_RESET}"));
        }
        if self.show_timing {
            if let Some(duration) = info.parse_duration {
                lines.push(format!(
                    "{muted}Parse time:{ANSI_RESET} {}",
                    format_duration(duration)
                ));
            }
        }

        // Headers (condensed)
        if !info.headers.is_empty() {
            lines.push(format!("{muted}Headers ({}):{ANSI_RESET}", info.headers.len()));
            for (name, value) in &info.headers {
                lines.push(format!("  {accent}{name}:{ANSI_RESET} {value}"));
            }
        }

        lines.push(format!("{muted}=================={ANSI_RESET}"));
        lines.join("\n")
    }

    fn inspect_rich(&self, info: &RequestInfo) -> String {
        let method_color = self.method_color(&info.method);
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let border = self.theme.border.to_ansi_fg();
        let header_style = self.theme.header.to_ansi_fg();

        let mut lines = Vec::new();

        // Top border with title
        lines.push(format!(
            "{border}┌─────────────────────────────────────────────┐{ANSI_RESET}"
        ));
        lines.push(format!(
            "{border}│{ANSI_RESET} {header_style}{ANSI_BOLD}HTTP Request{ANSI_RESET}                                 {border}│{ANSI_RESET}"
        ));
        lines.push(format!(
            "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
        ));

        // Request line with method badge
        let method_bg = method_color.to_ansi_bg();
        let full_path = match &info.query {
            Some(q) => format!(
                "{}{}?{q}{ANSI_RESET}",
                info.path,
                self.theme.accent.to_ansi_fg()
            ),
            None => info.path.clone(),
        };
        lines.push(format!(
            "{border}│{ANSI_RESET} {method_bg}{ANSI_BOLD} {} {ANSI_RESET} {full_path}",
            info.method
        ));

        // Metadata row
        let mut meta_parts = Vec::new();
        if let Some(ip) = &info.client_ip {
            meta_parts.push(format!("Client: {ip}"));
        }
        if let Some(id) = &info.request_id {
            meta_parts.push(format!("ID: {id}"));
        }
        if self.show_timing {
            if let Some(duration) = info.parse_duration {
                meta_parts.push(format!("Parsed: {}", format_duration(duration)));
            }
        }
        if !meta_parts.is_empty() {
            lines.push(format!(
                "{border}│{ANSI_RESET} {muted}{}{ANSI_RESET}",
                meta_parts.join(" │ ")
            ));
        }

        // Headers section
        if !info.headers.is_empty() {
            lines.push(format!(
                "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
            ));
            lines.push(format!(
                "{border}│{ANSI_RESET} {header_style}Headers{ANSI_RESET} {muted}({}){ANSI_RESET}",
                info.headers.len()
            ));

            // Find max header name length for alignment
            let max_name_len = info
                .headers
                .iter()
                .map(|(n, _)| n.len())
                .max()
                .unwrap_or(0)
                .min(20);

            for (name, value) in &info.headers {
                let truncated_name = if name.len() > max_name_len {
                    format!("{}...", &name[..max_name_len - 3])
                } else {
                    name.clone()
                };
                lines.push(format!(
                    "{border}│{ANSI_RESET}   {accent}{:width$}{ANSI_RESET}: {value}",
                    truncated_name,
                    width = max_name_len
                ));
            }
        }

        // Body section
        if let Some(preview) = &info.body_preview {
            lines.push(format!(
                "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
            ));
            let size_info = match info.body_size {
                Some(size) if info.body_truncated => {
                    format!("{muted}({size} bytes, truncated){ANSI_RESET}")
                }
                Some(size) => format!("{muted}({size} bytes){ANSI_RESET}"),
                None => String::new(),
            };
            lines.push(format!(
                "{border}│{ANSI_RESET} {header_style}Body{ANSI_RESET} {size_info}"
            ));
            // Wrap long body preview
            for line in preview.lines().take(5) {
                let truncated = if line.len() > 40 {
                    format!("{}...", &line[..37])
                } else {
                    line.to_string()
                };
                lines.push(format!("{border}│{ANSI_RESET}   {ANSI_DIM}{truncated}{ANSI_RESET}"));
            }
        }

        // Bottom border
        lines.push(format!(
            "{border}└─────────────────────────────────────────────┘{ANSI_RESET}"
        ));

        lines.join("\n")
    }

    fn method_color(&self, method: &str) -> crate::themes::Color {
        match method.to_uppercase().as_str() {
            "GET" => self.theme.http_get,
            "POST" => self.theme.http_post,
            "PUT" => self.theme.http_put,
            "DELETE" => self.theme.http_delete,
            "PATCH" => self.theme.http_patch,
            "OPTIONS" => self.theme.http_options,
            "HEAD" => self.theme.http_head,
            _ => self.theme.muted,
        }
    }
}

/// HTTP response inspector.
///
/// Provides detailed visual display of HTTP responses for debugging.
#[derive(Debug, Clone)]
pub struct ResponseInspector {
    mode: OutputMode,
    theme: FastApiTheme,
    /// Maximum body preview length.
    pub max_body_preview: usize,
    /// Whether to show all headers.
    pub show_all_headers: bool,
    /// Whether to show timing information.
    pub show_timing: bool,
}

impl ResponseInspector {
    /// Create a new response inspector.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            max_body_preview: DEFAULT_BODY_PREVIEW_LEN,
            show_all_headers: true,
            show_timing: true,
        }
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Inspect a response and return formatted output.
    #[must_use]
    pub fn inspect(&self, info: &ResponseInfo) -> String {
        match self.mode {
            OutputMode::Plain => self.inspect_plain(info),
            OutputMode::Minimal => self.inspect_minimal(info),
            OutputMode::Rich => self.inspect_rich(info),
        }
    }

    fn inspect_plain(&self, info: &ResponseInfo) -> String {
        let mut lines = Vec::new();

        // Status line
        lines.push("=== HTTP Response ===".to_string());
        let reason = info.reason.as_deref().unwrap_or(info.default_reason());
        lines.push(format!("HTTP/1.1 {} {reason}", info.status));

        // Timing
        if self.show_timing {
            if let Some(duration) = info.response_time {
                lines.push(format!("Response time: {}", format_duration(duration)));
            }
        }

        // Headers
        if !info.headers.is_empty() {
            lines.push(String::new());
            lines.push("Headers:".to_string());
            for (name, value) in &info.headers {
                lines.push(format!("  {name}: {value}"));
            }
        }

        // Body
        if let Some(preview) = &info.body_preview {
            lines.push(String::new());
            let size_info = match info.body_size {
                Some(size) if info.body_truncated => format!(" ({size} bytes, truncated)"),
                Some(size) => format!(" ({size} bytes)"),
                None => String::new(),
            };
            lines.push(format!("Body{size_info}:"));
            lines.push(format!("  {preview}"));
        }

        lines.push("=====================".to_string());
        lines.join("\n")
    }

    fn inspect_minimal(&self, info: &ResponseInfo) -> String {
        let status_color = self.status_color(info.status).to_ansi_fg();
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();

        let mut lines = Vec::new();

        // Status line with color
        lines.push(format!("{muted}=== HTTP Response ==={ANSI_RESET}"));
        let reason = info.reason.as_deref().unwrap_or(info.default_reason());
        let icon = self.status_icon(info.status);
        lines.push(format!(
            "{status_color}{icon} {} {reason}{ANSI_RESET}",
            info.status
        ));

        // Timing
        if self.show_timing {
            if let Some(duration) = info.response_time {
                lines.push(format!(
                    "{muted}Response time:{ANSI_RESET} {}",
                    format_duration(duration)
                ));
            }
        }

        // Headers (condensed)
        if !info.headers.is_empty() {
            lines.push(format!(
                "{muted}Headers ({}):{ANSI_RESET}",
                info.headers.len()
            ));
            for (name, value) in &info.headers {
                lines.push(format!("  {accent}{name}:{ANSI_RESET} {value}"));
            }
        }

        lines.push(format!("{muted}=================={ANSI_RESET}"));
        lines.join("\n")
    }

    fn inspect_rich(&self, info: &ResponseInfo) -> String {
        let status_color = self.status_color(info.status);
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let border = self.theme.border.to_ansi_fg();
        let header_style = self.theme.header.to_ansi_fg();

        let mut lines = Vec::new();

        // Top border
        lines.push(format!(
            "{border}┌─────────────────────────────────────────────┐{ANSI_RESET}"
        ));
        lines.push(format!(
            "{border}│{ANSI_RESET} {header_style}{ANSI_BOLD}HTTP Response{ANSI_RESET}                                {border}│{ANSI_RESET}"
        ));
        lines.push(format!(
            "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
        ));

        // Status line with badge
        let status_bg = status_color.to_ansi_bg();
        let reason = info.reason.as_deref().unwrap_or(info.default_reason());
        let icon = self.status_icon(info.status);
        lines.push(format!(
            "{border}│{ANSI_RESET} {status_bg}{ANSI_BOLD} {icon} {} {ANSI_RESET} {reason}",
            info.status
        ));

        // Timing
        if self.show_timing {
            if let Some(duration) = info.response_time {
                lines.push(format!(
                    "{border}│{ANSI_RESET} {muted}Response time: {}{ANSI_RESET}",
                    format_duration(duration)
                ));
            }
        }

        // Headers section
        if !info.headers.is_empty() {
            lines.push(format!(
                "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
            ));
            lines.push(format!(
                "{border}│{ANSI_RESET} {header_style}Headers{ANSI_RESET} {muted}({}){ANSI_RESET}",
                info.headers.len()
            ));

            let max_name_len = info
                .headers
                .iter()
                .map(|(n, _)| n.len())
                .max()
                .unwrap_or(0)
                .min(20);

            for (name, value) in &info.headers {
                let truncated_name = if name.len() > max_name_len {
                    format!("{}...", &name[..max_name_len - 3])
                } else {
                    name.clone()
                };
                lines.push(format!(
                    "{border}│{ANSI_RESET}   {accent}{:width$}{ANSI_RESET}: {value}",
                    truncated_name,
                    width = max_name_len
                ));
            }
        }

        // Body section
        if let Some(preview) = &info.body_preview {
            lines.push(format!(
                "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
            ));
            let size_info = match info.body_size {
                Some(size) if info.body_truncated => {
                    format!("{muted}({size} bytes, truncated){ANSI_RESET}")
                }
                Some(size) => format!("{muted}({size} bytes){ANSI_RESET}"),
                None => String::new(),
            };
            lines.push(format!(
                "{border}│{ANSI_RESET} {header_style}Body{ANSI_RESET} {size_info}"
            ));
            for line in preview.lines().take(5) {
                let truncated = if line.len() > 40 {
                    format!("{}...", &line[..37])
                } else {
                    line.to_string()
                };
                lines.push(format!("{border}│{ANSI_RESET}   {ANSI_DIM}{truncated}{ANSI_RESET}"));
            }
        }

        // Bottom border
        lines.push(format!(
            "{border}└─────────────────────────────────────────────┘{ANSI_RESET}"
        ));

        lines.join("\n")
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

    fn status_icon(&self, status: u16) -> &'static str {
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

/// Format a duration in human-readable form.
fn format_duration(duration: Duration) -> String {
    let micros = duration.as_micros();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> RequestInfo {
        RequestInfo::new("POST", "/api/users")
            .query("version=2")
            .http_version("HTTP/1.1")
            .header("Content-Type", "application/json")
            .header("Authorization", "Bearer token123")
            .header("X-Request-ID", "req-abc-123")
            .body_preview(r#"{"name": "Alice", "email": "alice@example.com"}"#, 48)
            .content_type("application/json")
            .parse_duration(Duration::from_micros(150))
            .client_ip("192.168.1.100")
            .request_id("req-abc-123")
    }

    fn sample_response() -> ResponseInfo {
        ResponseInfo::new(201)
            .reason("Created")
            .header("Content-Type", "application/json")
            .header("X-Request-ID", "req-abc-123")
            .header("Location", "/api/users/42")
            .body_preview(r#"{"id": 42, "name": "Alice"}"#, 27)
            .content_type("application/json")
            .response_time(Duration::from_millis(45))
    }

    #[test]
    fn test_request_info_builder() {
        let info = sample_request();
        assert_eq!(info.method, "POST");
        assert_eq!(info.path, "/api/users");
        assert_eq!(info.query, Some("version=2".to_string()));
        assert_eq!(info.headers.len(), 3);
        assert!(info.body_preview.is_some());
    }

    #[test]
    fn test_response_info_builder() {
        let info = sample_response();
        assert_eq!(info.status, 201);
        assert_eq!(info.reason, Some("Created".to_string()));
        assert_eq!(info.headers.len(), 3);
    }

    #[test]
    fn test_request_inspector_plain() {
        let inspector = RequestInspector::new(OutputMode::Plain);
        let output = inspector.inspect(&sample_request());

        assert!(output.contains("HTTP Request"));
        assert!(output.contains("POST"));
        assert!(output.contains("/api/users?version=2"));
        assert!(output.contains("Content-Type: application/json"));
        assert!(output.contains("Authorization: Bearer"));
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn test_request_inspector_rich_has_ansi() {
        let inspector = RequestInspector::new(OutputMode::Rich);
        let output = inspector.inspect(&sample_request());

        assert!(output.contains("\x1b["));
        assert!(output.contains("POST"));
    }

    #[test]
    fn test_response_inspector_plain() {
        let inspector = ResponseInspector::new(OutputMode::Plain);
        let output = inspector.inspect(&sample_response());

        assert!(output.contains("HTTP Response"));
        assert!(output.contains("201"));
        assert!(output.contains("Created"));
        assert!(output.contains("Content-Type: application/json"));
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn test_response_inspector_rich_has_ansi() {
        let inspector = ResponseInspector::new(OutputMode::Rich);
        let output = inspector.inspect(&sample_response());

        assert!(output.contains("\x1b["));
        assert!(output.contains("201"));
    }

    #[test]
    fn test_response_default_reason() {
        let info = ResponseInfo::new(404);
        assert_eq!(info.default_reason(), "Not Found");

        let info = ResponseInfo::new(500);
        assert_eq!(info.default_reason(), "Internal Server Error");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_micros(500)), "500µs");
        assert_eq!(format_duration(Duration::from_micros(1500)), "1.50ms");
        assert_eq!(format_duration(Duration::from_secs(2)), "2.00s");
    }
}
