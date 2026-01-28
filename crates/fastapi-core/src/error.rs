//! Error types.
//!
//! This module provides error types for HTTP responses with support for
//! debug mode that can include additional diagnostic information.
//!
//! # Debug Mode
//!
//! Debug mode can be enabled to include additional diagnostic information
//! in error responses:
//!
//! - Source location (file, line, function)
//! - Full validation error context
//! - Handler name and route pattern
//!
//! Debug mode is controlled per-error via the `with_debug_info` method,
//! and should only be enabled when the application is in debug mode AND
//! the request includes a valid debug token (if configured).
//!
//! # Example
//!
//! ```
//! use fastapi_core::error::{HttpError, DebugInfo};
//!
//! // Production mode - no debug info
//! let error = HttpError::not_found().with_detail("User not found");
//!
//! // Debug mode - with source location
//! let error = HttpError::not_found()
//!     .with_detail("User not found")
//!     .with_debug_info(DebugInfo::new()
//!         .with_source_location(file!(), line!(), "get_user")
//!         .with_route_pattern("/users/{id}"));
//! ```

use crate::response::{IntoResponse, Response, ResponseBody, StatusCode};
use serde::{Serialize, Serializer};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

// ============================================================================
// Debug Mode Configuration
// ============================================================================

/// Global flag for thread-local debug mode state.
/// This is used when debug info needs to be propagated through error creation.
static DEBUG_MODE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable debug mode globally.
///
/// When debug mode is enabled, errors can include additional diagnostic
/// information in their responses. This should be controlled by the
/// application configuration and request-level debug token validation.
pub fn enable_debug_mode() {
    DEBUG_MODE_ENABLED.store(true, Ordering::SeqCst);
}

/// Disable debug mode globally.
pub fn disable_debug_mode() {
    DEBUG_MODE_ENABLED.store(false, Ordering::SeqCst);
}

/// Check if debug mode is enabled globally.
#[must_use]
pub fn is_debug_mode_enabled() -> bool {
    DEBUG_MODE_ENABLED.load(Ordering::SeqCst)
}

/// Debug configuration for secure debug mode access.
///
/// This allows configuring a debug header that must be present with a
/// secret token for debug information to be included in responses.
///
/// # Example
///
/// ```
/// use fastapi_core::error::DebugConfig;
///
/// // Require X-Debug-Token header with a secret
/// let config = DebugConfig::new()
///     .with_debug_header("X-Debug-Token", "my-secret-token");
///
/// // Or allow debug mode without authentication (dangerous!)
/// let config = DebugConfig::new().allow_unauthenticated();
/// ```
#[derive(Debug, Clone)]
pub struct DebugConfig {
    /// Enable debug mode for the application.
    pub enabled: bool,
    /// Header name for debug token authentication.
    pub debug_header: Option<String>,
    /// Expected token value for debug access.
    pub debug_token: Option<String>,
    /// Allow debug mode without authentication (dangerous in production).
    pub allow_unauthenticated: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            debug_header: None,
            debug_token: None,
            allow_unauthenticated: false,
        }
    }
}

impl DebugConfig {
    /// Create a new debug configuration with debug mode disabled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable debug mode.
    #[must_use]
    pub fn enable(mut self) -> Self {
        self.enabled = true;
        self
    }

    /// Configure the debug header and token for authenticated debug access.
    ///
    /// When configured, debug information will only be included in responses
    /// if the request includes this header with the expected token value.
    #[must_use]
    pub fn with_debug_header(
        mut self,
        header_name: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        self.debug_header = Some(header_name.into());
        self.debug_token = Some(token.into());
        self
    }

    /// Allow debug mode without authentication.
    ///
    /// # Warning
    ///
    /// This is dangerous in production as it exposes internal details
    /// to anyone. Only use for development/testing.
    #[must_use]
    pub fn allow_unauthenticated(mut self) -> Self {
        self.allow_unauthenticated = true;
        self
    }

    /// Check if a request is authorized for debug mode.
    ///
    /// Returns true if:
    /// - Debug mode is disabled (debug info won't be shown anyway)
    /// - `allow_unauthenticated` is true
    /// - The request includes the correct debug header/token
    ///
    /// # Security
    ///
    /// Token comparison uses constant-time comparison to prevent timing attacks.
    pub fn is_authorized(&self, request_headers: &[(String, Vec<u8>)]) -> bool {
        if !self.enabled {
            return false;
        }

        if self.allow_unauthenticated {
            return true;
        }

        // Check for debug header
        if let (Some(header_name), Some(expected_token)) = (&self.debug_header, &self.debug_token) {
            for (name, value) in request_headers {
                if name.eq_ignore_ascii_case(header_name) {
                    if let Ok(token) = std::str::from_utf8(value) {
                        // Use constant-time comparison to prevent timing attacks
                        return constant_time_str_eq(token, expected_token);
                    }
                }
            }
        }

        false
    }
}

/// Constant-time string comparison to prevent timing attacks.
///
/// This function compares two strings in constant time (for same-length inputs)
/// to prevent timing-based side-channel attacks.
fn constant_time_str_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();

    if a_bytes.len() != b_bytes.len() {
        return false;
    }

    let diff = a_bytes
        .iter()
        .zip(b_bytes.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y));

    diff == 0
}

// ============================================================================
// Debug Information
// ============================================================================

/// Debug information to include in error responses when debug mode is enabled.
///
/// This struct holds diagnostic information that helps developers understand
/// where and why an error occurred. This information should NEVER be included
/// in production responses as it can leak sensitive implementation details.
///
/// # Fields
///
/// - `source_file`: The file where the error originated
/// - `source_line`: The line number in the source file
/// - `function_name`: The function or handler that generated the error
/// - `route_pattern`: The matched route pattern (e.g., "/users/{id}")
/// - `handler_name`: The name of the handler function
/// - `extra`: Additional key-value debug information
///
/// # Example
///
/// ```
/// use fastapi_core::error::DebugInfo;
///
/// let debug = DebugInfo::new()
///     .with_source_location("src/handlers/user.rs", 42, "get_user")
///     .with_route_pattern("/users/{id}")
///     .with_extra("user_id_received", "abc123");
/// ```
#[derive(Debug, Clone, Default, Serialize)]
pub struct DebugInfo {
    /// Source file path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    /// Source line number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_line: Option<u32>,
    /// Function or handler name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    /// Matched route pattern.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_pattern: Option<String>,
    /// Handler name (may differ from function_name for wrapped handlers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler_name: Option<String>,
    /// Additional debug context.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, String>,
}

impl DebugInfo {
    /// Create empty debug info.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the source location.
    #[must_use]
    pub fn with_source_location(
        mut self,
        file: impl Into<String>,
        line: u32,
        function: impl Into<String>,
    ) -> Self {
        self.source_file = Some(file.into());
        self.source_line = Some(line);
        self.function_name = Some(function.into());
        self
    }

    /// Set the route pattern.
    #[must_use]
    pub fn with_route_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.route_pattern = Some(pattern.into());
        self
    }

    /// Set the handler name.
    #[must_use]
    pub fn with_handler_name(mut self, name: impl Into<String>) -> Self {
        self.handler_name = Some(name.into());
        self
    }

    /// Add extra debug information.
    #[must_use]
    pub fn with_extra(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }

    /// Check if this debug info is empty (has no data).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source_file.is_none()
            && self.source_line.is_none()
            && self.function_name.is_none()
            && self.route_pattern.is_none()
            && self.handler_name.is_none()
            && self.extra.is_empty()
    }
}

/// Macro to capture the current source location for debug info.
///
/// # Example
///
/// ```
/// use fastapi_core::{debug_location, error::DebugInfo};
///
/// fn my_handler() -> DebugInfo {
///     debug_location!()
/// }
/// ```
#[macro_export]
macro_rules! debug_location {
    () => {
        $crate::error::DebugInfo::new().with_source_location(
            file!(),
            line!(),
            // Get a reasonable function name approximation
            module_path!(),
        )
    };
    ($func_name:expr) => {
        $crate::error::DebugInfo::new().with_source_location(file!(), line!(), $func_name)
    };
}

// ============================================================================
// Location Types for Validation Errors (FastAPI-compatible)
// ============================================================================

/// Location item for validation error paths.
///
/// FastAPI uses tuples where items can be either strings (field names)
/// or integers (array indices). This enum models that structure.
///
/// # Examples
///
/// ```
/// use fastapi_core::error::LocItem;
///
/// // Field name
/// let field = LocItem::field("email");
/// assert_eq!(field.as_str(), Some("email"));
///
/// // Array index
/// let idx = LocItem::index(0);
/// assert_eq!(idx.as_index(), Some(0));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocItem {
    /// Field name (string).
    Field(String),
    /// Array index (integer).
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
    pub fn index(idx: usize) -> Self {
        Self::Index(idx)
    }

    /// Get the field name if this is a Field variant.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Field(s) => Some(s),
            Self::Index(_) => None,
        }
    }

    /// Get the index if this is an Index variant.
    #[must_use]
    pub fn as_index(&self) -> Option<usize> {
        match self {
            Self::Field(_) => None,
            Self::Index(i) => Some(*i),
        }
    }
}

impl From<&str> for LocItem {
    fn from(s: &str) -> Self {
        Self::Field(s.to_owned())
    }
}

impl From<String> for LocItem {
    fn from(s: String) -> Self {
        Self::Field(s)
    }
}

impl From<usize> for LocItem {
    fn from(i: usize) -> Self {
        Self::Index(i)
    }
}

impl Serialize for LocItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Field(s) => serializer.serialize_str(s),
            Self::Index(i) => serializer.serialize_u64(*i as u64),
        }
    }
}

// ============================================================================
// Location Prefixes (FastAPI conventions)
// ============================================================================

/// Location prefixes for different extraction sources.
pub mod loc {
    use super::LocItem;

    /// Path parameter location: `["path", "param_name"]`
    #[must_use]
    pub fn path(param: &str) -> Vec<LocItem> {
        vec![LocItem::field("path"), LocItem::field(param)]
    }

    /// Query parameter location: `["query", "param_name"]`
    #[must_use]
    pub fn query(param: &str) -> Vec<LocItem> {
        vec![LocItem::field("query"), LocItem::field(param)]
    }

    /// Header location: `["header", "header_name"]`
    #[must_use]
    pub fn header(name: &str) -> Vec<LocItem> {
        vec![LocItem::field("header"), LocItem::field(name)]
    }

    /// Cookie location: `["cookie", "cookie_name"]`
    #[must_use]
    pub fn cookie(name: &str) -> Vec<LocItem> {
        vec![LocItem::field("cookie"), LocItem::field(name)]
    }

    /// Body root location: `["body"]`
    #[must_use]
    pub fn body() -> Vec<LocItem> {
        vec![LocItem::field("body")]
    }

    /// Body field location: `["body", "field"]`
    #[must_use]
    pub fn body_field(field: &str) -> Vec<LocItem> {
        vec![LocItem::field("body"), LocItem::field(field)]
    }

    /// Body nested path: `["body", "field", "nested", ...]`
    #[must_use]
    pub fn body_path(fields: &[&str]) -> Vec<LocItem> {
        let mut loc = vec![LocItem::field("body")];
        for field in fields {
            loc.push(LocItem::field(*field));
        }
        loc
    }

    /// Body with array index: `["body", "items", 0, "name"]`
    #[must_use]
    pub fn body_indexed(field: &str, idx: usize) -> Vec<LocItem> {
        vec![
            LocItem::field("body"),
            LocItem::field(field),
            LocItem::index(idx),
        ]
    }

    /// Response root location: `["response"]`
    #[must_use]
    pub fn response() -> Vec<LocItem> {
        vec![LocItem::field("response")]
    }

    /// Response field location: `["response", "field"]`
    #[must_use]
    pub fn response_field(field: &str) -> Vec<LocItem> {
        vec![LocItem::field("response"), LocItem::field(field)]
    }

    /// Response nested path: `["response", "field", "nested", ...]`
    #[must_use]
    pub fn response_path(fields: &[&str]) -> Vec<LocItem> {
        let mut loc = vec![LocItem::field("response")];
        for field in fields {
            loc.push(LocItem::field(*field));
        }
        loc
    }
}

// ============================================================================
// Common Validation Error Types (FastAPI/Pydantic conventions)
// ============================================================================

/// Common validation error type strings (matching Pydantic v2).
pub mod error_types {
    /// Required field is missing.
    pub const MISSING: &str = "missing";
    /// String is too short.
    pub const STRING_TOO_SHORT: &str = "string_too_short";
    /// String is too long.
    pub const STRING_TOO_LONG: &str = "string_too_long";
    /// Value is not a valid string.
    pub const STRING_TYPE: &str = "string_type";
    /// Value is not a valid integer.
    pub const INT_TYPE: &str = "int_type";
    /// Value is not a valid float.
    pub const FLOAT_TYPE: &str = "float_type";
    /// Value is not a valid boolean.
    pub const BOOL_TYPE: &str = "bool_type";
    /// Value is less than minimum.
    pub const GREATER_THAN_EQUAL: &str = "greater_than_equal";
    /// Value is greater than maximum.
    pub const LESS_THAN_EQUAL: &str = "less_than_equal";
    /// Value does not match pattern.
    pub const STRING_PATTERN_MISMATCH: &str = "string_pattern_mismatch";
    /// Value is not a valid email.
    pub const VALUE_ERROR: &str = "value_error";
    /// Value is not a valid URL.
    pub const URL_TYPE: &str = "url_type";
    /// Value is not a valid UUID.
    pub const UUID_TYPE: &str = "uuid_type";
    /// JSON parsing failed.
    pub const JSON_INVALID: &str = "json_invalid";
    /// JSON type mismatch.
    pub const JSON_TYPE: &str = "json_type";
    /// Array has too few items.
    pub const TOO_SHORT: &str = "too_short";
    /// Array has too many items.
    pub const TOO_LONG: &str = "too_long";
    /// Value is not in enum.
    pub const ENUM: &str = "enum";
    /// Extra field not allowed.
    pub const EXTRA_FORBIDDEN: &str = "extra_forbidden";

    // Response validation error types
    /// Response failed to serialize (e.g., JSON serialization error).
    pub const SERIALIZATION_ERROR: &str = "serialization_error";
    /// Response doesn't match the declared response model.
    pub const MODEL_VALIDATION_ERROR: &str = "model_validation_error";
}

// ============================================================================
// HTTP Error
// ============================================================================

/// HTTP error that produces a response.
///
/// When debug mode is enabled, errors can include additional diagnostic
/// information via the `debug_info` field. This information is conditionally
/// serialized into the response when `is_debug_mode_enabled()` returns true.
///
/// # Production Mode (default)
///
/// ```json
/// {"detail": "Not Found"}
/// ```
///
/// # Debug Mode (when enabled)
///
/// ```json
/// {
///   "detail": "Not Found",
///   "debug": {
///     "source_file": "src/handlers/user.rs",
///     "source_line": 42,
///     "function_name": "get_user",
///     "route_pattern": "/users/{id}"
///   }
/// }
/// ```
#[derive(Debug)]
pub struct HttpError {
    /// Status code.
    pub status: StatusCode,
    /// Error detail message.
    pub detail: Option<String>,
    /// Additional headers.
    pub headers: Vec<(String, Vec<u8>)>,
    /// Debug information (only included in response when debug mode is enabled).
    pub debug_info: Option<DebugInfo>,
}

impl HttpError {
    /// Create a new HTTP error.
    #[must_use]
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            detail: None,
            headers: Vec::new(),
            debug_info: None,
        }
    }

    /// Add a detail message.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Add a header.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Add debug information.
    ///
    /// This information will only be included in the response when debug mode
    /// is enabled (via `enable_debug_mode()`).
    ///
    /// # Example
    ///
    /// ```
    /// use fastapi_core::error::{HttpError, DebugInfo};
    ///
    /// let error = HttpError::not_found()
    ///     .with_detail("User not found")
    ///     .with_debug_info(DebugInfo::new()
    ///         .with_source_location("src/handlers/user.rs", 42, "get_user")
    ///         .with_route_pattern("/users/{id}"));
    /// ```
    #[must_use]
    pub fn with_debug_info(mut self, debug_info: DebugInfo) -> Self {
        self.debug_info = Some(debug_info);
        self
    }

    /// Add debug information at the current source location.
    ///
    /// This is a convenience method that captures the current file and line.
    /// Note: This captures the location where this method is called, not
    /// where the error originated. For accurate source tracking, use
    /// `with_debug_info` with a `DebugInfo` created via the `debug_location!` macro.
    #[must_use]
    pub fn with_debug_location(self, function_name: impl Into<String>) -> Self {
        self.with_debug_info(DebugInfo::new().with_source_location(
            std::any::type_name::<Self>(),
            0, // Line unknown when called this way
            function_name,
        ))
    }

    /// Create a 400 Bad Request error.
    #[must_use]
    pub fn bad_request() -> Self {
        Self::new(StatusCode::BAD_REQUEST)
    }

    /// Create a 401 Unauthorized error.
    #[must_use]
    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED)
    }

    /// Create a 403 Forbidden error.
    #[must_use]
    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN)
    }

    /// Create a 404 Not Found error.
    #[must_use]
    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND)
    }

    /// Create a 500 Internal Server Error.
    #[must_use]
    pub fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Create a 413 Payload Too Large error.
    #[must_use]
    pub fn payload_too_large() -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE)
    }

    /// Create a 415 Unsupported Media Type error.
    #[must_use]
    pub fn unsupported_media_type() -> Self {
        Self::new(StatusCode::UNSUPPORTED_MEDIA_TYPE)
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let detail = self
            .detail
            .as_deref()
            .unwrap_or_else(|| self.status.canonical_reason());

        // Conditionally include debug info based on global debug mode flag
        let body = if is_debug_mode_enabled() {
            if let Some(ref debug_info) = self.debug_info {
                #[derive(Serialize)]
                struct ErrorBodyWithDebug<'a> {
                    detail: &'a str,
                    debug: &'a DebugInfo,
                }
                serde_json::to_vec(&ErrorBodyWithDebug {
                    detail,
                    debug: debug_info,
                })
                .unwrap_or_default()
            } else {
                #[derive(Serialize)]
                struct ErrorBody<'a> {
                    detail: &'a str,
                }
                serde_json::to_vec(&ErrorBody { detail }).unwrap_or_default()
            }
        } else {
            #[derive(Serialize)]
            struct ErrorBody<'a> {
                detail: &'a str,
            }
            serde_json::to_vec(&ErrorBody { detail }).unwrap_or_default()
        };

        let mut response = Response::with_status(self.status)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body));

        for (name, value) in self.headers {
            response = response.header(name, value);
        }

        response
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.status.canonical_reason())?;
        if let Some(ref detail) = self.detail {
            write!(f, ": {detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for HttpError {}

// ============================================================================
// Validation Error (FastAPI-compatible)
// ============================================================================

/// A single validation error item.
///
/// This structure matches FastAPI/Pydantic v2's validation error format exactly,
/// allowing for seamless compatibility with clients expecting FastAPI responses.
///
/// # Examples
///
/// ```
/// use fastapi_core::error::{ValidationError, loc, error_types};
/// use serde_json::json;
///
/// // Missing required field
/// let error = ValidationError::missing(loc::query("q"));
/// assert_eq!(error.error_type, "missing");
///
/// // String too short with context
/// let error = ValidationError::new(error_types::STRING_TOO_SHORT, loc::body_field("name"))
///     .with_msg("String should have at least 3 characters")
///     .with_input(json!("ab"))
///     .with_ctx_value("min_length", json!(3));
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct ValidationError {
    /// Error type identifier (e.g., "missing", "string_too_short").
    #[serde(rename = "type")]
    pub error_type: &'static str,

    /// Location path as a tuple of strings and integers.
    ///
    /// Examples:
    /// - `["query", "q"]` for query parameter
    /// - `["body", "user", "email"]` for nested body field
    /// - `["body", "items", 0, "name"]` for array item
    pub loc: Vec<LocItem>,

    /// Human-readable error message.
    pub msg: String,

    /// The input value that failed validation.
    ///
    /// This is the actual value the user provided that caused the error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,

    /// Additional context about the validation constraint.
    ///
    /// Examples:
    /// - `{"min_length": 3}` for string_too_short
    /// - `{"ge": 0}` for greater_than_equal
    /// - `{"pattern": "^\\d+$"}` for pattern mismatch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctx: Option<HashMap<String, serde_json::Value>>,
}

impl ValidationError {
    /// Create a new validation error.
    #[must_use]
    pub fn new(error_type: &'static str, loc: Vec<LocItem>) -> Self {
        Self {
            error_type,
            loc,
            msg: Self::default_message(error_type),
            input: None,
            ctx: None,
        }
    }

    /// Create a "missing" error for a required field.
    #[must_use]
    pub fn missing(loc: Vec<LocItem>) -> Self {
        Self::new(error_types::MISSING, loc)
    }

    /// Create a "string_too_short" error.
    #[must_use]
    pub fn string_too_short(loc: Vec<LocItem>, min_length: usize) -> Self {
        Self::new(error_types::STRING_TOO_SHORT, loc)
            .with_msg(format!(
                "String should have at least {min_length} character{}",
                if min_length == 1 { "" } else { "s" }
            ))
            .with_ctx_value("min_length", serde_json::json!(min_length))
    }

    /// Create a "string_too_long" error.
    #[must_use]
    pub fn string_too_long(loc: Vec<LocItem>, max_length: usize) -> Self {
        Self::new(error_types::STRING_TOO_LONG, loc)
            .with_msg(format!(
                "String should have at most {max_length} character{}",
                if max_length == 1 { "" } else { "s" }
            ))
            .with_ctx_value("max_length", serde_json::json!(max_length))
    }

    /// Create a type error (e.g., expected int, got string).
    #[must_use]
    pub fn type_error(loc: Vec<LocItem>, expected_type: &'static str) -> Self {
        let error_type = match expected_type {
            "string" => error_types::STRING_TYPE,
            "int" | "integer" => error_types::INT_TYPE,
            "float" | "number" => error_types::FLOAT_TYPE,
            "bool" | "boolean" => error_types::BOOL_TYPE,
            _ => error_types::VALUE_ERROR,
        };
        Self::new(error_type, loc).with_msg(format!("Input should be a valid {expected_type}"))
    }

    /// Create a JSON parsing error.
    #[must_use]
    pub fn json_invalid(loc: Vec<LocItem>, message: impl Into<String>) -> Self {
        Self::new(error_types::JSON_INVALID, loc).with_msg(message)
    }

    /// Set the human-readable message.
    #[must_use]
    pub fn with_msg(mut self, msg: impl Into<String>) -> Self {
        self.msg = msg.into();
        self
    }

    /// Set the input value.
    #[must_use]
    pub fn with_input(mut self, input: serde_json::Value) -> Self {
        self.input = Some(input);
        self
    }

    /// Add a context key-value pair.
    #[must_use]
    pub fn with_ctx_value(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.ctx
            .get_or_insert_with(HashMap::new)
            .insert(key.into(), value);
        self
    }

    /// Set the full context map.
    #[must_use]
    pub fn with_ctx(mut self, ctx: HashMap<String, serde_json::Value>) -> Self {
        self.ctx = Some(ctx);
        self
    }

    /// Add location items to the path.
    #[must_use]
    pub fn with_loc_prefix(mut self, prefix: Vec<LocItem>) -> Self {
        let mut new_loc = prefix;
        new_loc.extend(self.loc);
        self.loc = new_loc;
        self
    }

    /// Append a location item to the path.
    #[must_use]
    pub fn with_loc_suffix(mut self, item: impl Into<LocItem>) -> Self {
        self.loc.push(item.into());
        self
    }

    /// Get the default message for an error type.
    fn default_message(error_type: &str) -> String {
        match error_type {
            error_types::MISSING => "Field required".to_owned(),
            error_types::STRING_TOO_SHORT => "String too short".to_owned(),
            error_types::STRING_TOO_LONG => "String too long".to_owned(),
            error_types::STRING_TYPE => "Input should be a valid string".to_owned(),
            error_types::INT_TYPE => "Input should be a valid integer".to_owned(),
            error_types::FLOAT_TYPE => "Input should be a valid number".to_owned(),
            error_types::BOOL_TYPE => "Input should be a valid boolean".to_owned(),
            error_types::JSON_INVALID => "Invalid JSON".to_owned(),
            error_types::VALUE_ERROR => "Value error".to_owned(),
            _ => "Validation error".to_owned(),
        }
    }
}

// ============================================================================
// Validation Errors Collection
// ============================================================================

/// Collection of validation errors (422 Unprocessable Entity response).
///
/// This collects multiple validation errors from extractors and validators,
/// producing a FastAPI-compatible error response format.
///
/// # Examples
///
/// ```
/// use fastapi_core::error::{ValidationErrors, ValidationError, loc, error_types};
/// use serde_json::json;
///
/// let mut errors = ValidationErrors::new();
///
/// // Add multiple errors
/// errors.push(ValidationError::missing(loc::query("q")));
/// errors.push(ValidationError::string_too_short(loc::body_field("name"), 3)
///     .with_input(json!("ab")));
///
/// // Check and convert
/// if !errors.is_empty() {
///     let json = errors.to_json();
///     assert!(json.contains("missing"));
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ValidationErrors {
    /// The collected errors.
    pub errors: Vec<ValidationError>,
    /// The original input body (if available).
    pub body: Option<serde_json::Value>,
    /// Debug information (only included in response when debug mode is enabled).
    pub debug_info: Option<DebugInfo>,
}

impl ValidationErrors {
    /// Create empty validation errors.
    #[must_use]
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            body: None,
            debug_info: None,
        }
    }

    /// Create from a single error.
    #[must_use]
    pub fn single(error: ValidationError) -> Self {
        Self {
            errors: vec![error],
            body: None,
            debug_info: None,
        }
    }

    /// Create from multiple errors.
    #[must_use]
    pub fn from_errors(errors: Vec<ValidationError>) -> Self {
        Self {
            errors,
            body: None,
            debug_info: None,
        }
    }

    /// Add an error.
    pub fn push(&mut self, error: ValidationError) {
        self.errors.push(error);
    }

    /// Add multiple errors.
    pub fn extend(&mut self, errors: impl IntoIterator<Item = ValidationError>) {
        self.errors.extend(errors);
    }

    /// Set the original body that failed validation.
    #[must_use]
    pub fn with_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }

    /// Add debug information.
    ///
    /// This information will only be included in the response when debug mode
    /// is enabled (via `enable_debug_mode()`).
    #[must_use]
    pub fn with_debug_info(mut self, debug_info: DebugInfo) -> Self {
        self.debug_info = Some(debug_info);
        self
    }

    /// Check if there are any errors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// Get the number of errors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.errors.len()
    }

    /// Get an iterator over the errors.
    pub fn iter(&self) -> impl Iterator<Item = &ValidationError> {
        self.errors.iter()
    }

    /// Convert to FastAPI-compatible JSON string.
    #[must_use]
    pub fn to_json(&self) -> String {
        #[derive(Serialize)]
        struct Body<'a> {
            detail: &'a [ValidationError],
        }

        serde_json::to_string(&Body {
            detail: &self.errors,
        })
        .unwrap_or_else(|_| r#"{"detail":[]}"#.to_owned())
    }

    /// Convert to JSON bytes.
    #[must_use]
    pub fn to_json_bytes(&self) -> Vec<u8> {
        #[derive(Serialize)]
        struct Body<'a> {
            detail: &'a [ValidationError],
        }

        serde_json::to_vec(&Body {
            detail: &self.errors,
        })
        .unwrap_or_else(|_| b"{\"detail\":[]}".to_vec())
    }

    /// Merge another ValidationErrors into this one.
    pub fn merge(&mut self, other: ValidationErrors) {
        self.errors.extend(other.errors);
        if self.body.is_none() {
            self.body = other.body;
        }
        if self.debug_info.is_none() {
            self.debug_info = other.debug_info;
        }
    }

    /// Add a prefix to all error locations.
    #[must_use]
    pub fn with_loc_prefix(mut self, prefix: Vec<LocItem>) -> Self {
        for error in &mut self.errors {
            let mut new_loc = prefix.clone();
            new_loc.extend(std::mem::take(&mut error.loc));
            error.loc = new_loc;
        }
        self
    }
}

impl IntoIterator for ValidationErrors {
    type Item = ValidationError;
    type IntoIter = std::vec::IntoIter<ValidationError>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.into_iter()
    }
}

impl<'a> IntoIterator for &'a ValidationErrors {
    type Item = &'a ValidationError;
    type IntoIter = std::slice::Iter<'a, ValidationError>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.iter()
    }
}

impl Extend<ValidationError> for ValidationErrors {
    fn extend<T: IntoIterator<Item = ValidationError>>(&mut self, iter: T) {
        self.errors.extend(iter);
    }
}

impl FromIterator<ValidationError> for ValidationErrors {
    fn from_iter<T: IntoIterator<Item = ValidationError>>(iter: T) -> Self {
        Self::from_errors(iter.into_iter().collect())
    }
}

impl std::fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} validation error", self.errors.len())?;
        if self.errors.len() != 1 {
            write!(f, "s")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

impl IntoResponse for ValidationErrors {
    fn into_response(self) -> Response {
        // Conditionally include debug info based on global debug mode flag
        let body = if is_debug_mode_enabled() {
            if let Some(ref debug_info) = self.debug_info {
                #[derive(Serialize)]
                struct BodyWithDebug<'a> {
                    detail: &'a [ValidationError],
                    debug: &'a DebugInfo,
                }
                serde_json::to_vec(&BodyWithDebug {
                    detail: &self.errors,
                    debug: debug_info,
                })
                .unwrap_or_else(|_| b"{\"detail\":[]}".to_vec())
            } else {
                self.to_json_bytes()
            }
        } else {
            self.to_json_bytes()
        };

        Response::with_status(StatusCode::UNPROCESSABLE_ENTITY)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body))
    }
}

// ============================================================================
// Response Validation Error (500 Internal Server Error)
// ============================================================================

/// Response validation error for internal failures (500 Internal Server Error).
///
/// This error type is used when a response fails to serialize or validate
/// against the expected response model. Unlike [`ValidationErrors`] which
/// indicates client errors (422), this represents a bug in the server code
/// and returns a 500 status.
///
/// # When This Is Used
///
/// - Response fails to serialize to JSON
/// - Response doesn't match the declared response_model
/// - Internal data transformation fails
///
/// # Security Note
///
/// Error details and the original response content are logged server-side but
/// are NOT exposed to clients (unless debug mode is explicitly enabled).
/// This prevents leaking internal implementation details.
///
/// # Examples
///
/// ```
/// use fastapi_core::error::{ResponseValidationError, ValidationError, loc};
///
/// // Serialization failure
/// let error = ResponseValidationError::serialization_failed(
///     "failed to serialize field 'created_at': invalid date format"
/// );
///
/// // Response model validation failure
/// let error = ResponseValidationError::new()
///     .with_error(ValidationError::missing(loc::response_field("user_id")))
///     .with_response_content(serde_json::json!({"name": "Alice"}));
/// ```
#[derive(Debug, Clone, Default)]
pub struct ResponseValidationError {
    /// The validation errors that occurred.
    pub errors: Vec<ValidationError>,
    /// The response content that failed validation (for logging only).
    /// This is NOT included in the response to clients.
    pub response_content: Option<serde_json::Value>,
    /// A summary message for logging.
    pub summary: Option<String>,
    /// Debug information (only included in response when debug mode is enabled).
    pub debug_info: Option<DebugInfo>,
}

impl ResponseValidationError {
    /// Create a new empty response validation error.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a serialization failure error.
    ///
    /// Use this when response serialization to JSON fails.
    #[must_use]
    pub fn serialization_failed(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            errors: vec![
                ValidationError::new(
                    error_types::SERIALIZATION_ERROR,
                    vec![LocItem::field("response")],
                )
                .with_msg(&msg),
            ],
            response_content: None,
            summary: Some(msg),
            debug_info: None,
        }
    }

    /// Create a response model validation failure error.
    ///
    /// Use this when the response doesn't match the declared response_model.
    #[must_use]
    pub fn model_validation_failed(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            errors: vec![
                ValidationError::new(
                    error_types::MODEL_VALIDATION_ERROR,
                    vec![LocItem::field("response")],
                )
                .with_msg(&msg),
            ],
            response_content: None,
            summary: Some(msg),
            debug_info: None,
        }
    }

    /// Add a validation error.
    #[must_use]
    pub fn with_error(mut self, error: ValidationError) -> Self {
        self.errors.push(error);
        self
    }

    /// Add multiple validation errors.
    #[must_use]
    pub fn with_errors(mut self, errors: impl IntoIterator<Item = ValidationError>) -> Self {
        self.errors.extend(errors);
        self
    }

    /// Set the response content that failed validation.
    ///
    /// This is logged server-side but NOT exposed to clients.
    #[must_use]
    pub fn with_response_content(mut self, content: serde_json::Value) -> Self {
        self.response_content = Some(content);
        self
    }

    /// Set a summary message for logging.
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Add debug information.
    ///
    /// This information will only be included in the response when debug mode
    /// is enabled (via `enable_debug_mode()`).
    #[must_use]
    pub fn with_debug_info(mut self, debug_info: DebugInfo) -> Self {
        self.debug_info = Some(debug_info);
        self
    }

    /// Check if there are no errors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// Get the number of errors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.errors.len()
    }

    /// Get an iterator over the errors.
    pub fn iter(&self) -> impl Iterator<Item = &ValidationError> {
        self.errors.iter()
    }

    /// Log the error details (for server-side logging).
    ///
    /// This returns a formatted string suitable for logging that includes
    /// the error details and response content (if any).
    #[must_use]
    pub fn to_log_string(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref summary) = self.summary {
            parts.push(format!("Summary: {}", summary));
        }

        parts.push(format!("Errors ({}): ", self.errors.len()));
        for (i, error) in self.errors.iter().enumerate() {
            let loc_str: Vec<String> = error
                .loc
                .iter()
                .map(|item| match item {
                    LocItem::Field(s) => s.clone(),
                    LocItem::Index(i) => i.to_string(),
                })
                .collect();
            parts.push(format!(
                "  [{}] {} at [{}]: {}",
                i + 1,
                error.error_type,
                loc_str.join("."),
                error.msg
            ));
        }

        if let Some(ref content) = self.response_content {
            // Truncate large content for logging (UTF-8 safe)
            let content_str = serde_json::to_string(content).unwrap_or_default();
            let truncated = if content_str.len() > 500 {
                // Find a safe UTF-8 boundary near 500 chars
                let mut end = 500;
                while end > 0 && !content_str.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...(truncated)", &content_str[..end])
            } else {
                content_str
            };
            parts.push(format!("Response content: {}", truncated));
        }

        parts.join("\n")
    }
}

impl std::fmt::Display for ResponseValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Response validation failed")?;
        if let Some(ref summary) = self.summary {
            write!(f, ": {}", summary)?;
        }
        Ok(())
    }
}

impl std::error::Error for ResponseValidationError {}

impl IntoResponse for ResponseValidationError {
    fn into_response(self) -> Response {
        // Always log the full error details server-side
        // In a real application, this would use the logging system
        // For now, we include it in debug_info if debug mode is enabled

        // Build the response body
        let body = if is_debug_mode_enabled() {
            // In debug mode, include error details
            #[derive(Serialize)]
            struct DebugBody<'a> {
                error: &'static str,
                detail: &'static str,
                #[serde(skip_serializing_if = "Option::is_none")]
                debug: Option<DebugResponseInfo<'a>>,
            }

            #[derive(Serialize)]
            struct DebugResponseInfo<'a> {
                #[serde(skip_serializing_if = "Option::is_none")]
                summary: Option<&'a str>,
                errors: &'a [ValidationError],
                #[serde(skip_serializing_if = "Option::is_none")]
                response_content: &'a Option<serde_json::Value>,
                #[serde(skip_serializing_if = "Option::is_none")]
                source: Option<&'a DebugInfo>,
            }

            let debug_info = DebugResponseInfo {
                summary: self.summary.as_deref(),
                errors: &self.errors,
                response_content: &self.response_content,
                source: self.debug_info.as_ref(),
            };

            serde_json::to_vec(&DebugBody {
                error: "internal_server_error",
                detail: "Response validation failed",
                debug: Some(debug_info),
            })
            .unwrap_or_else(|_| {
                b"{\"error\":\"internal_server_error\",\"detail\":\"Internal Server Error\"}"
                    .to_vec()
            })
        } else {
            // In production, return a generic error message
            b"{\"error\":\"internal_server_error\",\"detail\":\"Internal Server Error\"}".to_vec()
        };

        Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========================================================================
    // LocItem Tests
    // ========================================================================

    #[test]
    fn loc_item_field_creation() {
        let item = LocItem::field("email");
        assert_eq!(item.as_str(), Some("email"));
        assert_eq!(item.as_index(), None);
    }

    #[test]
    fn loc_item_index_creation() {
        let item = LocItem::index(42);
        assert_eq!(item.as_str(), None);
        assert_eq!(item.as_index(), Some(42));
    }

    #[test]
    fn loc_item_from_str() {
        let item: LocItem = "name".into();
        assert_eq!(item, LocItem::Field("name".to_owned()));
    }

    #[test]
    fn loc_item_from_string() {
        let item: LocItem = String::from("age").into();
        assert_eq!(item, LocItem::Field("age".to_owned()));
    }

    #[test]
    fn loc_item_from_usize() {
        let item: LocItem = 5usize.into();
        assert_eq!(item, LocItem::Index(5));
    }

    #[test]
    fn loc_item_serialize_field() {
        let item = LocItem::field("email");
        let json = serde_json::to_string(&item).unwrap();
        assert_eq!(json, "\"email\"");
    }

    #[test]
    fn loc_item_serialize_index() {
        let item = LocItem::index(3);
        let json = serde_json::to_string(&item).unwrap();
        assert_eq!(json, "3");
    }

    // ========================================================================
    // Location Helper Tests
    // ========================================================================

    #[test]
    fn loc_path_creates_correct_location() {
        let loc = loc::path("user_id");
        assert_eq!(loc.len(), 2);
        assert_eq!(loc[0].as_str(), Some("path"));
        assert_eq!(loc[1].as_str(), Some("user_id"));
    }

    #[test]
    fn loc_query_creates_correct_location() {
        let loc = loc::query("q");
        assert_eq!(loc.len(), 2);
        assert_eq!(loc[0].as_str(), Some("query"));
        assert_eq!(loc[1].as_str(), Some("q"));
    }

    #[test]
    fn loc_header_creates_correct_location() {
        let loc = loc::header("Authorization");
        assert_eq!(loc.len(), 2);
        assert_eq!(loc[0].as_str(), Some("header"));
        assert_eq!(loc[1].as_str(), Some("Authorization"));
    }

    #[test]
    fn loc_cookie_creates_correct_location() {
        let loc = loc::cookie("session_id");
        assert_eq!(loc.len(), 2);
        assert_eq!(loc[0].as_str(), Some("cookie"));
        assert_eq!(loc[1].as_str(), Some("session_id"));
    }

    #[test]
    fn loc_body_creates_root_location() {
        let loc = loc::body();
        assert_eq!(loc.len(), 1);
        assert_eq!(loc[0].as_str(), Some("body"));
    }

    #[test]
    fn loc_body_field_creates_correct_location() {
        let loc = loc::body_field("email");
        assert_eq!(loc.len(), 2);
        assert_eq!(loc[0].as_str(), Some("body"));
        assert_eq!(loc[1].as_str(), Some("email"));
    }

    #[test]
    fn loc_body_path_creates_nested_location() {
        let loc = loc::body_path(&["user", "profile", "name"]);
        assert_eq!(loc.len(), 4);
        assert_eq!(loc[0].as_str(), Some("body"));
        assert_eq!(loc[1].as_str(), Some("user"));
        assert_eq!(loc[2].as_str(), Some("profile"));
        assert_eq!(loc[3].as_str(), Some("name"));
    }

    #[test]
    fn loc_body_indexed_creates_array_location() {
        let loc = loc::body_indexed("items", 0);
        assert_eq!(loc.len(), 3);
        assert_eq!(loc[0].as_str(), Some("body"));
        assert_eq!(loc[1].as_str(), Some("items"));
        assert_eq!(loc[2].as_index(), Some(0));
    }

    // ========================================================================
    // ValidationError Tests
    // ========================================================================

    #[test]
    fn validation_error_new_with_default_message() {
        let error = ValidationError::new(error_types::MISSING, loc::query("q"));
        assert_eq!(error.error_type, "missing");
        assert_eq!(error.msg, "Field required");
        assert!(error.input.is_none());
        assert!(error.ctx.is_none());
    }

    #[test]
    fn validation_error_missing() {
        let error = ValidationError::missing(loc::query("page"));
        assert_eq!(error.error_type, "missing");
        assert_eq!(error.msg, "Field required");
    }

    #[test]
    fn validation_error_string_too_short() {
        let error = ValidationError::string_too_short(loc::body_field("name"), 3);
        assert_eq!(error.error_type, "string_too_short");
        assert!(error.msg.contains("3"));
        assert!(error.ctx.is_some());
        let ctx = error.ctx.unwrap();
        assert_eq!(ctx.get("min_length"), Some(&json!(3)));
    }

    #[test]
    fn validation_error_string_too_long() {
        let error = ValidationError::string_too_long(loc::body_field("bio"), 500);
        assert_eq!(error.error_type, "string_too_long");
        assert!(error.msg.contains("500"));
        assert!(error.ctx.is_some());
        let ctx = error.ctx.unwrap();
        assert_eq!(ctx.get("max_length"), Some(&json!(500)));
    }

    #[test]
    fn validation_error_type_error_int() {
        let error = ValidationError::type_error(loc::query("count"), "integer");
        assert_eq!(error.error_type, "int_type");
        assert!(error.msg.contains("integer"));
    }

    #[test]
    fn validation_error_type_error_string() {
        let error = ValidationError::type_error(loc::body_field("name"), "string");
        assert_eq!(error.error_type, "string_type");
        assert!(error.msg.contains("string"));
    }

    #[test]
    fn validation_error_json_invalid() {
        let error = ValidationError::json_invalid(loc::body(), "unexpected end of input");
        assert_eq!(error.error_type, "json_invalid");
        assert_eq!(error.msg, "unexpected end of input");
    }

    #[test]
    fn validation_error_with_input() {
        let error = ValidationError::missing(loc::query("q")).with_input(json!(null));
        assert_eq!(error.input, Some(json!(null)));
    }

    #[test]
    fn validation_error_with_ctx_value() {
        let error = ValidationError::new(error_types::GREATER_THAN_EQUAL, loc::body_field("age"))
            .with_ctx_value("ge", json!(0));
        assert!(error.ctx.is_some());
        assert_eq!(error.ctx.unwrap().get("ge"), Some(&json!(0)));
    }

    #[test]
    fn validation_error_with_multiple_ctx_values() {
        let error = ValidationError::new(
            error_types::STRING_PATTERN_MISMATCH,
            loc::body_field("email"),
        )
        .with_ctx_value("pattern", json!("^.+@.+$"))
        .with_ctx_value("expected", json!("email format"));
        let ctx = error.ctx.unwrap();
        assert_eq!(ctx.len(), 2);
        assert_eq!(ctx.get("pattern"), Some(&json!("^.+@.+$")));
        assert_eq!(ctx.get("expected"), Some(&json!("email format")));
    }

    #[test]
    fn validation_error_with_loc_prefix() {
        let error = ValidationError::missing(vec![LocItem::field("email")])
            .with_loc_prefix(vec![LocItem::field("body"), LocItem::field("user")]);
        assert_eq!(error.loc.len(), 3);
        assert_eq!(error.loc[0].as_str(), Some("body"));
        assert_eq!(error.loc[1].as_str(), Some("user"));
        assert_eq!(error.loc[2].as_str(), Some("email"));
    }

    #[test]
    fn validation_error_with_loc_suffix() {
        let error = ValidationError::missing(loc::body())
            .with_loc_suffix("items")
            .with_loc_suffix(0usize)
            .with_loc_suffix("name");
        assert_eq!(error.loc.len(), 4);
        assert_eq!(error.loc[0].as_str(), Some("body"));
        assert_eq!(error.loc[1].as_str(), Some("items"));
        assert_eq!(error.loc[2].as_index(), Some(0));
        assert_eq!(error.loc[3].as_str(), Some("name"));
    }

    // ========================================================================
    // ValidationError Serialization Tests
    // ========================================================================

    #[test]
    fn validation_error_serializes_to_fastapi_format() {
        let error = ValidationError::missing(loc::query("q"));
        let json = serde_json::to_value(&error).unwrap();

        assert_eq!(json["type"], "missing");
        assert_eq!(json["loc"], json!(["query", "q"]));
        assert_eq!(json["msg"], "Field required");
        assert!(json.get("input").is_none()); // skip_serializing_if = None
        assert!(json.get("ctx").is_none());
    }

    #[test]
    fn validation_error_serializes_with_array_index() {
        let error = ValidationError::missing(vec![
            LocItem::field("body"),
            LocItem::field("items"),
            LocItem::index(2),
            LocItem::field("name"),
        ]);
        let json = serde_json::to_value(&error).unwrap();

        assert_eq!(json["loc"], json!(["body", "items", 2, "name"]));
    }

    #[test]
    fn validation_error_serializes_with_input_and_ctx() {
        let error =
            ValidationError::string_too_short(loc::body_field("name"), 3).with_input(json!("ab"));
        let json = serde_json::to_value(&error).unwrap();

        assert_eq!(json["input"], "ab");
        assert_eq!(json["ctx"]["min_length"], 3);
    }

    // ========================================================================
    // ValidationErrors Collection Tests
    // ========================================================================

    #[test]
    fn validation_errors_new_is_empty() {
        let errors = ValidationErrors::new();
        assert!(errors.is_empty());
        assert_eq!(errors.len(), 0);
    }

    #[test]
    fn validation_errors_single() {
        let errors = ValidationErrors::single(ValidationError::missing(loc::query("q")));
        assert!(!errors.is_empty());
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn validation_errors_push() {
        let mut errors = ValidationErrors::new();
        errors.push(ValidationError::missing(loc::query("q")));
        errors.push(ValidationError::missing(loc::query("page")));
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn validation_errors_extend() {
        let mut errors = ValidationErrors::new();
        errors.extend(vec![
            ValidationError::missing(loc::query("q")),
            ValidationError::missing(loc::query("page")),
        ]);
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn validation_errors_from_errors() {
        let errors = ValidationErrors::from_errors(vec![
            ValidationError::missing(loc::query("q")),
            ValidationError::string_too_short(loc::body_field("name"), 1),
        ]);
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn validation_errors_with_body() {
        let body = json!({"name": ""});
        let errors = ValidationErrors::single(ValidationError::string_too_short(
            loc::body_field("name"),
            1,
        ))
        .with_body(body.clone());

        assert_eq!(errors.body, Some(body));
    }

    #[test]
    fn validation_errors_merge() {
        let mut errors1 = ValidationErrors::single(ValidationError::missing(loc::query("q")));
        let errors2 = ValidationErrors::single(ValidationError::missing(loc::query("page")));

        errors1.merge(errors2);
        assert_eq!(errors1.len(), 2);
    }

    #[test]
    fn validation_errors_with_loc_prefix() {
        let errors = ValidationErrors::from_errors(vec![
            ValidationError::missing(vec![LocItem::field("name")]),
            ValidationError::missing(vec![LocItem::field("email")]),
        ])
        .with_loc_prefix(vec![LocItem::field("body"), LocItem::field("user")]);

        for error in &errors {
            assert_eq!(error.loc[0].as_str(), Some("body"));
            assert_eq!(error.loc[1].as_str(), Some("user"));
        }
    }

    #[test]
    fn validation_errors_iter() {
        let errors = ValidationErrors::from_errors(vec![
            ValidationError::missing(loc::query("q")),
            ValidationError::missing(loc::query("page")),
        ]);

        let count = errors.iter().count();
        assert_eq!(count, 2);
    }

    #[test]
    fn validation_errors_into_iter() {
        let errors = ValidationErrors::from_errors(vec![
            ValidationError::missing(loc::query("q")),
            ValidationError::missing(loc::query("page")),
        ]);

        let collected: Vec<_> = errors.into_iter().collect();
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn validation_errors_from_iterator() {
        let errors: ValidationErrors = vec![
            ValidationError::missing(loc::query("q")),
            ValidationError::missing(loc::query("page")),
        ]
        .into_iter()
        .collect();

        assert_eq!(errors.len(), 2);
    }

    // ========================================================================
    // ValidationErrors JSON Output Tests
    // ========================================================================

    #[test]
    fn validation_errors_to_json() {
        let errors = ValidationErrors::single(
            ValidationError::missing(loc::query("q")).with_input(json!(null)),
        );
        let json = errors.to_json();

        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["detail"].is_array());
        assert_eq!(parsed["detail"][0]["type"], "missing");
        assert_eq!(parsed["detail"][0]["loc"], json!(["query", "q"]));
    }

    #[test]
    fn validation_errors_to_json_bytes() {
        let errors = ValidationErrors::single(ValidationError::missing(loc::query("q")));
        let bytes = errors.to_json_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert!(json["detail"].is_array());
    }

    #[test]
    fn validation_errors_fastapi_format_match() {
        // This tests the exact format FastAPI/Pydantic v2 produces
        let errors = ValidationErrors::from_errors(vec![
            ValidationError::missing(loc::query("q")),
            ValidationError::string_too_short(loc::body_field("name"), 3).with_input(json!("ab")),
        ]);

        let json = errors.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // First error: missing query param
        assert_eq!(parsed["detail"][0]["type"], "missing");
        assert_eq!(parsed["detail"][0]["loc"], json!(["query", "q"]));
        assert_eq!(parsed["detail"][0]["msg"], "Field required");

        // Second error: string too short
        assert_eq!(parsed["detail"][1]["type"], "string_too_short");
        assert_eq!(parsed["detail"][1]["loc"], json!(["body", "name"]));
        assert_eq!(parsed["detail"][1]["input"], "ab");
        assert_eq!(parsed["detail"][1]["ctx"]["min_length"], 3);
    }

    #[test]
    fn validation_errors_nested_array_location() {
        // Test the complex nested case: body > items[0] > name
        let error = ValidationError::missing(vec![
            LocItem::field("body"),
            LocItem::field("items"),
            LocItem::index(0),
            LocItem::field("name"),
        ]);
        let errors = ValidationErrors::single(error);
        let json = errors.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(
            parsed["detail"][0]["loc"],
            json!(["body", "items", 0, "name"])
        );
    }

    // ========================================================================
    // IntoResponse Tests
    // ========================================================================

    #[test]
    fn validation_errors_into_response() {
        let errors = ValidationErrors::single(ValidationError::missing(loc::query("q")));
        let response = errors.into_response();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // Check content-type header
        let content_type = response
            .headers()
            .iter()
            .find(|(name, _): &&(String, Vec<u8>)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_slice());
        assert_eq!(content_type, Some(b"application/json".as_slice()));
    }

    // ========================================================================
    // Error Types Constants Tests
    // ========================================================================

    #[test]
    fn error_types_match_pydantic() {
        // Verify our constants match Pydantic v2 error types
        assert_eq!(error_types::MISSING, "missing");
        assert_eq!(error_types::STRING_TOO_SHORT, "string_too_short");
        assert_eq!(error_types::STRING_TOO_LONG, "string_too_long");
        assert_eq!(error_types::STRING_TYPE, "string_type");
        assert_eq!(error_types::INT_TYPE, "int_type");
        assert_eq!(error_types::FLOAT_TYPE, "float_type");
        assert_eq!(error_types::BOOL_TYPE, "bool_type");
        assert_eq!(error_types::JSON_INVALID, "json_invalid");
        assert_eq!(error_types::VALUE_ERROR, "value_error");
    }

    // ========================================================================
    // HttpError Tests
    // ========================================================================

    #[test]
    fn http_error_new_with_status() {
        let error = HttpError::new(StatusCode::NOT_FOUND);
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert!(error.detail.is_none());
        assert!(error.headers.is_empty());
    }

    #[test]
    fn http_error_bad_request() {
        let error = HttpError::bad_request();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.status.as_u16(), 400);
    }

    #[test]
    fn http_error_unauthorized() {
        let error = HttpError::unauthorized();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert_eq!(error.status.as_u16(), 401);
    }

    #[test]
    fn http_error_forbidden() {
        let error = HttpError::forbidden();
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(error.status.as_u16(), 403);
    }

    #[test]
    fn http_error_not_found() {
        let error = HttpError::not_found();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.status.as_u16(), 404);
    }

    #[test]
    fn http_error_internal() {
        let error = HttpError::internal();
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.status.as_u16(), 500);
    }

    #[test]
    fn http_error_payload_too_large() {
        let error = HttpError::payload_too_large();
        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error.status.as_u16(), 413);
    }

    #[test]
    fn http_error_unsupported_media_type() {
        let error = HttpError::unsupported_media_type();
        assert_eq!(error.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(error.status.as_u16(), 415);
    }

    #[test]
    fn http_error_with_detail() {
        let error = HttpError::not_found().with_detail("User not found");
        assert_eq!(error.detail, Some("User not found".to_owned()));
    }

    #[test]
    fn http_error_with_detail_owned_string() {
        let detail = String::from("Resource missing");
        let error = HttpError::not_found().with_detail(detail);
        assert_eq!(error.detail, Some("Resource missing".to_owned()));
    }

    #[test]
    fn http_error_with_header() {
        let error = HttpError::unauthorized()
            .with_header("WWW-Authenticate", b"Bearer realm=\"api\"".to_vec());
        assert_eq!(error.headers.len(), 1);
        assert_eq!(error.headers[0].0, "WWW-Authenticate");
        assert_eq!(error.headers[0].1, b"Bearer realm=\"api\"".to_vec());
    }

    #[test]
    fn http_error_with_multiple_headers() {
        let error = HttpError::bad_request()
            .with_header("X-Error-Code", b"E001".to_vec())
            .with_header("X-Error-Context", b"validation".to_vec())
            .with_header("Retry-After", b"60".to_vec());
        assert_eq!(error.headers.len(), 3);
    }

    #[test]
    fn http_error_with_detail_and_headers() {
        let error = HttpError::unauthorized()
            .with_detail("Invalid or expired token")
            .with_header("WWW-Authenticate", b"Bearer".to_vec())
            .with_header("X-Token-Expired", b"true".to_vec());

        assert_eq!(error.detail, Some("Invalid or expired token".to_owned()));
        assert_eq!(error.headers.len(), 2);
    }

    #[test]
    fn http_error_display_without_detail() {
        let error = HttpError::not_found();
        let display = format!("{}", error);
        assert_eq!(display, "Not Found");
    }

    #[test]
    fn http_error_display_with_detail() {
        let error = HttpError::not_found().with_detail("User 123 not found");
        let display = format!("{}", error);
        assert_eq!(display, "Not Found: User 123 not found");
    }

    #[test]
    fn http_error_is_error_trait() {
        let error: Box<dyn std::error::Error> = Box::new(HttpError::internal());
        // Just verify it compiles and we can use it as a trait object
        assert!(error.to_string().contains("Internal Server Error"));
    }

    #[test]
    fn http_error_into_response_status() {
        let error = HttpError::forbidden();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn http_error_into_response_json_content_type() {
        let error = HttpError::bad_request();
        let response = error.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_slice());
        assert_eq!(content_type, Some(b"application/json".as_slice()));
    }

    #[test]
    fn http_error_into_response_json_body_format() {
        let error = HttpError::not_found().with_detail("Resource not found");
        let response = error.into_response();

        // Extract body
        let body = match response.body_ref() {
            ResponseBody::Bytes(b) => b.clone(),
            _ => panic!("Expected bytes body"),
        };

        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["detail"], "Resource not found");
    }

    #[test]
    fn http_error_into_response_default_detail() {
        // When no detail is provided, should use canonical reason
        let error = HttpError::not_found();
        let response = error.into_response();

        let body = match response.body_ref() {
            ResponseBody::Bytes(b) => b.clone(),
            _ => panic!("Expected bytes body"),
        };

        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["detail"], "Not Found");
    }

    #[test]
    fn http_error_into_response_with_custom_headers() {
        let error = HttpError::unauthorized()
            .with_detail("Token expired")
            .with_header("WWW-Authenticate", b"Bearer realm=\"api\"".to_vec());

        let response = error.into_response();

        // Check custom header is present
        let www_auth = response
            .headers()
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("www-authenticate"))
            .map(|(_, v)| v.as_slice());
        assert_eq!(www_auth, Some(b"Bearer realm=\"api\"".as_slice()));
    }

    #[test]
    fn http_error_into_response_multiple_custom_headers() {
        let error = HttpError::bad_request()
            .with_header("X-Error-Code", b"VALIDATION_FAILED".to_vec())
            .with_header("X-Request-Id", b"abc-123".to_vec());

        let response = error.into_response();

        let headers: Vec<_> = response.headers().iter().collect();

        // Should have content-type plus our two custom headers
        assert!(headers.len() >= 3);

        let error_code = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-error-code"))
            .map(|(_, v)| v.as_slice());
        assert_eq!(error_code, Some(b"VALIDATION_FAILED".as_slice()));

        let request_id = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-request-id"))
            .map(|(_, v)| v.as_slice());
        assert_eq!(request_id, Some(b"abc-123".as_slice()));
    }

    #[test]
    fn http_error_response_body_is_valid_json() {
        // Test all status code variants produce valid JSON
        let errors = vec![
            HttpError::bad_request(),
            HttpError::unauthorized(),
            HttpError::forbidden(),
            HttpError::not_found(),
            HttpError::internal(),
            HttpError::payload_too_large(),
            HttpError::unsupported_media_type(),
        ];

        for error in errors {
            let status = error.status;
            let response = error.into_response();
            let body = match response.body_ref() {
                ResponseBody::Bytes(b) => b.clone(),
                _ => panic!("Expected bytes body"),
            };

            // Should parse as valid JSON
            let parsed: Result<serde_json::Value, _> = serde_json::from_slice(&body);
            assert!(
                parsed.is_ok(),
                "Failed to parse JSON for status {}: {:?}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            );

            // Should have detail field
            let json = parsed.unwrap();
            assert!(
                json.get("detail").is_some(),
                "Missing detail field for status {}",
                status.as_u16()
            );
        }
    }

    #[test]
    fn http_error_fastapi_compatible_format() {
        // Verify our error format matches FastAPI's HTTPException
        // FastAPI returns: {"detail": "message"}
        let error = HttpError::forbidden().with_detail("Insufficient permissions");
        let response = error.into_response();

        let body = match response.body_ref() {
            ResponseBody::Bytes(b) => b.clone(),
            _ => panic!("Expected bytes body"),
        };

        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Only "detail" key, no extra fields
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 1, "Expected only 'detail' field");
        assert_eq!(json["detail"], "Insufficient permissions");
    }

    #[test]
    fn http_error_chained_builder_pattern() {
        // Verify builder pattern works correctly with method chaining
        let error = HttpError::new(StatusCode::TOO_MANY_REQUESTS)
            .with_detail("Rate limit exceeded")
            .with_header("Retry-After", b"60".to_vec())
            .with_header("X-RateLimit-Remaining", b"0".to_vec());

        assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(error.detail, Some("Rate limit exceeded".to_owned()));
        assert_eq!(error.headers.len(), 2);
    }

    // ========================================================================
    // Additional Error Types Constants Tests
    // ========================================================================

    #[test]
    fn error_types_all_constants_defined() {
        // Verify all error type constants are non-empty
        assert!(!error_types::MISSING.is_empty());
        assert!(!error_types::STRING_TOO_SHORT.is_empty());
        assert!(!error_types::STRING_TOO_LONG.is_empty());
        assert!(!error_types::STRING_TYPE.is_empty());
        assert!(!error_types::INT_TYPE.is_empty());
        assert!(!error_types::FLOAT_TYPE.is_empty());
        assert!(!error_types::BOOL_TYPE.is_empty());
        assert!(!error_types::GREATER_THAN_EQUAL.is_empty());
        assert!(!error_types::LESS_THAN_EQUAL.is_empty());
        assert!(!error_types::STRING_PATTERN_MISMATCH.is_empty());
        assert!(!error_types::VALUE_ERROR.is_empty());
        assert!(!error_types::URL_TYPE.is_empty());
        assert!(!error_types::UUID_TYPE.is_empty());
        assert!(!error_types::JSON_INVALID.is_empty());
        assert!(!error_types::JSON_TYPE.is_empty());
        assert!(!error_types::TOO_SHORT.is_empty());
        assert!(!error_types::TOO_LONG.is_empty());
        assert!(!error_types::ENUM.is_empty());
        assert!(!error_types::EXTRA_FORBIDDEN.is_empty());
    }

    #[test]
    fn error_types_numeric_range_constants() {
        // Verify numeric range error types
        assert_eq!(error_types::GREATER_THAN_EQUAL, "greater_than_equal");
        assert_eq!(error_types::LESS_THAN_EQUAL, "less_than_equal");
    }

    #[test]
    fn error_types_collection_constants() {
        // Verify collection-related error types
        assert_eq!(error_types::TOO_SHORT, "too_short");
        assert_eq!(error_types::TOO_LONG, "too_long");
    }

    // ========================================================================
    // Edge Cases and Error Conditions
    // ========================================================================

    #[test]
    fn validation_error_empty_location() {
        let error = ValidationError::new(error_types::VALUE_ERROR, vec![]);
        assert!(error.loc.is_empty());

        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["loc"], json!([]));
    }

    #[test]
    fn validation_error_deeply_nested_location() {
        // Test very deeply nested location path
        let error = ValidationError::missing(vec![
            LocItem::field("body"),
            LocItem::field("data"),
            LocItem::field("users"),
            LocItem::index(0),
            LocItem::field("profile"),
            LocItem::field("settings"),
            LocItem::index(5),
            LocItem::field("value"),
        ]);

        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(
            json["loc"],
            json!([
                "body", "data", "users", 0, "profile", "settings", 5, "value"
            ])
        );
    }

    #[test]
    fn validation_errors_empty_to_json() {
        let errors = ValidationErrors::new();
        let json = errors.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["detail"], json!([]));
    }

    #[test]
    fn validation_errors_many_errors() {
        // Test with many errors to ensure performance
        let mut errors = ValidationErrors::new();
        for i in 0..100 {
            errors.push(ValidationError::missing(loc::query(&format!("param{}", i))));
        }

        assert_eq!(errors.len(), 100);

        let json = errors.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["detail"].as_array().unwrap().len(), 100);
    }

    #[test]
    fn validation_error_special_characters_in_field_name() {
        // Test field names with special characters
        let error = ValidationError::missing(vec![
            LocItem::field("body"),
            LocItem::field("user-name"),
            LocItem::field("email@domain"),
        ]);

        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["loc"], json!(["body", "user-name", "email@domain"]));
    }

    #[test]
    fn validation_error_unicode_in_message() {
        let error = ValidationError::new(error_types::VALUE_ERROR, loc::body_field("name"))
            .with_msg("名前が無効です");

        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["msg"], "名前が無効です");
    }

    #[test]
    fn validation_error_large_input_value() {
        // Test with large input value
        let large_string = "x".repeat(10000);
        let error = ValidationError::string_too_long(loc::body_field("bio"), 500)
            .with_input(json!(large_string));

        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(json["input"].as_str().unwrap().len(), 10000);
    }

    #[test]
    fn http_error_empty_detail() {
        // Explicitly setting empty string as detail
        let error = HttpError::bad_request().with_detail("");
        assert_eq!(error.detail, Some(String::new()));

        let response = error.into_response();
        let body = match response.body_ref() {
            ResponseBody::Bytes(b) => b.clone(),
            _ => panic!("Expected bytes body"),
        };

        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Empty detail should be used as-is
        assert_eq!(parsed["detail"], "");
    }

    #[test]
    fn http_error_binary_header_value() {
        // Test header with non-UTF8 bytes
        let error = HttpError::bad_request().with_header("X-Binary", vec![0x00, 0xFF, 0x80]);

        assert_eq!(error.headers[0].1, vec![0x00, 0xFF, 0x80]);
    }

    // ========================================================================
    // Debug Mode Tests
    // ========================================================================

    #[test]
    fn debug_mode_default_disabled() {
        // Ensure debug mode is disabled by default
        disable_debug_mode();
        assert!(!is_debug_mode_enabled());
    }

    #[test]
    fn debug_mode_can_be_enabled_and_disabled() {
        // Start disabled
        disable_debug_mode();
        assert!(!is_debug_mode_enabled());

        // Enable
        enable_debug_mode();
        assert!(is_debug_mode_enabled());

        // Disable again
        disable_debug_mode();
        assert!(!is_debug_mode_enabled());
    }

    #[test]
    fn debug_config_default() {
        let config = DebugConfig::default();
        assert!(!config.enabled);
        assert!(config.debug_header.is_none());
        assert!(config.debug_token.is_none());
        assert!(!config.allow_unauthenticated);
    }

    #[test]
    fn debug_config_builder() {
        let config = DebugConfig::new()
            .enable()
            .with_debug_header("X-Debug-Token", "secret123");

        assert!(config.enabled);
        assert_eq!(config.debug_header, Some("X-Debug-Token".to_owned()));
        assert_eq!(config.debug_token, Some("secret123".to_owned()));
        assert!(!config.allow_unauthenticated);
    }

    #[test]
    fn debug_config_allow_unauthenticated() {
        let config = DebugConfig::new().enable().allow_unauthenticated();

        assert!(config.enabled);
        assert!(config.allow_unauthenticated);
    }

    #[test]
    fn debug_config_is_authorized_when_disabled() {
        let config = DebugConfig::new();
        let headers: Vec<(String, Vec<u8>)> = vec![];

        // Not authorized when debug mode is disabled
        assert!(!config.is_authorized(&headers));
    }

    #[test]
    fn debug_config_is_authorized_unauthenticated() {
        let config = DebugConfig::new().enable().allow_unauthenticated();
        let headers: Vec<(String, Vec<u8>)> = vec![];

        // Authorized when allow_unauthenticated is true
        assert!(config.is_authorized(&headers));
    }

    #[test]
    fn debug_config_is_authorized_with_valid_token() {
        let config = DebugConfig::new()
            .enable()
            .with_debug_header("X-Debug-Token", "my-secret");

        let headers = vec![("X-Debug-Token".to_owned(), b"my-secret".to_vec())];

        assert!(config.is_authorized(&headers));
    }

    #[test]
    fn debug_config_is_authorized_with_invalid_token() {
        let config = DebugConfig::new()
            .enable()
            .with_debug_header("X-Debug-Token", "my-secret");

        let headers = vec![("X-Debug-Token".to_owned(), b"wrong-secret".to_vec())];

        assert!(!config.is_authorized(&headers));
    }

    #[test]
    fn debug_config_is_authorized_missing_header() {
        let config = DebugConfig::new()
            .enable()
            .with_debug_header("X-Debug-Token", "my-secret");

        let headers: Vec<(String, Vec<u8>)> = vec![];

        assert!(!config.is_authorized(&headers));
    }

    #[test]
    fn debug_config_header_case_insensitive() {
        let config = DebugConfig::new()
            .enable()
            .with_debug_header("X-Debug-Token", "my-secret");

        let headers = vec![("x-debug-token".to_owned(), b"my-secret".to_vec())];

        assert!(config.is_authorized(&headers));
    }

    #[test]
    fn debug_info_new() {
        let info = DebugInfo::new();
        assert!(info.is_empty());
        assert!(info.source_file.is_none());
        assert!(info.source_line.is_none());
        assert!(info.function_name.is_none());
        assert!(info.route_pattern.is_none());
        assert!(info.handler_name.is_none());
        assert!(info.extra.is_empty());
    }

    #[test]
    fn debug_info_with_source_location() {
        let info = DebugInfo::new().with_source_location("src/handlers/user.rs", 42, "get_user");

        assert!(!info.is_empty());
        assert_eq!(info.source_file, Some("src/handlers/user.rs".to_owned()));
        assert_eq!(info.source_line, Some(42));
        assert_eq!(info.function_name, Some("get_user".to_owned()));
    }

    #[test]
    fn debug_info_with_route_pattern() {
        let info = DebugInfo::new().with_route_pattern("/users/{id}");

        assert!(!info.is_empty());
        assert_eq!(info.route_pattern, Some("/users/{id}".to_owned()));
    }

    #[test]
    fn debug_info_with_handler_name() {
        let info = DebugInfo::new().with_handler_name("UserController::get");

        assert!(!info.is_empty());
        assert_eq!(info.handler_name, Some("UserController::get".to_owned()));
    }

    #[test]
    fn debug_info_with_extra() {
        let info = DebugInfo::new()
            .with_extra("user_id", "abc123")
            .with_extra("request_id", "req-456");

        assert!(!info.is_empty());
        assert_eq!(info.extra.get("user_id"), Some(&"abc123".to_owned()));
        assert_eq!(info.extra.get("request_id"), Some(&"req-456".to_owned()));
    }

    #[test]
    fn debug_info_full_builder() {
        let info = DebugInfo::new()
            .with_source_location("src/api/users.rs", 100, "create_user")
            .with_route_pattern("/api/users")
            .with_handler_name("UsersHandler::create")
            .with_extra("method", "POST");

        assert!(!info.is_empty());
        assert_eq!(info.source_file, Some("src/api/users.rs".to_owned()));
        assert_eq!(info.source_line, Some(100));
        assert_eq!(info.function_name, Some("create_user".to_owned()));
        assert_eq!(info.route_pattern, Some("/api/users".to_owned()));
        assert_eq!(info.handler_name, Some("UsersHandler::create".to_owned()));
        assert_eq!(info.extra.get("method"), Some(&"POST".to_owned()));
    }

    #[test]
    fn debug_info_serialization() {
        let info = DebugInfo::new()
            .with_source_location("src/test.rs", 42, "test_fn")
            .with_route_pattern("/test");

        let json = serde_json::to_value(&info).unwrap();

        assert_eq!(json["source_file"], "src/test.rs");
        assert_eq!(json["source_line"], 42);
        assert_eq!(json["function_name"], "test_fn");
        assert_eq!(json["route_pattern"], "/test");
        // handler_name and extra should be omitted when empty/None
        assert!(json.get("handler_name").is_none());
        assert!(json.get("extra").is_none());
    }

    #[test]
    fn debug_info_serialization_skip_none() {
        let info = DebugInfo::new().with_route_pattern("/test");

        let json = serde_json::to_value(&info).unwrap();

        // Only route_pattern should be present
        assert_eq!(json["route_pattern"], "/test");
        assert!(json.get("source_file").is_none());
        assert!(json.get("source_line").is_none());
        assert!(json.get("function_name").is_none());
    }

    #[test]
    fn http_error_with_debug_info() {
        let debug = DebugInfo::new()
            .with_source_location("src/handlers.rs", 50, "handle_request")
            .with_route_pattern("/api/test");

        let error = HttpError::not_found()
            .with_detail("Resource not found")
            .with_debug_info(debug);

        assert!(error.debug_info.is_some());
        let info = error.debug_info.unwrap();
        assert_eq!(info.source_file, Some("src/handlers.rs".to_owned()));
        assert_eq!(info.source_line, Some(50));
    }

    #[test]
    fn http_error_response_without_debug_mode() {
        disable_debug_mode();

        let error = HttpError::not_found()
            .with_detail("User not found")
            .with_debug_info(
                DebugInfo::new()
                    .with_source_location("src/test.rs", 42, "test")
                    .with_route_pattern("/users/{id}"),
            );

        let response = error.into_response();
        let body = match response.body_ref() {
            ResponseBody::Bytes(b) => b.clone(),
            _ => panic!("Expected bytes body"),
        };

        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Should only have detail, no debug info
        assert_eq!(parsed["detail"], "User not found");
        assert!(parsed.get("debug").is_none());
    }

    #[test]
    fn http_error_response_with_debug_mode() {
        // Enable debug mode for this test
        enable_debug_mode();

        let error = HttpError::not_found()
            .with_detail("User not found")
            .with_debug_info(
                DebugInfo::new()
                    .with_source_location("src/test.rs", 42, "test")
                    .with_route_pattern("/users/{id}"),
            );

        let response = error.into_response();
        let body = match response.body_ref() {
            ResponseBody::Bytes(b) => b.clone(),
            _ => panic!("Expected bytes body"),
        };

        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Should have both detail and debug info
        assert_eq!(parsed["detail"], "User not found");
        assert!(parsed.get("debug").is_some());
        assert_eq!(parsed["debug"]["source_file"], "src/test.rs");
        assert_eq!(parsed["debug"]["source_line"], 42);
        assert_eq!(parsed["debug"]["function_name"], "test");
        assert_eq!(parsed["debug"]["route_pattern"], "/users/{id}");

        // Clean up
        disable_debug_mode();
    }

    #[test]
    fn http_error_response_with_debug_mode_no_debug_info() {
        // Enable debug mode but don't add debug info
        enable_debug_mode();

        let error = HttpError::not_found().with_detail("User not found");

        let response = error.into_response();
        let body = match response.body_ref() {
            ResponseBody::Bytes(b) => b.clone(),
            _ => panic!("Expected bytes body"),
        };

        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Should only have detail, no debug field
        assert_eq!(parsed["detail"], "User not found");
        assert!(parsed.get("debug").is_none());

        // Clean up
        disable_debug_mode();
    }

    #[test]
    fn validation_errors_with_debug_info() {
        let errors = ValidationErrors::single(ValidationError::missing(loc::query("q")))
            .with_debug_info(
                DebugInfo::new()
                    .with_source_location("src/extractors.rs", 100, "extract_query")
                    .with_handler_name("SearchHandler::search"),
            );

        assert!(errors.debug_info.is_some());
    }

    #[test]
    fn validation_errors_response_without_debug_mode() {
        disable_debug_mode();

        let errors = ValidationErrors::single(ValidationError::missing(loc::query("q")))
            .with_debug_info(DebugInfo::new().with_source_location("src/test.rs", 42, "test"));

        let response = errors.into_response();
        let body = match response.body_ref() {
            ResponseBody::Bytes(b) => b.clone(),
            _ => panic!("Expected bytes body"),
        };

        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Should only have detail array, no debug info
        assert!(parsed["detail"].is_array());
        assert!(parsed.get("debug").is_none());
    }

    #[test]
    fn validation_errors_response_with_debug_mode() {
        enable_debug_mode();

        let errors = ValidationErrors::single(ValidationError::missing(loc::query("q")))
            .with_debug_info(
                DebugInfo::new()
                    .with_source_location("src/test.rs", 42, "test")
                    .with_route_pattern("/search"),
            );

        let response = errors.into_response();
        let body = match response.body_ref() {
            ResponseBody::Bytes(b) => b.clone(),
            _ => panic!("Expected bytes body"),
        };

        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Should have both detail and debug info
        assert!(parsed["detail"].is_array());
        assert!(parsed.get("debug").is_some());
        assert_eq!(parsed["debug"]["source_file"], "src/test.rs");
        assert_eq!(parsed["debug"]["route_pattern"], "/search");

        // Clean up
        disable_debug_mode();
    }

    #[test]
    fn validation_errors_merge_preserves_debug_info() {
        let mut errors1 = ValidationErrors::single(ValidationError::missing(loc::query("q")))
            .with_debug_info(DebugInfo::new().with_source_location("src/a.rs", 1, "a"));

        let errors2 = ValidationErrors::single(ValidationError::missing(loc::query("page")))
            .with_debug_info(DebugInfo::new().with_source_location("src/b.rs", 2, "b"));

        errors1.merge(errors2);

        // Should keep the first debug_info
        assert!(errors1.debug_info.is_some());
        assert_eq!(
            errors1.debug_info.as_ref().unwrap().source_file,
            Some("src/a.rs".to_owned())
        );
    }

    #[test]
    fn validation_errors_merge_takes_other_debug_info_if_none() {
        let mut errors1 = ValidationErrors::single(ValidationError::missing(loc::query("q")));

        let errors2 = ValidationErrors::single(ValidationError::missing(loc::query("page")))
            .with_debug_info(DebugInfo::new().with_source_location("src/b.rs", 2, "b"));

        errors1.merge(errors2);

        // Should take debug_info from errors2
        assert!(errors1.debug_info.is_some());
        assert_eq!(
            errors1.debug_info.as_ref().unwrap().source_file,
            Some("src/b.rs".to_owned())
        );
    }

    // ========================================================================
    // ResponseValidationError Tests
    // ========================================================================

    #[test]
    fn response_validation_error_new_is_empty() {
        let error = ResponseValidationError::new();
        assert!(error.is_empty());
        assert_eq!(error.len(), 0);
        assert!(error.response_content.is_none());
        assert!(error.summary.is_none());
    }

    #[test]
    fn response_validation_error_serialization_failed() {
        let error = ResponseValidationError::serialization_failed("failed to serialize DateTime");
        assert_eq!(error.len(), 1);
        assert!(error.summary.is_some());
        assert_eq!(
            error.summary.as_deref(),
            Some("failed to serialize DateTime")
        );
        assert_eq!(error.errors[0].error_type, error_types::SERIALIZATION_ERROR);
    }

    #[test]
    fn response_validation_error_model_validation_failed() {
        let error = ResponseValidationError::model_validation_failed("missing required field 'id'");
        assert_eq!(error.len(), 1);
        assert!(error.summary.is_some());
        assert_eq!(
            error.errors[0].error_type,
            error_types::MODEL_VALIDATION_ERROR
        );
    }

    #[test]
    fn response_validation_error_with_error() {
        let error = ResponseValidationError::new()
            .with_error(ValidationError::missing(loc::response_field("user_id")));
        assert_eq!(error.len(), 1);
        assert_eq!(error.errors[0].loc.len(), 2);
    }

    #[test]
    fn response_validation_error_with_errors() {
        let error = ResponseValidationError::new().with_errors(vec![
            ValidationError::missing(loc::response_field("id")),
            ValidationError::missing(loc::response_field("name")),
        ]);
        assert_eq!(error.len(), 2);
    }

    #[test]
    fn response_validation_error_with_response_content() {
        let content = json!({"name": "Alice", "age": 30});
        let error = ResponseValidationError::serialization_failed("test")
            .with_response_content(content.clone());
        assert!(error.response_content.is_some());
        assert_eq!(error.response_content.as_ref().unwrap()["name"], "Alice");
    }

    #[test]
    fn response_validation_error_with_summary() {
        let error = ResponseValidationError::new().with_summary("Custom summary");
        assert_eq!(error.summary.as_deref(), Some("Custom summary"));
    }

    #[test]
    fn response_validation_error_with_debug_info() {
        let error = ResponseValidationError::serialization_failed("test")
            .with_debug_info(DebugInfo::new().with_source_location("handler.rs", 42, "get_user"));
        assert!(error.debug_info.is_some());
    }

    #[test]
    fn response_validation_error_display() {
        let error = ResponseValidationError::new();
        assert_eq!(format!("{}", error), "Response validation failed");

        let error = ResponseValidationError::new().with_summary("missing field");
        assert_eq!(
            format!("{}", error),
            "Response validation failed: missing field"
        );
    }

    #[test]
    fn response_validation_error_into_response_production_mode() {
        // Ensure debug mode is off
        disable_debug_mode();

        let error = ResponseValidationError::serialization_failed("some internal error")
            .with_response_content(json!({"secret": "data"}));

        let response = error.into_response();
        assert_eq!(response.status().as_u16(), 500);

        // Check content-type header
        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());
        assert_eq!(content_type, Some("application/json".to_string()));

        // Check body - should NOT include internal details
        if let crate::response::ResponseBody::Bytes(bytes) = response.body_ref() {
            let body: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(body["error"], "internal_server_error");
            assert_eq!(body["detail"], "Internal Server Error");
            // Should NOT include debug info or response content
            assert!(body.get("debug").is_none());
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn response_validation_error_into_response_debug_mode() {
        // Enable debug mode
        enable_debug_mode();

        let error = ResponseValidationError::serialization_failed("DateTime serialize failed")
            .with_response_content(json!({"created_at": "invalid-date"}))
            .with_debug_info(DebugInfo::new().with_source_location("handler.rs", 100, "get_user"));

        let response = error.into_response();
        assert_eq!(response.status().as_u16(), 500);

        // Check body - should include debug info
        if let crate::response::ResponseBody::Bytes(bytes) = response.body_ref() {
            let body: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(body["error"], "internal_server_error");
            // Should include debug info
            assert!(body.get("debug").is_some());
            let debug = &body["debug"];
            assert_eq!(debug["summary"], "DateTime serialize failed");
            assert!(debug.get("errors").is_some());
            assert!(debug.get("response_content").is_some());
        } else {
            panic!("Expected Bytes body");
        }

        // Restore default state
        disable_debug_mode();
    }

    #[test]
    fn response_validation_error_to_log_string() {
        let error = ResponseValidationError::serialization_failed("test error")
            .with_error(ValidationError::missing(loc::response_field("id")))
            .with_response_content(json!({"name": "Alice"}));

        let log = error.to_log_string();
        assert!(log.contains("Summary: test error"));
        assert!(log.contains("serialization_error"));
        assert!(log.contains("Response content:"));
        assert!(log.contains("Alice"));
    }

    #[test]
    fn response_validation_error_to_log_string_truncates_large_content() {
        // Create a large response content
        let large_string = "x".repeat(1000);
        let error = ResponseValidationError::serialization_failed("test")
            .with_response_content(json!({"data": large_string}));

        let log = error.to_log_string();
        assert!(log.contains("(truncated)"));
    }

    #[test]
    fn response_validation_error_iter() {
        let error = ResponseValidationError::new()
            .with_error(ValidationError::missing(loc::response_field("a")))
            .with_error(ValidationError::missing(loc::response_field("b")));

        let locs: Vec<_> = error.iter().map(|e| e.loc.clone()).collect();
        assert_eq!(locs.len(), 2);
    }

    #[test]
    fn loc_response_helper() {
        let loc = loc::response();
        assert_eq!(loc.len(), 1);
        assert!(matches!(&loc[0], LocItem::Field(s) if s == "response"));
    }

    #[test]
    fn loc_response_field_helper() {
        let loc = loc::response_field("user_id");
        assert_eq!(loc.len(), 2);
        assert!(matches!(&loc[0], LocItem::Field(s) if s == "response"));
        assert!(matches!(&loc[1], LocItem::Field(s) if s == "user_id"));
    }

    #[test]
    fn loc_response_path_helper() {
        let loc = loc::response_path(&["user", "profile", "name"]);
        assert_eq!(loc.len(), 4);
        assert!(matches!(&loc[0], LocItem::Field(s) if s == "response"));
        assert!(matches!(&loc[1], LocItem::Field(s) if s == "user"));
        assert!(matches!(&loc[2], LocItem::Field(s) if s == "profile"));
        assert!(matches!(&loc[3], LocItem::Field(s) if s == "name"));
    }
}
