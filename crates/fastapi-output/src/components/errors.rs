//! Error formatting component.
//!
//! Provides formatted error display for validation errors, HTTP errors,
//! and internal errors with location path display.

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;
use std::fmt::Write;

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";

/// Location item for error paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocItem {
    /// Field name.
    Field(String),
    /// Array index.
    Index(usize),
}

impl LocItem {
    /// Create a field location item.
    #[must_use]
    pub fn field(name: impl Into<String>) -> Self {
        Self::Field(name.into())
    }

    /// Create an index location item.
    #[must_use]
    pub const fn index(idx: usize) -> Self {
        Self::Index(idx)
    }

    /// Format the location item.
    #[must_use]
    pub fn format(&self) -> String {
        match self {
            Self::Field(name) => name.clone(),
            Self::Index(idx) => format!("[{idx}]"),
        }
    }
}

/// A single validation error.
#[derive(Debug, Clone)]
pub struct ValidationErrorDetail {
    /// Location path (e.g., ["body", "user", "email"]).
    pub loc: Vec<LocItem>,
    /// Error message.
    pub msg: String,
    /// Error type (e.g., "value_error", "type_error").
    pub error_type: String,
}

impl ValidationErrorDetail {
    /// Create a new validation error.
    #[must_use]
    pub fn new(loc: Vec<LocItem>, msg: impl Into<String>, error_type: impl Into<String>) -> Self {
        Self {
            loc,
            msg: msg.into(),
            error_type: error_type.into(),
        }
    }

    /// Format the location path as a string.
    #[must_use]
    pub fn format_loc(&self) -> String {
        if self.loc.is_empty() {
            return String::new();
        }

        let mut result = String::new();
        for (i, item) in self.loc.iter().enumerate() {
            match item {
                LocItem::Field(name) => {
                    if i > 0 {
                        result.push('.');
                    }
                    result.push_str(name);
                }
                LocItem::Index(idx) => {
                    let _ = write!(result, "[{idx}]");
                }
            }
        }
        result
    }
}

/// HTTP error information.
#[derive(Debug, Clone)]
pub struct HttpErrorInfo {
    /// HTTP status code.
    pub status: u16,
    /// Error detail message.
    pub detail: String,
    /// Optional error code.
    pub code: Option<String>,
    /// Request path (for context).
    pub path: Option<String>,
    /// Request method (for context).
    pub method: Option<String>,
}

impl HttpErrorInfo {
    /// Create a new HTTP error.
    #[must_use]
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
            code: None,
            path: None,
            method: None,
        }
    }

    /// Set the error code.
    #[must_use]
    pub fn code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Set the request path.
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Set the request method.
    #[must_use]
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    /// Get the status category name.
    #[must_use]
    pub fn status_category(&self) -> &'static str {
        match self.status {
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            422 => "Unprocessable Entity",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ if self.status >= 400 && self.status < 500 => "Client Error",
            _ if self.status >= 500 => "Server Error",
            _ => "Error",
        }
    }
}

/// Formatted error output.
#[derive(Debug, Clone)]
pub struct FormattedError {
    /// Plain text version.
    pub plain: String,
    /// ANSI-formatted version.
    pub rich: String,
}

/// Error formatter.
#[derive(Debug, Clone)]
pub struct ErrorFormatter {
    mode: OutputMode,
    theme: FastApiTheme,
    /// Whether to show error codes.
    pub show_codes: bool,
    /// Whether to show request context.
    pub show_context: bool,
}

impl ErrorFormatter {
    /// Create a new error formatter.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            show_codes: true,
            show_context: true,
        }
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Format a list of validation errors.
    #[must_use]
    pub fn format_validation_errors(&self, errors: &[ValidationErrorDetail]) -> FormattedError {
        match self.mode {
            OutputMode::Plain => {
                let plain = self.format_validation_plain(errors);
                FormattedError {
                    plain: plain.clone(),
                    rich: plain,
                }
            }
            OutputMode::Minimal | OutputMode::Rich => {
                let plain = self.format_validation_plain(errors);
                let rich = self.format_validation_rich(errors);
                FormattedError { plain, rich }
            }
        }
    }

    fn format_validation_plain(&self, errors: &[ValidationErrorDetail]) -> String {
        let mut lines = Vec::new();

        lines.push(format!(
            "Validation Error ({count} error(s)):",
            count = errors.len()
        ));
        lines.push(String::new());

        for error in errors {
            let loc = error.format_loc();
            if loc.is_empty() {
                lines.push(format!("  - {msg}", msg = error.msg));
            } else {
                lines.push(format!("  - {loc}: {msg}", msg = error.msg));
            }

            if self.show_codes {
                lines.push(format!(
                    "    [type: {error_type}]",
                    error_type = error.error_type
                ));
            }
        }

        lines.join("\n")
    }

    fn format_validation_rich(&self, errors: &[ValidationErrorDetail]) -> String {
        let mut lines = Vec::new();
        let error_color = self.theme.error.to_ansi_fg();
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let warning = self.theme.warning.to_ansi_fg();

        // Header
        lines.push(format!(
            "{error_color}{ANSI_BOLD}✗ Validation Error{ANSI_RESET} {muted}({count} error(s)){ANSI_RESET}",
            count = errors.len()
        ));
        lines.push(String::new());

        for error in errors {
            let loc = error.format_loc();

            // Error line with location
            if loc.is_empty() {
                lines.push(format!("  {warning}●{ANSI_RESET} {msg}", msg = error.msg));
            } else {
                lines.push(format!(
                    "  {warning}●{ANSI_RESET} {accent}{loc}{ANSI_RESET}: {msg}",
                    msg = error.msg
                ));
            }

            // Error type
            if self.show_codes {
                lines.push(format!(
                    "    {muted}[type: {error_type}]{ANSI_RESET}",
                    error_type = error.error_type
                ));
            }
        }

        lines.join("\n")
    }

    /// Format an HTTP error.
    #[must_use]
    pub fn format_http_error(&self, error: &HttpErrorInfo) -> FormattedError {
        match self.mode {
            OutputMode::Plain => {
                let plain = self.format_http_plain(error);
                FormattedError {
                    plain: plain.clone(),
                    rich: plain,
                }
            }
            OutputMode::Minimal | OutputMode::Rich => {
                let plain = self.format_http_plain(error);
                let rich = self.format_http_rich(error);
                FormattedError { plain, rich }
            }
        }
    }

    fn format_http_plain(&self, error: &HttpErrorInfo) -> String {
        let mut lines = Vec::new();

        // Status line
        lines.push(format!(
            "HTTP {status} {category}",
            status = error.status,
            category = error.status_category()
        ));

        // Detail
        lines.push(format!("Detail: {detail}", detail = error.detail));

        // Code
        if self.show_codes {
            if let Some(code) = &error.code {
                lines.push(format!("Code: {code}"));
            }
        }

        // Context
        if self.show_context {
            if let (Some(method), Some(path)) = (&error.method, &error.path) {
                lines.push(format!("Request: {method} {path}"));
            }
        }

        lines.join("\n")
    }

    fn format_http_rich(&self, error: &HttpErrorInfo) -> String {
        let mut lines = Vec::new();
        let status_color = self.status_color(error.status).to_ansi_fg();
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();

        // Status line with color
        let icon = if error.status >= 500 { "✗" } else { "⚠" };
        lines.push(format!(
            "{status_color}{ANSI_BOLD}{icon} HTTP {status}{ANSI_RESET} {muted}{category}{ANSI_RESET}",
            status = error.status,
            category = error.status_category()
        ));

        // Detail
        lines.push(format!("  {detail}", detail = error.detail));

        // Code
        if self.show_codes {
            if let Some(code) = &error.code {
                lines.push(format!("  {muted}Code: {accent}{code}{ANSI_RESET}"));
            }
        }

        // Context
        if self.show_context {
            if let (Some(method), Some(path)) = (&error.method, &error.path) {
                lines.push(format!(
                    "  {muted}Request: {accent}{method} {path}{ANSI_RESET}"
                ));
            }
        }

        lines.join("\n")
    }

    fn status_color(&self, status: u16) -> crate::themes::Color {
        match status {
            400..=499 => self.theme.status_4xx,
            500..=599 => self.theme.status_5xx,
            _ => self.theme.muted,
        }
    }

    /// Format a simple error message.
    #[must_use]
    pub fn format_simple(&self, message: &str) -> FormattedError {
        let plain = format!("Error: {message}");

        let rich = match self.mode {
            OutputMode::Plain => plain.clone(),
            OutputMode::Minimal | OutputMode::Rich => {
                let error_color = self.theme.error.to_ansi_fg();
                format!("{error_color}{ANSI_BOLD}✗ Error:{ANSI_RESET} {message}")
            }
        };

        FormattedError { plain, rich }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loc_item_format() {
        assert_eq!(LocItem::field("name").format(), "name");
        assert_eq!(LocItem::index(0).format(), "[0]");
    }

    #[test]
    fn test_validation_error_format_loc() {
        let error = ValidationErrorDetail::new(
            vec![
                LocItem::field("body"),
                LocItem::field("users"),
                LocItem::index(0),
                LocItem::field("email"),
            ],
            "invalid email",
            "value_error",
        );

        assert_eq!(error.format_loc(), "body.users[0].email");
    }

    #[test]
    fn test_validation_error_empty_loc() {
        let error = ValidationErrorDetail::new(vec![], "missing field", "value_error");
        assert_eq!(error.format_loc(), "");
    }

    #[test]
    fn test_http_error_builder() {
        let error = HttpErrorInfo::new(404, "Resource not found")
            .code("NOT_FOUND")
            .path("/api/users/123")
            .method("GET");

        assert_eq!(error.status, 404);
        assert_eq!(error.detail, "Resource not found");
        assert_eq!(error.code, Some("NOT_FOUND".to_string()));
        assert_eq!(error.path, Some("/api/users/123".to_string()));
    }

    #[test]
    fn test_http_error_status_category() {
        assert_eq!(HttpErrorInfo::new(400, "").status_category(), "Bad Request");
        assert_eq!(HttpErrorInfo::new(404, "").status_category(), "Not Found");
        assert_eq!(
            HttpErrorInfo::new(500, "").status_category(),
            "Internal Server Error"
        );
        assert_eq!(
            HttpErrorInfo::new(418, "").status_category(),
            "Client Error"
        );
    }

    #[test]
    fn test_formatter_validation_plain() {
        let formatter = ErrorFormatter::new(OutputMode::Plain);
        let errors = vec![
            ValidationErrorDetail::new(
                vec![LocItem::field("body"), LocItem::field("email")],
                "invalid email format",
                "value_error.email",
            ),
            ValidationErrorDetail::new(
                vec![LocItem::field("body"), LocItem::field("age")],
                "must be positive",
                "value_error.number",
            ),
        ];

        let result = formatter.format_validation_errors(&errors);

        assert!(result.plain.contains("Validation Error"));
        assert!(result.plain.contains("2 error(s)"));
        assert!(result.plain.contains("body.email"));
        assert!(result.plain.contains("invalid email format"));
        assert!(result.plain.contains("body.age"));
        assert!(!result.plain.contains("\x1b["));
    }

    #[test]
    fn test_formatter_validation_rich_has_ansi() {
        let formatter = ErrorFormatter::new(OutputMode::Rich);
        let errors = vec![ValidationErrorDetail::new(
            vec![LocItem::field("name")],
            "required",
            "value_error",
        )];

        let result = formatter.format_validation_errors(&errors);

        assert!(result.rich.contains("\x1b["));
    }

    #[test]
    fn test_formatter_http_plain() {
        let formatter = ErrorFormatter::new(OutputMode::Plain);
        let error = HttpErrorInfo::new(404, "User not found")
            .code("USER_NOT_FOUND")
            .path("/api/users/123")
            .method("GET");

        let result = formatter.format_http_error(&error);

        assert!(result.plain.contains("HTTP 404"));
        assert!(result.plain.contains("Not Found"));
        assert!(result.plain.contains("User not found"));
        assert!(result.plain.contains("USER_NOT_FOUND"));
        assert!(result.plain.contains("GET /api/users/123"));
    }

    #[test]
    fn test_formatter_simple() {
        let formatter = ErrorFormatter::new(OutputMode::Plain);
        let result = formatter.format_simple("Something went wrong");

        assert!(result.plain.contains("Error:"));
        assert!(result.plain.contains("Something went wrong"));
    }

    #[test]
    fn test_formatter_no_codes() {
        let mut formatter = ErrorFormatter::new(OutputMode::Plain);
        formatter.show_codes = false;

        let errors = vec![ValidationErrorDetail::new(
            vec![LocItem::field("field")],
            "error",
            "error_type",
        )];

        let result = formatter.format_validation_errors(&errors);

        assert!(!result.plain.contains("error_type"));
    }
}
