//! Error types.

use crate::response::{IntoResponse, Response, ResponseBody, StatusCode};
use serde::{Serialize, Serializer};
use std::collections::HashMap;

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
}

// ============================================================================
// HTTP Error
// ============================================================================

/// HTTP error that produces a response.
#[derive(Debug)]
pub struct HttpError {
    /// Status code.
    pub status: StatusCode,
    /// Error detail message.
    pub detail: Option<String>,
    /// Additional headers.
    pub headers: Vec<(String, Vec<u8>)>,
}

impl HttpError {
    /// Create a new HTTP error.
    #[must_use]
    pub fn new(status: StatusCode) -> Self {
        Self {
            status,
            detail: None,
            headers: Vec::new(),
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
        #[derive(Serialize)]
        struct ErrorBody<'a> {
            detail: &'a str,
        }

        let detail = self
            .detail
            .as_deref()
            .unwrap_or_else(|| self.status.canonical_reason());

        let body = serde_json::to_vec(&ErrorBody { detail }).unwrap_or_default();

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
}

impl ValidationErrors {
    /// Create empty validation errors.
    #[must_use]
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            body: None,
        }
    }

    /// Create from a single error.
    #[must_use]
    pub fn single(error: ValidationError) -> Self {
        Self {
            errors: vec![error],
            body: None,
        }
    }

    /// Create from multiple errors.
    #[must_use]
    pub fn from_errors(errors: Vec<ValidationError>) -> Self {
        Self { errors, body: None }
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
        let body = self.to_json_bytes();

        Response::with_status(StatusCode::UNPROCESSABLE_ENTITY)
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
}
