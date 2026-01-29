//! Request extraction traits and extractors.
//!
//! This module provides the [`FromRequest`] trait and common extractors
//! like [`Json`] and [`Path`] for parsing request data.

use crate::context::RequestContext;
use crate::error::{HttpError, ValidationError, ValidationErrors};
use crate::request::{Body, Request};
use crate::response::{IntoResponse, Response, ResponseBody};
use serde::de::{
    self, DeserializeOwned, Deserializer, IntoDeserializer, MapAccess, SeqAccess, Visitor,
};
use std::fmt;
use std::future::Future;
use std::ops::{Deref, DerefMut};

/// Trait for types that can be extracted from a request.
///
/// This is the core abstraction for request handlers. Each parameter
/// in a handler function implements this trait.
///
/// The `ctx` parameter provides access to the request context, including
/// asupersync's capability context for cancellation checkpoints and
/// budget-aware operations.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{FromRequest, Request, RequestContext};
///
/// struct MyExtractor(String);
///
/// impl FromRequest for MyExtractor {
///     type Error = std::convert::Infallible;
///
///     async fn from_request(
///         ctx: &RequestContext,
///         req: &mut Request,
///     ) -> Result<Self, Self::Error> {
///         // Check for cancellation before expensive work
///         let _ = ctx.checkpoint();
///         Ok(MyExtractor("extracted".to_string()))
///     }
/// }
/// ```
pub trait FromRequest: Sized {
    /// Error type when extraction fails.
    type Error: IntoResponse;

    /// Extract a value from the request.
    ///
    /// # Parameters
    ///
    /// - `ctx`: The request context providing access to asupersync capabilities
    /// - `req`: The HTTP request to extract from
    fn from_request(
        ctx: &RequestContext,
        req: &mut Request,
    ) -> impl Future<Output = Result<Self, Self::Error>> + Send;
}

// Implement for Option to make extractors optional
impl<T: FromRequest> FromRequest for Option<T> {
    type Error = std::convert::Infallible;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        Ok(T::from_request(ctx, req).await.ok())
    }
}

// Implement for RequestContext itself - allows handlers to receive the context
impl FromRequest for RequestContext {
    type Error = std::convert::Infallible;

    async fn from_request(ctx: &RequestContext, _req: &mut Request) -> Result<Self, Self::Error> {
        Ok(ctx.clone())
    }
}

// ============================================================================
// JSON Body Extractor
// ============================================================================

/// Default maximum JSON body size (1MB).
pub const DEFAULT_JSON_LIMIT: usize = 1024 * 1024;

/// Configuration for JSON extraction.
#[derive(Debug, Clone)]
pub struct JsonConfig {
    /// Maximum body size in bytes.
    limit: usize,
    /// Content-Type header value to accept (case-insensitive).
    /// If None, accepts any application/json variant.
    content_type: Option<String>,
}

impl Default for JsonConfig {
    fn default() -> Self {
        Self {
            limit: DEFAULT_JSON_LIMIT,
            content_type: None,
        }
    }
}

impl JsonConfig {
    /// Create a new JSON configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum body size limit.
    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Set a specific Content-Type to accept.
    #[must_use]
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Returns the configured size limit.
    #[must_use]
    pub fn get_limit(&self) -> usize {
        self.limit
    }
}

/// JSON body extractor.
///
/// Extracts a JSON body from the request and deserializes it to type `T`.
///
/// # Error Responses
///
/// - **415 Unsupported Media Type**: Content-Type is not `application/json`
/// - **413 Payload Too Large**: Body exceeds configured size limit
/// - **422 Unprocessable Entity**: JSON parsing failed
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::Json;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct CreateUser {
///     name: String,
///     email: String,
/// }
///
/// async fn create_user(Json(user): Json<CreateUser>) -> impl IntoResponse {
///     format!("Created user: {}", user.name)
/// }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T> Json<T> {
    /// Unwrap the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: serde::Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        match serde_json::to_vec(&self.0) {
            Ok(bytes) => Response::ok()
                .header("content-type", b"application/json".to_vec())
                .body(ResponseBody::Bytes(bytes)),
            Err(e) => {
                // Serialization error - use ResponseValidationError for proper handling
                // This ensures error details are logged but not exposed to clients
                crate::error::ResponseValidationError::serialization_failed(e.to_string())
                    .into_response()
            }
        }
    }
}

/// Error returned when JSON extraction fails.
#[derive(Debug)]
pub enum JsonExtractError {
    /// Content-Type header is missing or not application/json.
    UnsupportedMediaType {
        /// The actual Content-Type received (if any).
        actual: Option<String>,
    },
    /// Request body exceeds the size limit.
    PayloadTooLarge {
        /// The actual body size.
        size: usize,
        /// The configured limit.
        limit: usize,
    },
    /// JSON deserialization failed.
    DeserializeError {
        /// The serde_json error message.
        message: String,
        /// Line number where error occurred (if available).
        line: Option<usize>,
        /// Column number where error occurred (if available).
        column: Option<usize>,
    },
    /// Streaming request bodies are not supported.
    StreamingNotSupported,
}

impl std::fmt::Display for JsonExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedMediaType { actual } => {
                if let Some(ct) = actual {
                    write!(f, "Expected Content-Type: application/json, got: {ct}")
                } else {
                    write!(f, "Missing Content-Type header, expected application/json")
                }
            }
            Self::PayloadTooLarge { size, limit } => {
                write!(
                    f,
                    "Request body too large: {size} bytes exceeds {limit} byte limit"
                )
            }
            Self::DeserializeError {
                message,
                line,
                column,
            } => {
                if let (Some(l), Some(c)) = (line, column) {
                    write!(f, "JSON parse error at line {l}, column {c}: {message}")
                } else {
                    write!(f, "JSON parse error: {message}")
                }
            }
            Self::StreamingNotSupported => {
                write!(
                    f,
                    "Streaming request bodies are not supported for JSON extraction"
                )
            }
        }
    }
}

impl std::error::Error for JsonExtractError {}

impl IntoResponse for JsonExtractError {
    fn into_response(self) -> crate::response::Response {
        match self {
            Self::UnsupportedMediaType { actual } => {
                let detail = if let Some(ct) = actual {
                    format!("Expected Content-Type: application/json, got: {ct}")
                } else {
                    "Missing Content-Type header, expected application/json".to_string()
                };
                HttpError::unsupported_media_type()
                    .with_detail(detail)
                    .into_response()
            }
            Self::PayloadTooLarge { size, limit } => HttpError::payload_too_large()
                .with_detail(format!(
                    "Request body too large: {size} bytes exceeds {limit} byte limit"
                ))
                .into_response(),
            Self::DeserializeError {
                message,
                line,
                column,
            } => {
                // Return a 422 with validation error format
                let msg = if let (Some(l), Some(c)) = (line, column) {
                    format!("JSON parse error at line {l}, column {c}: {message}")
                } else {
                    format!("JSON parse error: {message}")
                };
                ValidationErrors::single(ValidationError::json_invalid(
                    crate::error::loc::body(),
                    msg,
                ))
                .into_response()
            }
            Self::StreamingNotSupported => HttpError::bad_request()
                .with_detail("Streaming request bodies are not supported for JSON extraction")
                .into_response(),
        }
    }
}

impl<T: DeserializeOwned> FromRequest for Json<T> {
    type Error = JsonExtractError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Validate Content-Type
        let content_type = req
            .headers()
            .get("content-type")
            .and_then(|v| std::str::from_utf8(v).ok());

        let is_json = content_type.is_some_and(|ct| {
            let ct_lower = ct.to_ascii_lowercase();
            ct_lower.starts_with("application/json")
                || ct_lower.starts_with("application/") && ct_lower.contains("+json")
        });

        if !is_json {
            return Err(JsonExtractError::UnsupportedMediaType {
                actual: content_type.map(String::from),
            });
        }

        // Check cancellation before reading the body
        let _ = ctx.checkpoint();

        // Get body bytes
        let body = req.take_body();
        let bytes = match body {
            Body::Empty => Vec::new(),
            Body::Bytes(b) => b,
            Body::Stream(_) => {
                // Streaming bodies not yet supported in Json extractor
                return Err(JsonExtractError::StreamingNotSupported);
            }
        };

        // Check size limit using the configured limit from RequestContext.
        // This respects both app-level config (AppConfig.max_body_size) and
        // per-route overrides when available.
        let limit = ctx.max_body_size();
        if bytes.len() > limit {
            return Err(JsonExtractError::PayloadTooLarge {
                size: bytes.len(),
                limit,
            });
        }

        // Check cancellation before deserialization
        let _ = ctx.checkpoint();

        // Deserialize JSON
        let value =
            serde_json::from_slice(&bytes).map_err(|e| JsonExtractError::DeserializeError {
                message: e.to_string(),
                line: Some(e.line()),
                column: Some(e.column()),
            })?;

        // Check cancellation after parsing
        let _ = ctx.checkpoint();

        Ok(Json(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::Method;

    // Helper to create a test context
    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    // Helper to create a request with JSON body
    fn json_request(body: &str) -> Request {
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(body.as_bytes().to_vec()));
        req
    }

    #[test]
    fn json_config_defaults() {
        let config = JsonConfig::default();
        assert_eq!(config.get_limit(), DEFAULT_JSON_LIMIT);
    }

    #[test]
    fn json_config_custom() {
        let config = JsonConfig::new().limit(1024);
        assert_eq!(config.get_limit(), 1024);
    }

    #[test]
    fn json_deref() {
        let json = Json(42i32);
        assert_eq!(*json, 42);
    }

    #[test]
    fn json_into_inner() {
        let json = Json("hello".to_string());
        assert_eq!(json.into_inner(), "hello");
    }

    #[test]
    fn json_extract_success() {
        use serde::Deserialize;

        #[derive(Deserialize, Debug, PartialEq)]
        struct TestPayload {
            name: String,
            value: i32,
        }

        let ctx = test_context();
        let mut req = json_request(r#"{"name": "test", "value": 42}"#);

        let result = futures_executor::block_on(Json::<TestPayload>::from_request(&ctx, &mut req));
        let Json(payload) = result.unwrap();
        assert_eq!(payload.name, "test");
        assert_eq!(payload.value, 42);
    }

    #[test]
    fn json_extract_wrong_content_type() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct TestPayload {
            #[allow(dead_code)]
            name: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"text/plain".to_vec());
        req.set_body(Body::Bytes(b"{}".to_vec()));

        let result = futures_executor::block_on(Json::<TestPayload>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(JsonExtractError::UnsupportedMediaType { actual: Some(ct) })
            if ct == "text/plain"
        ));
    }

    #[test]
    fn json_extract_missing_content_type() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct TestPayload {
            #[allow(dead_code)]
            name: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.set_body(Body::Bytes(b"{}".to_vec()));

        let result = futures_executor::block_on(Json::<TestPayload>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(JsonExtractError::UnsupportedMediaType { actual: None })
        ));
    }

    #[test]
    fn json_extract_invalid_json() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        struct TestPayload {
            #[allow(dead_code)]
            name: String,
        }

        let ctx = test_context();
        let mut req = json_request(r#"{"name": invalid}"#);

        let result = futures_executor::block_on(Json::<TestPayload>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(JsonExtractError::DeserializeError { .. })
        ));
    }

    #[test]
    fn json_extract_application_json_charset() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct TestPayload {
            value: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json; charset=utf-8".to_vec());
        req.set_body(Body::Bytes(b"{\"value\": 123}".to_vec()));

        let result = futures_executor::block_on(Json::<TestPayload>::from_request(&ctx, &mut req));
        let Json(payload) = result.unwrap();
        assert_eq!(payload.value, 123);
    }

    #[test]
    fn json_extract_vendor_json() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct TestPayload {
            value: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        // Vendor media types like application/vnd.api+json should work
        req.headers_mut()
            .insert("content-type", b"application/vnd.api+json".to_vec());
        req.set_body(Body::Bytes(b"{\"value\": 456}".to_vec()));

        let result = futures_executor::block_on(Json::<TestPayload>::from_request(&ctx, &mut req));
        let Json(payload) = result.unwrap();
        assert_eq!(payload.value, 456);
    }

    #[test]
    fn json_error_display() {
        let err = JsonExtractError::UnsupportedMediaType {
            actual: Some("text/html".to_string()),
        };
        assert!(err.to_string().contains("text/html"));

        let err = JsonExtractError::PayloadTooLarge {
            size: 2000,
            limit: 1000,
        };
        assert!(err.to_string().contains("2000"));
        assert!(err.to_string().contains("1000"));

        let err = JsonExtractError::DeserializeError {
            message: "unexpected token".to_string(),
            line: Some(1),
            column: Some(10),
        };
        assert!(err.to_string().contains("line 1"));
        assert!(err.to_string().contains("column 10"));
    }
}

// ============================================================================
// Form Body Extractor
// ============================================================================

/// Default maximum form body size (1MB).
pub const DEFAULT_FORM_LIMIT: usize = 1024 * 1024;

/// Configuration for form extraction.
#[derive(Debug, Clone)]
pub struct FormConfig {
    limit: usize,
}

impl Default for FormConfig {
    fn default() -> Self {
        Self {
            limit: DEFAULT_FORM_LIMIT,
        }
    }
}

impl FormConfig {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    #[must_use]
    pub fn get_limit(&self) -> usize {
        self.limit
    }
}

/// Form body extractor for `application/x-www-form-urlencoded`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Form<T>(pub T);

impl<T> Form<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Form<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Form<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Error for form extraction failures.
#[derive(Debug)]
pub enum FormExtractError {
    UnsupportedMediaType { actual: Option<String> },
    PayloadTooLarge { size: usize, limit: usize },
    DeserializeError { message: String },
    StreamingNotSupported,
    InvalidUtf8,
}

impl std::fmt::Display for FormExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedMediaType { actual } => {
                if let Some(ct) = actual {
                    write!(f, "Expected application/x-www-form-urlencoded, got: {ct}")
                } else {
                    write!(f, "Missing Content-Type header")
                }
            }
            Self::PayloadTooLarge { size, limit } => {
                write!(f, "Body too large: {size} > {limit}")
            }
            Self::DeserializeError { message } => write!(f, "Form error: {message}"),
            Self::StreamingNotSupported => write!(f, "Streaming not supported"),
            Self::InvalidUtf8 => write!(f, "Invalid UTF-8"),
        }
    }
}

impl std::error::Error for FormExtractError {}

impl IntoResponse for FormExtractError {
    fn into_response(self) -> Response {
        match &self {
            FormExtractError::UnsupportedMediaType { .. } => {
                HttpError::unsupported_media_type().into_response()
            }
            FormExtractError::PayloadTooLarge { size, limit } => HttpError::payload_too_large()
                .with_detail(format!("Body {size} > {limit}"))
                .into_response(),
            FormExtractError::DeserializeError { message } => {
                use crate::error::error_types;
                ValidationErrors::single(
                    ValidationError::new(
                        error_types::VALUE_ERROR,
                        vec![crate::error::LocItem::field("body")],
                    )
                    .with_msg(message.clone()),
                )
                .into_response()
            }
            FormExtractError::StreamingNotSupported => HttpError::bad_request().into_response(),
            FormExtractError::InvalidUtf8 => HttpError::bad_request()
                .with_detail("Invalid UTF-8")
                .into_response(),
        }
    }
}

impl<T: DeserializeOwned> FromRequest for Form<T> {
    type Error = FormExtractError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let ct = req
            .headers()
            .get("content-type")
            .and_then(|v| std::str::from_utf8(v).ok());
        let is_form = ct.is_some_and(|c| {
            c.to_ascii_lowercase()
                .starts_with("application/x-www-form-urlencoded")
        });
        if !is_form {
            return Err(FormExtractError::UnsupportedMediaType {
                actual: ct.map(String::from),
            });
        }
        let _ = ctx.checkpoint();
        let body = req.take_body();
        let bytes = match body {
            Body::Empty => Vec::new(),
            Body::Bytes(b) => b,
            Body::Stream(_) => return Err(FormExtractError::StreamingNotSupported),
        };
        let limit = ctx.max_body_size();
        if bytes.len() > limit {
            return Err(FormExtractError::PayloadTooLarge {
                size: bytes.len(),
                limit,
            });
        }
        let _ = ctx.checkpoint();
        let body_str = std::str::from_utf8(&bytes).map_err(|_| FormExtractError::InvalidUtf8)?;
        let params = QueryParams::parse(body_str);
        let value = T::deserialize(QueryDeserializer::new(&params)).map_err(|e| {
            FormExtractError::DeserializeError {
                message: e.to_string(),
            }
        })?;
        let _ = ctx.checkpoint();
        Ok(Form(value))
    }
}

#[cfg(test)]
mod form_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    fn form_request(body: &str) -> Request {
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut().insert(
            "content-type",
            b"application/x-www-form-urlencoded".to_vec(),
        );
        req.set_body(Body::Bytes(body.as_bytes().to_vec()));
        req
    }

    #[test]
    fn form_extract_success() {
        use serde::Deserialize;
        #[derive(Deserialize, Debug, PartialEq)]
        struct Login {
            username: String,
            password: String,
        }
        let ctx = test_context();
        let mut req = form_request("username=alice&password=secret");
        let result = futures_executor::block_on(Form::<Login>::from_request(&ctx, &mut req));
        let Form(form) = result.unwrap();
        assert_eq!(form.username, "alice");
        assert_eq!(form.password, "secret");
    }

    #[test]
    fn form_wrong_content_type() {
        use serde::Deserialize;
        #[derive(Deserialize)]
        struct T {
            #[allow(dead_code)]
            x: String,
        }
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"x=1".to_vec()));
        let result = futures_executor::block_on(Form::<T>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(FormExtractError::UnsupportedMediaType { .. })
        ));
    }
}

// ============================================================================
// Raw Body Extractors (Bytes/String)
// ============================================================================

/// Default maximum raw body size (2MB).
pub const DEFAULT_RAW_BODY_LIMIT: usize = 2 * 1024 * 1024;

/// Configuration for raw body extraction.
#[derive(Debug, Clone)]
pub struct RawBodyConfig {
    /// Maximum body size in bytes.
    limit: usize,
}

impl Default for RawBodyConfig {
    fn default() -> Self {
        Self {
            limit: DEFAULT_RAW_BODY_LIMIT,
        }
    }
}

impl RawBodyConfig {
    /// Create a new configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum body size.
    #[must_use]
    pub fn limit(mut self, size: usize) -> Self {
        self.limit = size;
        self
    }

    /// Get the maximum body size.
    #[must_use]
    pub fn get_limit(&self) -> usize {
        self.limit
    }
}

/// Error for raw body extraction failures.
#[derive(Debug)]
pub enum RawBodyError {
    /// Body exceeds maximum allowed size.
    PayloadTooLarge { size: usize, limit: usize },
    /// Streaming body not supported.
    StreamingNotSupported,
    /// Body is not valid UTF-8 (for String extractor).
    InvalidUtf8,
}

impl std::fmt::Display for RawBodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge { size, limit } => {
                write!(
                    f,
                    "Payload too large: {size} bytes exceeds limit of {limit}"
                )
            }
            Self::StreamingNotSupported => {
                write!(f, "Streaming body not supported for raw extraction")
            }
            Self::InvalidUtf8 => write!(f, "Body is not valid UTF-8"),
        }
    }
}

impl std::error::Error for RawBodyError {}

impl IntoResponse for RawBodyError {
    fn into_response(self) -> Response {
        match &self {
            RawBodyError::PayloadTooLarge { size, limit } => HttpError::payload_too_large()
                .with_detail(format!("Body {size} bytes > {limit} limit"))
                .into_response(),
            RawBodyError::StreamingNotSupported => HttpError::bad_request()
                .with_detail("Streaming body not supported")
                .into_response(),
            RawBodyError::InvalidUtf8 => HttpError::bad_request()
                .with_detail("Body is not valid UTF-8")
                .into_response(),
        }
    }
}

/// Raw bytes body extractor.
///
/// Extracts the request body as raw bytes without any content-type validation.
/// This is useful when you need the raw payload regardless of format.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{Bytes, FromRequest};
///
/// async fn upload(body: Bytes) -> String {
///     format!("Received {} bytes", body.len())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    /// Create a new Bytes from a vector.
    #[must_use]
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    /// Get the length of the body.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if the body is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Get the bytes as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Take ownership of the inner Vec.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl std::ops::Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(data: Vec<u8>) -> Self {
        Self(data)
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        bytes.0
    }
}

impl FromRequest for Bytes {
    type Error = RawBodyError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let _ = ctx.checkpoint();

        let body = req.take_body();
        let bytes = match body {
            Body::Empty => Vec::new(),
            Body::Bytes(b) => b,
            Body::Stream(_) => return Err(RawBodyError::StreamingNotSupported),
        };

        // Get limit from config or use default
        let limit = req
            .get_extension::<RawBodyConfig>()
            .map(|c| c.limit)
            .unwrap_or(DEFAULT_RAW_BODY_LIMIT);

        if bytes.len() > limit {
            return Err(RawBodyError::PayloadTooLarge {
                size: bytes.len(),
                limit,
            });
        }

        let _ = ctx.checkpoint();
        Ok(Bytes(bytes))
    }
}

/// String body extractor.
///
/// Extracts the request body as a UTF-8 string. Returns an error if the
/// body is not valid UTF-8.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{StringBody, FromRequest};
///
/// async fn process(body: StringBody) -> String {
///     format!("Received: {}", body.as_str())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct StringBody(pub String);

impl StringBody {
    /// Create a new Text from a string.
    #[must_use]
    pub fn new(data: String) -> Self {
        Self(data)
    }

    /// Get the length of the string.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if the string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Get the string as a str slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Take ownership of the inner String.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl AsRef<str> for StringBody {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for StringBody {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<String> for StringBody {
    fn from(data: String) -> Self {
        Self(data)
    }
}

impl From<StringBody> for String {
    fn from(text: StringBody) -> Self {
        text.0
    }
}

impl FromRequest for StringBody {
    type Error = RawBodyError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let bytes = Bytes::from_request(ctx, req).await?;

        let text = String::from_utf8(bytes.into_inner()).map_err(|_| RawBodyError::InvalidUtf8)?;

        Ok(StringBody(text))
    }
}

#[cfg(test)]
mod raw_body_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        RequestContext::new(asupersync::Cx::for_testing(), 1)
    }

    #[test]
    fn test_bytes_extract_success() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.set_body(Body::Bytes(b"hello world".to_vec()));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        let bytes = result.unwrap();
        assert_eq!(bytes.as_slice(), b"hello world");
        assert_eq!(bytes.len(), 11);
    }

    #[test]
    fn test_bytes_extract_empty() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.set_body(Body::Empty);

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        let bytes = result.unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn test_bytes_size_limit() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        let large_body = vec![0u8; DEFAULT_RAW_BODY_LIMIT + 1];
        req.set_body(Body::Bytes(large_body));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        assert!(matches!(result, Err(RawBodyError::PayloadTooLarge { .. })));
    }

    #[test]
    fn test_bytes_custom_limit() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.insert_extension(RawBodyConfig::new().limit(100));
        req.set_body(Body::Bytes(vec![0u8; 150]));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(RawBodyError::PayloadTooLarge {
                size: 150,
                limit: 100
            })
        ));
    }

    #[test]
    fn test_bytes_deref() {
        let bytes = Bytes::new(b"test".to_vec());
        assert_eq!(&*bytes, b"test");
    }

    #[test]
    fn test_bytes_from_vec() {
        let bytes: Bytes = vec![1, 2, 3].into();
        assert_eq!(bytes.as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn test_string_body_extract_success() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/text");
        req.set_body(Body::Bytes(b"hello world".to_vec()));

        let result = futures_executor::block_on(StringBody::from_request(&ctx, &mut req));
        let text = result.unwrap();
        assert_eq!(text.as_str(), "hello world");
        assert_eq!(text.len(), 11);
    }

    #[test]
    fn test_string_body_extract_empty() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/text");
        req.set_body(Body::Empty);

        let result = futures_executor::block_on(StringBody::from_request(&ctx, &mut req));
        let text = result.unwrap();
        assert!(text.is_empty());
    }

    #[test]
    fn test_string_body_invalid_utf8() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/text");
        // Invalid UTF-8 sequence
        req.set_body(Body::Bytes(vec![0xff, 0xfe, 0x00, 0x01]));

        let result = futures_executor::block_on(StringBody::from_request(&ctx, &mut req));
        assert!(matches!(result, Err(RawBodyError::InvalidUtf8)));
    }

    #[test]
    fn test_string_body_deref() {
        let text = StringBody::new("hello".to_string());
        assert_eq!(&*text, "hello");
    }

    #[test]
    fn test_string_body_from_string() {
        let text: StringBody = "test".to_string().into();
        assert_eq!(text.as_str(), "test");
    }

    #[test]
    fn test_string_body_unicode() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/text");
        req.set_body(Body::Bytes("こんにちは世界 🌍".as_bytes().to_vec()));

        let result = futures_executor::block_on(StringBody::from_request(&ctx, &mut req));
        let text = result.unwrap();
        assert_eq!(text.as_str(), "こんにちは世界 🌍");
    }
}

// ============================================================================
// Multipart Form Extractor
// ============================================================================

/// Default maximum file size for multipart uploads (10MB).
pub const DEFAULT_MULTIPART_FILE_SIZE: usize = 10 * 1024 * 1024;

/// Default maximum total size for multipart uploads (50MB).
pub const DEFAULT_MULTIPART_TOTAL_SIZE: usize = 50 * 1024 * 1024;

/// Default maximum number of fields in multipart form.
pub const DEFAULT_MULTIPART_MAX_FIELDS: usize = 100;

/// Configuration for multipart form extraction.
#[derive(Debug, Clone)]
pub struct MultipartConfig {
    max_file_size: usize,
    max_total_size: usize,
    max_fields: usize,
}

impl Default for MultipartConfig {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MULTIPART_FILE_SIZE,
            max_total_size: DEFAULT_MULTIPART_TOTAL_SIZE,
            max_fields: DEFAULT_MULTIPART_MAX_FIELDS,
        }
    }
}

impl MultipartConfig {
    /// Create a new configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum file size.
    #[must_use]
    pub fn max_file_size(mut self, size: usize) -> Self {
        self.max_file_size = size;
        self
    }

    /// Set the maximum total upload size.
    #[must_use]
    pub fn max_total_size(mut self, size: usize) -> Self {
        self.max_total_size = size;
        self
    }

    /// Set the maximum number of fields.
    #[must_use]
    pub fn max_fields(mut self, count: usize) -> Self {
        self.max_fields = count;
        self
    }

    /// Get the maximum file size.
    #[must_use]
    pub fn get_max_file_size(&self) -> usize {
        self.max_file_size
    }

    /// Get the maximum total upload size.
    #[must_use]
    pub fn get_max_total_size(&self) -> usize {
        self.max_total_size
    }

    /// Get the maximum number of fields.
    #[must_use]
    pub fn get_max_fields(&self) -> usize {
        self.max_fields
    }
}

/// An uploaded file extracted from a multipart form.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{UploadedFile, FromRequest};
///
/// async fn upload(file: UploadedFile) -> String {
///     format!("Received file '{}' ({} bytes)", file.filename(), file.size())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct UploadedFile {
    /// The form field name.
    field_name: String,
    /// The original filename.
    filename: String,
    /// The Content-Type of the file.
    content_type: String,
    /// The file contents.
    data: Vec<u8>,
}

impl UploadedFile {
    /// Create a new uploaded file.
    #[must_use]
    pub fn new(field_name: String, filename: String, content_type: String, data: Vec<u8>) -> Self {
        Self {
            field_name,
            filename,
            content_type,
            data,
        }
    }

    /// Get the form field name.
    #[must_use]
    pub fn field_name(&self) -> &str {
        &self.field_name
    }

    /// Get the original filename.
    #[must_use]
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// Get the Content-Type.
    #[must_use]
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    /// Get the file data.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Take ownership of the file data.
    #[must_use]
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// Get the file size in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Get the file extension from the filename.
    #[must_use]
    pub fn extension(&self) -> Option<&str> {
        self.filename
            .rsplit('.')
            .next()
            .filter(|ext| !ext.is_empty() && *ext != self.filename)
    }

    /// Read the file data as UTF-8 text.
    ///
    /// Returns `None` if the data is not valid UTF-8.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }
}

/// Error for multipart form extraction failures.
#[derive(Debug)]
pub enum MultipartExtractError {
    /// Content-Type is not multipart/form-data.
    UnsupportedMediaType { actual: Option<String> },
    /// Missing boundary in Content-Type.
    MissingBoundary,
    /// File size exceeds limit.
    FileTooLarge { size: usize, limit: usize },
    /// Total upload size exceeds limit.
    TotalTooLarge { size: usize, limit: usize },
    /// Too many fields.
    TooManyFields { count: usize, limit: usize },
    /// Invalid multipart format.
    InvalidFormat { detail: String },
    /// Streaming body not supported.
    StreamingNotSupported,
    /// No file found with the given field name.
    FileNotFound { field_name: String },
}

impl std::fmt::Display for MultipartExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedMediaType { actual } => {
                if let Some(ct) = actual {
                    write!(f, "Expected multipart/form-data, got: {ct}")
                } else {
                    write!(f, "Expected multipart/form-data, got empty Content-Type")
                }
            }
            Self::MissingBoundary => write!(f, "Missing boundary in multipart Content-Type"),
            Self::FileTooLarge { size, limit } => {
                write!(f, "File too large: {size} bytes exceeds limit of {limit}")
            }
            Self::TotalTooLarge { size, limit } => {
                write!(
                    f,
                    "Total upload too large: {size} bytes exceeds limit of {limit}"
                )
            }
            Self::TooManyFields { count, limit } => {
                write!(f, "Too many fields: {count} exceeds limit of {limit}")
            }
            Self::InvalidFormat { detail } => {
                write!(f, "Invalid multipart format: {detail}")
            }
            Self::StreamingNotSupported => {
                write!(f, "Streaming body not supported for multipart extraction")
            }
            Self::FileNotFound { field_name } => {
                write!(f, "No file found with field name '{field_name}'")
            }
        }
    }
}

impl std::error::Error for MultipartExtractError {}

impl IntoResponse for MultipartExtractError {
    fn into_response(self) -> Response {
        match &self {
            MultipartExtractError::UnsupportedMediaType { .. } => {
                HttpError::unsupported_media_type().into_response()
            }
            MultipartExtractError::MissingBoundary => HttpError::bad_request()
                .with_detail("Missing boundary in multipart Content-Type")
                .into_response(),
            MultipartExtractError::FileTooLarge { size, limit } => HttpError::payload_too_large()
                .with_detail(format!("File {size} bytes > {limit} limit"))
                .into_response(),
            MultipartExtractError::TotalTooLarge { size, limit } => HttpError::payload_too_large()
                .with_detail(format!("Total {size} bytes > {limit} limit"))
                .into_response(),
            MultipartExtractError::TooManyFields { count, limit } => HttpError::bad_request()
                .with_detail(format!("Too many fields: {count} > {limit}"))
                .into_response(),
            MultipartExtractError::InvalidFormat { detail } => HttpError::bad_request()
                .with_detail(format!("Invalid multipart: {detail}"))
                .into_response(),
            MultipartExtractError::StreamingNotSupported => HttpError::bad_request()
                .with_detail("Streaming body not supported")
                .into_response(),
            MultipartExtractError::FileNotFound { field_name } => {
                use crate::error::error_types;
                ValidationErrors::single(
                    ValidationError::new(
                        error_types::VALUE_ERROR,
                        vec![crate::error::LocItem::field(field_name)],
                    )
                    .with_msg(format!("Required file '{field_name}' not found")),
                )
                .into_response()
            }
        }
    }
}

/// Multipart form data extractor.
///
/// Extracts a complete multipart form including all fields and files.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{Multipart, FromRequest};
///
/// async fn upload(form: Multipart) -> String {
///     let description = form.get_field("description").unwrap_or("No description");
///     let file = form.get_file("document");
///     format!("Description: {}, File: {:?}", description, file.map(|f| f.filename()))
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Multipart {
    parts: Vec<MultipartPart>,
}

/// A part of a multipart form (either a field or a file).
#[derive(Debug, Clone)]
pub struct MultipartPart {
    /// Field name.
    pub name: String,
    /// Filename if this is a file upload.
    pub filename: Option<String>,
    /// Content-Type if specified.
    pub content_type: Option<String>,
    /// The part data.
    pub data: Vec<u8>,
}

impl Multipart {
    /// Create from parsed parts.
    #[must_use]
    pub fn from_parts(parts: Vec<MultipartPart>) -> Self {
        Self { parts }
    }

    /// Get all parts.
    #[must_use]
    pub fn parts(&self) -> &[MultipartPart] {
        &self.parts
    }

    /// Get a form field value by name.
    #[must_use]
    pub fn get_field(&self, name: &str) -> Option<&str> {
        self.parts
            .iter()
            .find(|p| p.name == name && p.filename.is_none())
            .and_then(|p| std::str::from_utf8(&p.data).ok())
    }

    /// Get an uploaded file by field name.
    #[must_use]
    pub fn get_file(&self, name: &str) -> Option<UploadedFile> {
        self.parts
            .iter()
            .find(|p| p.name == name && p.filename.is_some())
            .map(|p| {
                UploadedFile::new(
                    p.name.clone(),
                    p.filename.clone().unwrap_or_default(),
                    p.content_type
                        .clone()
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    p.data.clone(),
                )
            })
    }

    /// Get all files.
    #[must_use]
    pub fn files(&self) -> Vec<UploadedFile> {
        self.parts
            .iter()
            .filter(|p| p.filename.is_some())
            .map(|p| {
                UploadedFile::new(
                    p.name.clone(),
                    p.filename.clone().unwrap_or_default(),
                    p.content_type
                        .clone()
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    p.data.clone(),
                )
            })
            .collect()
    }

    /// Get all files with a specific field name.
    #[must_use]
    pub fn get_files(&self, name: &str) -> Vec<UploadedFile> {
        self.parts
            .iter()
            .filter(|p| p.name == name && p.filename.is_some())
            .map(|p| {
                UploadedFile::new(
                    p.name.clone(),
                    p.filename.clone().unwrap_or_default(),
                    p.content_type
                        .clone()
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    p.data.clone(),
                )
            })
            .collect()
    }

    /// Get all field names and values.
    #[must_use]
    pub fn fields(&self) -> Vec<(&str, &str)> {
        self.parts
            .iter()
            .filter(|p| p.filename.is_none())
            .filter_map(|p| Some((p.name.as_str(), std::str::from_utf8(&p.data).ok()?)))
            .collect()
    }

    /// Check if a field exists.
    #[must_use]
    pub fn has_field(&self, name: &str) -> bool {
        self.parts.iter().any(|p| p.name == name)
    }

    /// Get the number of parts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    /// Check if the form is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

impl FromRequest for Multipart {
    type Error = MultipartExtractError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Check Content-Type
        let content_type = req
            .headers()
            .get("content-type")
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(String::from);

        let ct = content_type
            .as_deref()
            .ok_or(MultipartExtractError::UnsupportedMediaType { actual: None })?;

        if !ct.to_ascii_lowercase().starts_with("multipart/form-data") {
            return Err(MultipartExtractError::UnsupportedMediaType {
                actual: Some(ct.to_string()),
            });
        }

        // Parse boundary
        let boundary = parse_multipart_boundary(ct)?;

        let _ = ctx.checkpoint();

        // Get body
        let body = req.take_body();
        let bytes = match body {
            Body::Empty => Vec::new(),
            Body::Bytes(b) => b,
            Body::Stream(_) => return Err(MultipartExtractError::StreamingNotSupported),
        };

        // Get config from request extensions or use default
        let config = req
            .get_extension::<MultipartConfig>()
            .cloned()
            .unwrap_or_default();

        let _ = ctx.checkpoint();

        // Parse multipart
        let parts = parse_multipart_body(&bytes, &boundary, &config)?;

        let _ = ctx.checkpoint();

        Ok(Multipart::from_parts(parts))
    }
}

/// File extractor for a single file upload.
///
/// This extractor gets a single file from a multipart form by field name.
/// The field name is specified via `FileConfig` extension on the request,
/// or defaults to "file".
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{File, FromRequest};
///
/// async fn upload(file: File) -> String {
///     format!("Received: {} ({} bytes)", file.filename(), file.size())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct File(pub UploadedFile);

impl File {
    /// Get the underlying uploaded file.
    #[must_use]
    pub fn into_inner(self) -> UploadedFile {
        self.0
    }

    /// Get a reference to the uploaded file.
    #[must_use]
    pub fn inner(&self) -> &UploadedFile {
        &self.0
    }
}

impl std::ops::Deref for File {
    type Target = UploadedFile;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Configuration for the File extractor.
#[derive(Debug, Clone)]
pub struct FileConfig {
    field_name: String,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            field_name: "file".to_string(),
        }
    }
}

impl FileConfig {
    /// Create a new file config with the given field name.
    #[must_use]
    pub fn new(field_name: impl Into<String>) -> Self {
        Self {
            field_name: field_name.into(),
        }
    }

    /// Get the field name.
    #[must_use]
    pub fn field_name(&self) -> &str {
        &self.field_name
    }
}

impl FromRequest for File {
    type Error = MultipartExtractError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let field_name = req
            .get_extension::<FileConfig>()
            .map(|c| c.field_name.clone())
            .unwrap_or_else(|| "file".to_string());

        let multipart = Multipart::from_request(ctx, req).await?;

        let file = multipart
            .get_file(&field_name)
            .ok_or(MultipartExtractError::FileNotFound { field_name })?;

        Ok(File(file))
    }
}

/// Parse boundary from Content-Type header.
fn parse_multipart_boundary(content_type: &str) -> Result<String, MultipartExtractError> {
    for part in content_type.split(';') {
        let part = part.trim();
        if let Some(boundary) = part
            .strip_prefix("boundary=")
            .or_else(|| part.strip_prefix("BOUNDARY="))
        {
            let boundary = boundary.trim_matches('"').trim_matches('\'');
            if boundary.is_empty() {
                return Err(MultipartExtractError::MissingBoundary);
            }
            return Ok(boundary.to_string());
        }
    }
    Err(MultipartExtractError::MissingBoundary)
}

/// Parse multipart body into parts.
fn parse_multipart_body(
    body: &[u8],
    boundary: &str,
    config: &MultipartConfig,
) -> Result<Vec<MultipartPart>, MultipartExtractError> {
    let boundary_bytes = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut total_size = 0usize;
    let mut pos = 0;

    // Find first boundary
    pos = find_bytes(body, &boundary_bytes, pos).ok_or_else(|| {
        MultipartExtractError::InvalidFormat {
            detail: "no boundary found".to_string(),
        }
    })?;

    loop {
        // Check field limit
        if parts.len() >= config.max_fields {
            return Err(MultipartExtractError::TooManyFields {
                count: parts.len() + 1,
                limit: config.max_fields,
            });
        }

        // Check if this is the final boundary (--boundary--)
        let boundary_end = pos + boundary_bytes.len();
        if boundary_end + 2 <= body.len() && body[boundary_end..boundary_end + 2] == *b"--" {
            break;
        }

        // Skip boundary and CRLF
        pos = boundary_end;
        if pos + 2 > body.len() {
            return Err(MultipartExtractError::InvalidFormat {
                detail: "unexpected end after boundary".to_string(),
            });
        }
        if body[pos..pos + 2] != *b"\r\n" {
            return Err(MultipartExtractError::InvalidFormat {
                detail: "expected CRLF after boundary".to_string(),
            });
        }
        pos += 2;

        // Parse headers
        let mut name = None;
        let mut filename = None;
        let mut content_type = None;

        loop {
            let line_end =
                find_crlf(body, pos).ok_or_else(|| MultipartExtractError::InvalidFormat {
                    detail: "unterminated headers".to_string(),
                })?;

            let line = &body[pos..line_end];
            if line.is_empty() {
                pos = line_end + 2;
                break;
            }

            if let Ok(line_str) = std::str::from_utf8(line) {
                if let Some((header_name, header_value)) = line_str.split_once(':') {
                    let header_name = header_name.trim().to_ascii_lowercase();
                    let header_value = header_value.trim();

                    if header_name == "content-disposition" {
                        (name, filename) = parse_content_disposition_header(header_value);
                    } else if header_name == "content-type" {
                        content_type = Some(header_value.to_string());
                    }
                }
            }

            pos = line_end + 2;
        }

        let name = name.ok_or_else(|| MultipartExtractError::InvalidFormat {
            detail: "missing Content-Disposition name".to_string(),
        })?;

        // Find next boundary
        let data_end = find_bytes(body, &boundary_bytes, pos).ok_or_else(|| {
            MultipartExtractError::InvalidFormat {
                detail: "missing closing boundary".to_string(),
            }
        })?;

        // Data ends before \r\n--boundary
        let data = if data_end >= 2 && body[data_end - 2..data_end] == *b"\r\n" {
            &body[pos..data_end - 2]
        } else {
            &body[pos..data_end]
        };

        // Check size limits for files
        if filename.is_some() && data.len() > config.max_file_size {
            return Err(MultipartExtractError::FileTooLarge {
                size: data.len(),
                limit: config.max_file_size,
            });
        }

        total_size += data.len();
        if total_size > config.max_total_size {
            return Err(MultipartExtractError::TotalTooLarge {
                size: total_size,
                limit: config.max_total_size,
            });
        }

        parts.push(MultipartPart {
            name,
            filename,
            content_type,
            data: data.to_vec(),
        });

        pos = data_end;
    }

    Ok(parts)
}

/// Find a byte sequence in data starting from position.
///
/// Uses memchr for SIMD-accelerated searching when the needle starts with
/// a specific byte, providing 2-5x speedup over naive linear search.
fn find_bytes(data: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start);
    }
    if start >= data.len() {
        return None;
    }
    // Use memchr to find candidate positions for the first byte,
    // then verify the full needle match
    let first_byte = needle[0];
    let search_slice = &data[start..];

    if needle.len() == 1 {
        // Single byte search - pure memchr
        return memchr::memchr(first_byte, search_slice).map(|pos| pos + start);
    }

    // Multi-byte needle: find first byte candidates, then verify
    let mut search_offset = 0;
    while let Some(pos) = memchr::memchr(first_byte, &search_slice[search_offset..]) {
        let abs_pos = start + search_offset + pos;
        if abs_pos + needle.len() > data.len() {
            return None;
        }
        if data[abs_pos..].starts_with(needle) {
            return Some(abs_pos);
        }
        search_offset += pos + 1;
    }
    None
}

/// Find CRLF in data starting from position.
///
/// Uses memchr for SIMD-accelerated CR byte search.
fn find_crlf(data: &[u8], start: usize) -> Option<usize> {
    if start >= data.len().saturating_sub(1) {
        return None;
    }
    let search_slice = &data[start..];

    // Find '\r' candidates using SIMD, then verify '\n' follows
    let mut search_offset = 0;
    while let Some(pos) = memchr::memchr(b'\r', &search_slice[search_offset..]) {
        let abs_pos = start + search_offset + pos;
        if abs_pos + 1 < data.len() && data[abs_pos + 1] == b'\n' {
            return Some(abs_pos);
        }
        search_offset += pos + 1;
        if search_offset >= search_slice.len().saturating_sub(1) {
            break;
        }
    }
    None
}

/// Parse Content-Disposition header value.
fn parse_content_disposition_header(value: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut filename = None;

    for part in value.split(';') {
        let part = part.trim();
        if let Some(n) = part
            .strip_prefix("name=")
            .or_else(|| part.strip_prefix("NAME="))
        {
            name = Some(unquote_param(n));
        } else if let Some(f) = part
            .strip_prefix("filename=")
            .or_else(|| part.strip_prefix("FILENAME="))
        {
            filename = Some(unquote_param(f));
        }
    }

    (name, filename)
}

/// Remove quotes from a parameter value.
fn unquote_param(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod multipart_tests {
    use super::*;
    use crate::RequestContext;
    use crate::request::Method;
    use asupersync::Cx;

    fn test_context() -> RequestContext {
        RequestContext::new(Cx::for_testing(), 1)
    }

    #[test]
    fn test_parse_boundary() {
        let ct = "multipart/form-data; boundary=----WebKit";
        let boundary = parse_multipart_boundary(ct).unwrap();
        assert_eq!(boundary, "----WebKit");
    }

    #[test]
    fn test_parse_boundary_quoted() {
        let ct = r#"multipart/form-data; boundary="simple""#;
        let boundary = parse_multipart_boundary(ct).unwrap();
        assert_eq!(boundary, "simple");
    }

    #[test]
    fn test_parse_boundary_missing() {
        let ct = "multipart/form-data";
        let result = parse_multipart_boundary(ct);
        assert!(matches!(
            result,
            Err(MultipartExtractError::MissingBoundary)
        ));
    }

    #[test]
    fn test_parse_simple_form() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"field1\"\r\n",
            "\r\n",
            "value1\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"field2\"\r\n",
            "\r\n",
            "value2\r\n",
            "------boundary--\r\n"
        );

        let config = MultipartConfig::default();
        let parts = parse_multipart_body(body.as_bytes(), boundary, &config).unwrap();

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "field1");
        assert_eq!(std::str::from_utf8(&parts[0].data).unwrap(), "value1");
        assert_eq!(parts[1].name, "field2");
        assert_eq!(std::str::from_utf8(&parts[1].data).unwrap(), "value2");
    }

    #[test]
    fn test_parse_file_upload() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "Hello!\r\n",
            "------boundary--\r\n"
        );

        let config = MultipartConfig::default();
        let parts = parse_multipart_body(body.as_bytes(), boundary, &config).unwrap();

        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].name, "file");
        assert_eq!(parts[0].filename, Some("test.txt".to_string()));
        assert_eq!(parts[0].content_type, Some("text/plain".to_string()));
        assert_eq!(std::str::from_utf8(&parts[0].data).unwrap(), "Hello!");
    }

    #[test]
    fn test_multipart_extractor() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"name\"\r\n",
            "\r\n",
            "John\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"avatar\"; filename=\"pic.jpg\"\r\n",
            "Content-Type: image/jpeg\r\n",
            "\r\n",
            "JPEG\r\n",
            "------boundary--\r\n"
        );

        let config = MultipartConfig::default();
        let parts = parse_multipart_body(body.as_bytes(), boundary, &config).unwrap();
        let form = Multipart::from_parts(parts);

        assert_eq!(form.get_field("name"), Some("John"));
        let file = form.get_file("avatar").unwrap();
        assert_eq!(file.filename(), "pic.jpg");
        assert_eq!(file.content_type(), "image/jpeg");
    }

    #[test]
    fn test_file_size_limit() {
        let boundary = "----boundary";
        let large = "x".repeat(1000);
        let body = format!(
            "------boundary\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"big.txt\"\r\n\
             \r\n\
             {}\r\n\
             ------boundary--\r\n",
            large
        );

        let config = MultipartConfig::default().max_file_size(100);
        let result = parse_multipart_body(body.as_bytes(), boundary, &config);

        assert!(matches!(
            result,
            Err(MultipartExtractError::FileTooLarge { .. })
        ));
    }

    #[test]
    fn test_total_size_limit() {
        let boundary = "----boundary";
        let data = "x".repeat(500);
        let body = format!(
            "------boundary\r\n\
             Content-Disposition: form-data; name=\"f1\"; filename=\"a.txt\"\r\n\
             \r\n\
             {}\r\n\
             ------boundary\r\n\
             Content-Disposition: form-data; name=\"f2\"; filename=\"b.txt\"\r\n\
             \r\n\
             {}\r\n\
             ------boundary--\r\n",
            data, data
        );

        let config = MultipartConfig::default()
            .max_file_size(1000)
            .max_total_size(800);
        let result = parse_multipart_body(body.as_bytes(), boundary, &config);

        assert!(matches!(
            result,
            Err(MultipartExtractError::TotalTooLarge { .. })
        ));
    }

    #[test]
    fn test_field_count_limit() {
        let boundary = "----boundary";
        let mut body = String::new();
        for i in 0..5 {
            body.push_str(&format!(
                "------boundary\r\n\
                 Content-Disposition: form-data; name=\"f{}\"\r\n\
                 \r\n\
                 v{}\r\n",
                i, i
            ));
        }
        body.push_str("------boundary--\r\n");

        let config = MultipartConfig::default().max_fields(3);
        let result = parse_multipart_body(body.as_bytes(), boundary, &config);

        assert!(matches!(
            result,
            Err(MultipartExtractError::TooManyFields { .. })
        ));
    }

    #[test]
    fn test_uploaded_file_extension() {
        let file = UploadedFile::new(
            "doc".to_string(),
            "report.pdf".to_string(),
            "application/pdf".to_string(),
            vec![],
        );
        assert_eq!(file.extension(), Some("pdf"));

        let no_ext = UploadedFile::new(
            "doc".to_string(),
            "README".to_string(),
            "text/plain".to_string(),
            vec![],
        );
        assert_eq!(no_ext.extension(), None);
    }

    #[test]
    fn test_multipart_from_request_wrong_content_type() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"{}".to_vec()));

        let result = futures_executor::block_on(Multipart::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(MultipartExtractError::UnsupportedMediaType { .. })
        ));
    }

    #[test]
    fn test_file_extractor() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"doc.pdf\"\r\n",
            "Content-Type: application/pdf\r\n",
            "\r\n",
            "PDF content\r\n",
            "------boundary--\r\n"
        );

        let config = MultipartConfig::default();
        let parts = parse_multipart_body(body.as_bytes(), boundary, &config).unwrap();
        let form = Multipart::from_parts(parts);

        let file = form.get_file("file").unwrap();
        assert_eq!(file.filename(), "doc.pdf");
        assert_eq!(file.content_type(), "application/pdf");
        assert_eq!(file.text(), Some("PDF content"));
    }

    #[test]
    fn test_multiple_files() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"files\"; filename=\"a.txt\"\r\n",
            "\r\n",
            "file a\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"files\"; filename=\"b.txt\"\r\n",
            "\r\n",
            "file b\r\n",
            "------boundary--\r\n"
        );

        let config = MultipartConfig::default();
        let parts = parse_multipart_body(body.as_bytes(), boundary, &config).unwrap();
        let form = Multipart::from_parts(parts);

        let files = form.get_files("files");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename(), "a.txt");
        assert_eq!(files[1].filename(), "b.txt");
    }
}

// ============================================================================
// Path Parameter Extractor
// ============================================================================

/// Extracted path parameters stored in request extensions.
///
/// This type is set by the router after matching a route and extracting
/// path parameters. The [`Path`] extractor retrieves this from the request.
///
/// # Example
///
/// For a route `/users/{user_id}/posts/{post_id}` matched against
/// `/users/42/posts/99`, this would contain:
/// `[("user_id", "42"), ("post_id", "99")]`
#[derive(Debug, Clone, Default)]
pub struct PathParams(pub Vec<(String, String)>);

impl PathParams {
    /// Create empty path parameters.
    #[must_use]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Create from a vector of name-value pairs.
    #[must_use]
    pub fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        Self(pairs)
    }

    /// Get a parameter value by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Get all parameters as a slice.
    #[must_use]
    pub fn as_slice(&self) -> &[(String, String)] {
        &self.0
    }

    /// Returns true if there are no parameters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the number of parameters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// Path parameter extractor.
///
/// Extracts path parameters from the URL and deserializes them to type `T`.
///
/// # Supported Types
///
/// - **Single value**: `Path<i64>` extracts the first (or only) path parameter
/// - **Tuple**: `Path<(String, i64)>` extracts parameters in order
/// - **Struct**: `Path<MyParams>` extracts parameters by field name
///
/// # Error Responses
///
/// - **500 Internal Server Error**: Path parameters not set by router (server bug)
/// - **422 Unprocessable Entity**: Parameter missing or type conversion failed
///
/// # Examples
///
/// ## Single Parameter
///
/// ```ignore
/// #[get("/users/{id}")]
/// async fn get_user(Path(id): Path<i64>) -> impl IntoResponse {
///     format!("User ID: {id}")
/// }
/// ```
///
/// ## Multiple Parameters (Tuple)
///
/// ```ignore
/// #[get("/users/{user_id}/posts/{post_id}")]
/// async fn get_post(Path((user_id, post_id)): Path<(i64, i64)>) -> impl IntoResponse {
///     format!("User {user_id}, Post {post_id}")
/// }
/// ```
///
/// ## Struct Extraction
///
/// ```ignore
/// #[derive(Deserialize)]
/// struct PostPath {
///     user_id: i64,
///     post_id: i64,
/// }
///
/// #[get("/users/{user_id}/posts/{post_id}")]
/// async fn get_post(Path(path): Path<PostPath>) -> impl IntoResponse {
///     format!("User {}, Post {}", path.user_id, path.post_id)
/// }
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Path<T>(pub T);

impl<T> Path<T> {
    /// Unwrap the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Path<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Path<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Error returned when path extraction fails.
#[derive(Debug)]
pub enum PathExtractError {
    /// Path parameters not available in request extensions.
    /// This indicates a server configuration error (router not setting params).
    MissingPathParams,
    /// A required parameter was not found.
    MissingParam {
        /// The parameter name that was missing.
        name: String,
    },
    /// Parameter value could not be converted to the expected type.
    InvalidValue {
        /// The parameter name.
        name: String,
        /// The actual value that couldn't be converted.
        value: String,
        /// Description of the expected type.
        expected: &'static str,
        /// Additional error details.
        message: String,
    },
    /// Deserialization error (e.g., wrong number of parameters for tuple).
    DeserializeError {
        /// The error message.
        message: String,
    },
}

impl fmt::Display for PathExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPathParams => {
                write!(f, "Path parameters not available in request")
            }
            Self::MissingParam { name } => {
                write!(f, "Missing path parameter: {name}")
            }
            Self::InvalidValue {
                name,
                value,
                expected,
                message,
            } => {
                write!(
                    f,
                    "Invalid value for path parameter '{name}': expected {expected}, got '{value}': {message}"
                )
            }
            Self::DeserializeError { message } => {
                write!(f, "Path deserialization error: {message}")
            }
        }
    }
}

impl std::error::Error for PathExtractError {}

impl IntoResponse for PathExtractError {
    fn into_response(self) -> crate::response::Response {
        match self {
            Self::MissingPathParams => {
                // Server bug - path params should always be set by router
                HttpError::internal()
                    .with_detail("Path parameters not available")
                    .into_response()
            }
            Self::MissingParam { name } => ValidationErrors::single(
                ValidationError::missing(crate::error::loc::path(&name))
                    .with_msg("Path parameter is required"),
            )
            .into_response(),
            Self::InvalidValue {
                name,
                value,
                expected,
                message,
            } => ValidationErrors::single(
                ValidationError::type_error(crate::error::loc::path(&name), &expected)
                    .with_msg(format!("Expected {expected}: {message}"))
                    .with_input(serde_json::Value::String(value)),
            )
            .into_response(),
            Self::DeserializeError { message } => ValidationErrors::single(
                ValidationError::new(
                    crate::error::error_types::VALUE_ERROR,
                    vec![crate::error::LocItem::field("path")],
                )
                .with_msg(message),
            )
            .into_response(),
        }
    }
}

impl<T: DeserializeOwned> FromRequest for Path<T> {
    type Error = PathExtractError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get path params from request extensions
        let params = req
            .get_extension::<PathParams>()
            .ok_or(PathExtractError::MissingPathParams)?
            .clone();

        // Deserialize using our custom deserializer
        let value = T::deserialize(PathDeserializer::new(&params))?;

        Ok(Path(value))
    }
}

// ============================================================================
// Path Parameter Deserializer
// ============================================================================

/// Custom serde deserializer for path parameters.
///
/// Handles three modes:
/// - Single value: Deserializes the first parameter value
/// - Sequence (tuple): Deserializes parameters in order
/// - Map (struct): Deserializes parameters by name
struct PathDeserializer<'de> {
    params: &'de PathParams,
}

impl<'de> PathDeserializer<'de> {
    fn new(params: &'de PathParams) -> Self {
        Self { params }
    }
}

impl<'de> Deserializer<'de> for PathDeserializer<'de> {
    type Error = PathExtractError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // Default: try as map (struct)
        self.deserialize_map(visitor)
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let b = value
            .parse::<bool>()
            .map_err(|_| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "boolean",
                message: "expected 'true' or 'false'".to_string(),
            })?;
        visitor.visit_bool(b)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<i8>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "i8",
                message: e.to_string(),
            })?;
        visitor.visit_i8(n)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<i16>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "i16",
                message: e.to_string(),
            })?;
        visitor.visit_i16(n)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<i32>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "i32",
                message: e.to_string(),
            })?;
        visitor.visit_i32(n)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<i64>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "i64",
                message: e.to_string(),
            })?;
        visitor.visit_i64(n)
    }

    fn deserialize_i128<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<i128>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "i128",
                message: e.to_string(),
            })?;
        visitor.visit_i128(n)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<u8>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "u8",
                message: e.to_string(),
            })?;
        visitor.visit_u8(n)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<u16>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "u16",
                message: e.to_string(),
            })?;
        visitor.visit_u16(n)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<u32>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "u32",
                message: e.to_string(),
            })?;
        visitor.visit_u32(n)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<u64>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "u64",
                message: e.to_string(),
            })?;
        visitor.visit_u64(n)
    }

    fn deserialize_u128<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<u128>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "u128",
                message: e.to_string(),
            })?;
        visitor.visit_u128(n)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<f32>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "f32",
                message: e.to_string(),
            })?;
        visitor.visit_f32(n)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<f64>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "f64",
                message: e.to_string(),
            })?;
        visitor.visit_f64(n)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let mut chars = value.chars();
        let c = chars.next().ok_or_else(|| PathExtractError::InvalidValue {
            name: self.get_first_name(),
            value: value.to_string(),
            expected: "char",
            message: "empty string".to_string(),
        })?;
        if chars.next().is_some() {
            return Err(PathExtractError::InvalidValue {
                name: self.get_first_name(),
                value: value.to_string(),
                expected: "char",
                message: "expected single character".to_string(),
            });
        }
        visitor.visit_char(c)
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        visitor.visit_str(value)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        visitor.visit_string(value.to_string())
    }

    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(PathExtractError::DeserializeError {
            message: "bytes deserialization not supported for path parameters".to_string(),
        })
    }

    fn deserialize_byte_buf<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(PathExtractError::DeserializeError {
            message: "byte_buf deserialization not supported for path parameters".to_string(),
        })
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // Path params are always present, so always Some
        visitor.visit_some(self)
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(PathSeqAccess::new(self.params))
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(PathSeqAccess::new(self.params))
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(PathSeqAccess::new(self.params))
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_map(PathMapAccess::new(self.params))
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_map(PathMapAccess::new(self.params))
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        visitor.visit_enum(value.into_deserializer())
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }
}

impl PathDeserializer<'_> {
    fn get_single_value(&self) -> Result<&str, PathExtractError> {
        self.params
            .0
            .first()
            .map(|(_, v)| v.as_str())
            .ok_or_else(|| PathExtractError::DeserializeError {
                message: "no path parameters available".to_string(),
            })
    }

    fn get_first_name(&self) -> String {
        self.params
            .0
            .first()
            .map_or_else(|| "unknown".to_string(), |(n, _)| n.clone())
    }
}

impl de::Error for PathExtractError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        PathExtractError::DeserializeError {
            message: msg.to_string(),
        }
    }
}

/// Sequence access for deserializing tuples from path params.
struct PathSeqAccess<'de> {
    params: &'de PathParams,
    index: usize,
}

impl<'de> PathSeqAccess<'de> {
    fn new(params: &'de PathParams) -> Self {
        Self { params, index: 0 }
    }
}

impl<'de> SeqAccess<'de> for PathSeqAccess<'de> {
    type Error = PathExtractError;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: de::DeserializeSeed<'de>,
    {
        if self.index >= self.params.0.len() {
            return Ok(None);
        }

        let (name, value) = &self.params.0[self.index];
        self.index += 1;

        seed.deserialize(PathValueDeserializer::new(name, value))
            .map(Some)
    }
}

/// Map access for deserializing structs from path params.
struct PathMapAccess<'de> {
    params: &'de PathParams,
    index: usize,
}

impl<'de> PathMapAccess<'de> {
    fn new(params: &'de PathParams) -> Self {
        Self { params, index: 0 }
    }
}

impl<'de> MapAccess<'de> for PathMapAccess<'de> {
    type Error = PathExtractError;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: de::DeserializeSeed<'de>,
    {
        if self.index >= self.params.0.len() {
            return Ok(None);
        }

        let (name, _) = &self.params.0[self.index];
        seed.deserialize(name.as_str().into_deserializer())
            .map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: de::DeserializeSeed<'de>,
    {
        let (name, value) = &self.params.0[self.index];
        self.index += 1;

        seed.deserialize(PathValueDeserializer::new(name, value))
    }
}

/// Deserializer for a single path parameter value.
struct PathValueDeserializer<'de> {
    name: &'de str,
    value: &'de str,
}

impl<'de> PathValueDeserializer<'de> {
    fn new(name: &'de str, value: &'de str) -> Self {
        Self { name, value }
    }
}

impl<'de> Deserializer<'de> for PathValueDeserializer<'de> {
    type Error = PathExtractError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // Default to string
        visitor.visit_str(self.value)
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let b = self
            .value
            .parse::<bool>()
            .map_err(|_| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "boolean",
                message: "expected 'true' or 'false'".to_string(),
            })?;
        visitor.visit_bool(b)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<i8>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "i8",
                message: e.to_string(),
            })?;
        visitor.visit_i8(n)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<i16>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "i16",
                message: e.to_string(),
            })?;
        visitor.visit_i16(n)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<i32>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "i32",
                message: e.to_string(),
            })?;
        visitor.visit_i32(n)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<i64>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "i64",
                message: e.to_string(),
            })?;
        visitor.visit_i64(n)
    }

    fn deserialize_i128<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<i128>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "i128",
                message: e.to_string(),
            })?;
        visitor.visit_i128(n)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<u8>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "u8",
                message: e.to_string(),
            })?;
        visitor.visit_u8(n)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<u16>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "u16",
                message: e.to_string(),
            })?;
        visitor.visit_u16(n)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<u32>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "u32",
                message: e.to_string(),
            })?;
        visitor.visit_u32(n)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<u64>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "u64",
                message: e.to_string(),
            })?;
        visitor.visit_u64(n)
    }

    fn deserialize_u128<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<u128>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "u128",
                message: e.to_string(),
            })?;
        visitor.visit_u128(n)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<f32>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "f32",
                message: e.to_string(),
            })?;
        visitor.visit_f32(n)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<f64>()
            .map_err(|e| PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "f64",
                message: e.to_string(),
            })?;
        visitor.visit_f64(n)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let mut chars = self.value.chars();
        let c = chars.next().ok_or_else(|| PathExtractError::InvalidValue {
            name: self.name.to_string(),
            value: self.value.to_string(),
            expected: "char",
            message: "empty string".to_string(),
        })?;
        if chars.next().is_some() {
            return Err(PathExtractError::InvalidValue {
                name: self.name.to_string(),
                value: self.value.to_string(),
                expected: "char",
                message: "expected single character".to_string(),
            });
        }
        visitor.visit_char(c)
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_str(self.value)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_string(self.value.to_string())
    }

    fn deserialize_bytes<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(PathExtractError::DeserializeError {
            message: "bytes deserialization not supported for path parameters".to_string(),
        })
    }

    fn deserialize_byte_buf<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(PathExtractError::DeserializeError {
            message: "byte_buf deserialization not supported for path parameters".to_string(),
        })
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(PathExtractError::DeserializeError {
            message: "sequence deserialization not supported for single path parameter".to_string(),
        })
    }

    fn deserialize_tuple<V>(self, _len: usize, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(PathExtractError::DeserializeError {
            message: "tuple deserialization not supported for single path parameter".to_string(),
        })
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(PathExtractError::DeserializeError {
            message: "tuple struct deserialization not supported for single path parameter"
                .to_string(),
        })
    }

    fn deserialize_map<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(PathExtractError::DeserializeError {
            message: "map deserialization not supported for single path parameter".to_string(),
        })
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(PathExtractError::DeserializeError {
            message: "struct deserialization not supported for single path parameter".to_string(),
        })
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_enum(self.value.into_deserializer())
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_str(self.value)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }
}

// ============================================================================
// Query String Extractor
// ============================================================================

/// Query string extractor.
///
/// Extracts and deserializes query string parameters into a typed struct.
/// This extractor uses serde for deserialization, so the target type must
/// implement `DeserializeOwned`.
///
/// # Features
///
/// - **Optional fields**: Use `Option<T>` for optional parameters
/// - **Multi-value**: Use `Vec<T>` for parameters that appear multiple times
/// - **Default values**: Use `#[serde(default)]` for default values
/// - **Percent-decoding**: Values are automatically percent-decoded
///
/// # Example
///
/// ```ignore
/// use fastapi_core::Query;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct SearchParams {
///     q: String,                      // Required
///     page: Option<i32>,              // Optional
///     #[serde(default)]
///     limit: i32,                     // Default (0)
///     tags: Vec<String>,              // Multi-value: ?tags=a&tags=b
/// }
///
/// #[get("/search")]
/// async fn search(cx: &Cx, params: Query<SearchParams>) -> impl IntoResponse {
///     // Access the inner value via params.0 or *params
///     let query = &params.q;
///     // ...
/// }
/// ```
///
/// # Error Handling
///
/// Returns HTTP 422 (Unprocessable Entity) when:
/// - Required fields are missing
/// - Type conversion fails (e.g., "abc" to i32)
/// - Serde deserialization fails
#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

impl<T> Query<T> {
    /// Create a new Query extractor with the given value.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Consume the extractor and return the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Query<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Error type for query string extraction failures.
#[derive(Debug)]
pub enum QueryExtractError {
    /// A required parameter is missing.
    MissingParam { name: String },
    /// A parameter value could not be converted to the expected type.
    InvalidValue {
        name: String,
        value: String,
        expected: &'static str,
        message: String,
    },
    /// Serde deserialization failed.
    DeserializeError { message: String },
}

impl fmt::Display for QueryExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingParam { name } => {
                write!(f, "Missing required query parameter: {}", name)
            }
            Self::InvalidValue {
                name,
                value,
                expected,
                message,
            } => {
                write!(
                    f,
                    "Invalid value '{}' for query parameter '{}' (expected {}): {}",
                    value, name, expected, message
                )
            }
            Self::DeserializeError { message } => {
                write!(f, "Query deserialization error: {}", message)
            }
        }
    }
}

impl std::error::Error for QueryExtractError {}

impl de::Error for QueryExtractError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::DeserializeError {
            message: msg.to_string(),
        }
    }
}

impl IntoResponse for QueryExtractError {
    fn into_response(self) -> crate::response::Response {
        match self {
            Self::MissingParam { name } => ValidationErrors::single(
                ValidationError::missing(crate::error::loc::query(&name))
                    .with_msg("Query parameter is required"),
            )
            .into_response(),
            Self::InvalidValue {
                name,
                value,
                expected,
                message,
            } => ValidationErrors::single(
                ValidationError::type_error(crate::error::loc::query(&name), &expected)
                    .with_msg(format!("Expected {expected}: {message}"))
                    .with_input(serde_json::Value::String(value)),
            )
            .into_response(),
            Self::DeserializeError { message } => ValidationErrors::single(
                ValidationError::new(
                    crate::error::error_types::VALUE_ERROR,
                    vec![crate::error::LocItem::field("query")],
                )
                .with_msg(message),
            )
            .into_response(),
        }
    }
}

/// Stored query parameters for extraction.
///
/// Similar to `PathParams` but handles multi-value parameters.
/// Stored in request extensions by the framework.
#[derive(Debug, Clone, Default)]
pub struct QueryParams {
    /// Params stored as Vec to preserve order and handle duplicates.
    params: Vec<(String, String)>,
}

impl QueryParams {
    /// Create empty query params.
    pub fn new() -> Self {
        Self { params: Vec::new() }
    }

    /// Create from a vector of key-value pairs.
    pub fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        Self { params: pairs }
    }

    /// Parse from a query string (without leading '?').
    pub fn parse(query: &str) -> Self {
        let pairs: Vec<(String, String)> = query
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|pair| {
                if let Some(eq_pos) = pair.find('=') {
                    let key = &pair[..eq_pos];
                    let value = &pair[eq_pos + 1..];
                    (
                        percent_decode(key).into_owned(),
                        percent_decode(value).into_owned(),
                    )
                } else {
                    // Key without value: "flag" -> ("flag", "")
                    (percent_decode(pair).into_owned(), String::new())
                }
            })
            .collect();
        Self { params: pairs }
    }

    /// Get the first value for a key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Get all values for a key.
    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.params
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// Check if a key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.params.iter().any(|(k, _)| k == key)
    }

    /// Get all key-value pairs.
    pub fn pairs(&self) -> &[(String, String)] {
        &self.params
    }

    /// Get iterator over unique keys.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        let mut seen = std::collections::HashSet::new();
        self.params.iter().filter_map(move |(k, _)| {
            if seen.insert(k.as_str()) {
                Some(k.as_str())
            } else {
                None
            }
        })
    }

    /// Return the number of parameters (including duplicates).
    pub fn len(&self) -> usize {
        self.params.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }
}

/// Percent-decode a string.
///
/// Returns a `Cow::Borrowed` if no decoding was needed,
/// or `Cow::Owned` if percent sequences were decoded.
fn percent_decode(s: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;

    // Fast path: no encoding
    if !s.contains('%') && !s.contains('+') {
        return Cow::Borrowed(s);
    }

    let mut result = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                // Try to decode hex pair
                if let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                    result.push(hi << 4 | lo);
                    i += 3;
                } else {
                    // Invalid hex, keep as-is
                    result.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                // Plus as space (application/x-www-form-urlencoded)
                result.push(b' ');
                i += 1;
            }
            b => {
                result.push(b);
                i += 1;
            }
        }
    }

    Cow::Owned(String::from_utf8_lossy(&result).into_owned())
}

/// Convert a hex digit to its numeric value.
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

impl<T: DeserializeOwned> FromRequest for Query<T> {
    type Error = QueryExtractError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get or parse query params
        let params = match req.get_extension::<QueryParams>() {
            Some(p) => p.clone(),
            None => {
                // Parse from request query string
                let query_str = req.query().unwrap_or("");
                QueryParams::parse(query_str)
            }
        };

        // Deserialize using our custom deserializer
        let value = T::deserialize(QueryDeserializer::new(&params))?;

        Ok(Query(value))
    }
}

// ============================================================================
// Query String Deserializer
// ============================================================================

/// Custom serde deserializer for query string parameters.
///
/// Handles:
/// - Single values: Deserializes from first matching parameter
/// - Sequences (Vec): Collects all values for a parameter
/// - Maps/Structs: Deserializes parameters by name
/// - Options: Missing parameters become None
struct QueryDeserializer<'de> {
    params: &'de QueryParams,
}

impl<'de> QueryDeserializer<'de> {
    fn new(params: &'de QueryParams) -> Self {
        Self { params }
    }
}

impl<'de> Deserializer<'de> for QueryDeserializer<'de> {
    type Error = QueryExtractError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // Default: try as map (struct)
        self.deserialize_map(visitor)
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .params
            .pairs()
            .first()
            .map(|(_, v)| v.as_str())
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: "value".to_string(),
            })?;

        let b = parse_bool(value).map_err(|msg| QueryExtractError::InvalidValue {
            name: "value".to_string(),
            value: value.to_string(),
            expected: "bool",
            message: msg,
        })?;
        visitor.visit_bool(b)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<i8>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "i8",
                message: e.to_string(),
            })?;
        visitor.visit_i8(n)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<i16>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "i16",
                message: e.to_string(),
            })?;
        visitor.visit_i16(n)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<i32>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "i32",
                message: e.to_string(),
            })?;
        visitor.visit_i32(n)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<i64>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "i64",
                message: e.to_string(),
            })?;
        visitor.visit_i64(n)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<u8>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "u8",
                message: e.to_string(),
            })?;
        visitor.visit_u8(n)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<u16>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "u16",
                message: e.to_string(),
            })?;
        visitor.visit_u16(n)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<u32>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "u32",
                message: e.to_string(),
            })?;
        visitor.visit_u32(n)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<u64>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "u64",
                message: e.to_string(),
            })?;
        visitor.visit_u64(n)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<f32>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "f32",
                message: e.to_string(),
            })?;
        visitor.visit_f32(n)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let n = value
            .parse::<f64>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "f64",
                message: e.to_string(),
            })?;
        visitor.visit_f64(n)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        let mut chars = value.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => visitor.visit_char(c),
            _ => Err(QueryExtractError::InvalidValue {
                name: "value".to_string(),
                value: value.to_string(),
                expected: "char",
                message: "expected single character".to_string(),
            }),
        }
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        visitor.visit_str(value)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        visitor.visit_string(value.to_owned())
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        visitor.visit_bytes(value.as_bytes())
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        visitor.visit_byte_buf(value.as_bytes().to_vec())
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // For top-level option, check if we have any params
        if self.params.is_empty() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // For a sequence at the top level, use all values
        let values: Vec<&str> = self
            .params
            .pairs()
            .iter()
            .map(|(_, v)| v.as_str())
            .collect();
        visitor.visit_seq(QuerySeqAccess::new(values))
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // For a tuple, use values in order
        let values: Vec<&str> = self
            .params
            .pairs()
            .iter()
            .map(|(_, v)| v.as_str())
            .collect();
        visitor.visit_seq(QuerySeqAccess::new(values))
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let values: Vec<&str> = self
            .params
            .pairs()
            .iter()
            .map(|(_, v)| v.as_str())
            .collect();
        visitor.visit_seq(QuerySeqAccess::new(values))
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_map(QueryMapAccess::new(self.params))
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // For enum, use the first value as a unit variant name
        let value = self.get_single_value()?;
        visitor.visit_enum(value.into_deserializer())
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self.get_single_value()?;
        visitor.visit_str(value)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }
}

impl<'de> QueryDeserializer<'de> {
    fn get_single_value(&self) -> Result<&'de str, QueryExtractError> {
        self.params
            .pairs()
            .first()
            .map(|(_, v)| v.as_str())
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: "value".to_string(),
            })
    }
}

/// Helper to parse boolean from string.
fn parse_bool(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" | "" => Ok(false),
        _ => Err(format!("cannot parse '{}' as boolean", s)),
    }
}

/// Sequence access for deserializing arrays/vectors from query params.
struct QuerySeqAccess<'de> {
    values: Vec<&'de str>,
    index: usize,
}

impl<'de> QuerySeqAccess<'de> {
    fn new(values: Vec<&'de str>) -> Self {
        Self { values, index: 0 }
    }
}

impl<'de> SeqAccess<'de> for QuerySeqAccess<'de> {
    type Error = QueryExtractError;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: de::DeserializeSeed<'de>,
    {
        if self.index >= self.values.len() {
            return Ok(None);
        }

        let value = self.values[self.index];
        self.index += 1;

        seed.deserialize(QueryValueDeserializer::new(value, None))
            .map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len() - self.index)
    }
}

/// Map access for deserializing structs from query params.
struct QueryMapAccess<'de> {
    params: &'de QueryParams,
    keys: Vec<&'de str>,
    index: usize,
}

impl<'de> QueryMapAccess<'de> {
    fn new(params: &'de QueryParams) -> Self {
        let keys: Vec<&str> = params.keys().collect();
        Self {
            params,
            keys,
            index: 0,
        }
    }
}

impl<'de> MapAccess<'de> for QueryMapAccess<'de> {
    type Error = QueryExtractError;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: de::DeserializeSeed<'de>,
    {
        if self.index >= self.keys.len() {
            return Ok(None);
        }

        let key = self.keys[self.index];
        seed.deserialize(key.into_deserializer()).map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: de::DeserializeSeed<'de>,
    {
        let key = self.keys[self.index];
        self.index += 1;

        // Get all values for this key to support Vec<T>
        let values = self.params.get_all(key);

        seed.deserialize(QueryFieldDeserializer::new(key, values))
    }
}

/// Deserializer for a single query parameter value.
struct QueryValueDeserializer<'de> {
    value: &'de str,
    name: Option<&'de str>,
}

impl<'de> QueryValueDeserializer<'de> {
    fn new(value: &'de str, name: Option<&'de str>) -> Self {
        Self { value, name }
    }

    fn field_name(&self) -> String {
        self.name.unwrap_or("value").to_string()
    }
}

impl<'de> Deserializer<'de> for QueryValueDeserializer<'de> {
    type Error = QueryExtractError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_str(self.value)
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let b = parse_bool(self.value).map_err(|msg| QueryExtractError::InvalidValue {
            name: self.field_name(),
            value: self.value.to_string(),
            expected: "bool",
            message: msg,
        })?;
        visitor.visit_bool(b)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<i8>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "i8",
                message: e.to_string(),
            })?;
        visitor.visit_i8(n)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<i16>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "i16",
                message: e.to_string(),
            })?;
        visitor.visit_i16(n)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<i32>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "i32",
                message: e.to_string(),
            })?;
        visitor.visit_i32(n)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<i64>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "i64",
                message: e.to_string(),
            })?;
        visitor.visit_i64(n)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<u8>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "u8",
                message: e.to_string(),
            })?;
        visitor.visit_u8(n)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<u16>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "u16",
                message: e.to_string(),
            })?;
        visitor.visit_u16(n)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<u32>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "u32",
                message: e.to_string(),
            })?;
        visitor.visit_u32(n)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<u64>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "u64",
                message: e.to_string(),
            })?;
        visitor.visit_u64(n)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<f32>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "f32",
                message: e.to_string(),
            })?;
        visitor.visit_f32(n)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let n = self
            .value
            .parse::<f64>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "f64",
                message: e.to_string(),
            })?;
        visitor.visit_f64(n)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let mut chars = self.value.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => visitor.visit_char(c),
            _ => Err(QueryExtractError::InvalidValue {
                name: self.field_name(),
                value: self.value.to_string(),
                expected: "char",
                message: "expected single character".to_string(),
            }),
        }
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_str(self.value)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_string(self.value.to_owned())
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_bytes(self.value.as_bytes())
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_byte_buf(self.value.as_bytes().to_vec())
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if self.value.is_empty() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // Single value as sequence of one
        visitor.visit_seq(QuerySeqAccess::new(vec![self.value]))
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(QuerySeqAccess::new(vec![self.value]))
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(QuerySeqAccess::new(vec![self.value]))
    }

    fn deserialize_map<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // Can't deserialize a single value as a map
        Err(QueryExtractError::DeserializeError {
            message: "cannot deserialize single value as map".to_string(),
        })
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_enum(self.value.into_deserializer())
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_str(self.value)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }
}

/// Deserializer for a query field that may have multiple values.
///
/// This handles the Vec<T> case: ?tags=a&tags=b -> tags: ["a", "b"]
struct QueryFieldDeserializer<'de> {
    name: &'de str,
    values: Vec<&'de str>,
}

impl<'de> QueryFieldDeserializer<'de> {
    fn new(name: &'de str, values: Vec<&'de str>) -> Self {
        Self { name, values }
    }
}

impl<'de> Deserializer<'de> for QueryFieldDeserializer<'de> {
    type Error = QueryExtractError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // Default to first value as string
        if let Some(value) = self.values.first() {
            visitor.visit_str(value)
        } else {
            visitor.visit_none()
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let b = parse_bool(value).map_err(|msg| QueryExtractError::InvalidValue {
            name: self.name.to_string(),
            value: (*value).to_string(),
            expected: "bool",
            message: msg,
        })?;
        visitor.visit_bool(b)
    }

    fn deserialize_i8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<i8>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "i8",
                message: e.to_string(),
            })?;
        visitor.visit_i8(n)
    }

    fn deserialize_i16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<i16>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "i16",
                message: e.to_string(),
            })?;
        visitor.visit_i16(n)
    }

    fn deserialize_i32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<i32>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "i32",
                message: e.to_string(),
            })?;
        visitor.visit_i32(n)
    }

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<i64>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "i64",
                message: e.to_string(),
            })?;
        visitor.visit_i64(n)
    }

    fn deserialize_u8<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<u8>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "u8",
                message: e.to_string(),
            })?;
        visitor.visit_u8(n)
    }

    fn deserialize_u16<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<u16>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "u16",
                message: e.to_string(),
            })?;
        visitor.visit_u16(n)
    }

    fn deserialize_u32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<u32>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "u32",
                message: e.to_string(),
            })?;
        visitor.visit_u32(n)
    }

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<u64>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "u64",
                message: e.to_string(),
            })?;
        visitor.visit_u64(n)
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<f32>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "f32",
                message: e.to_string(),
            })?;
        visitor.visit_f32(n)
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let n = value
            .parse::<f64>()
            .map_err(|e| QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "f64",
                message: e.to_string(),
            })?;
        visitor.visit_f64(n)
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        let mut chars = value.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => visitor.visit_char(c),
            _ => Err(QueryExtractError::InvalidValue {
                name: self.name.to_string(),
                value: (*value).to_string(),
                expected: "char",
                message: "expected single character".to_string(),
            }),
        }
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        visitor.visit_str(value)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        visitor.visit_string((*value).to_owned())
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        visitor.visit_bytes(value.as_bytes())
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        visitor.visit_byte_buf(value.as_bytes().to_vec())
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        if self.values.is_empty() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        // This is the Vec<T> case: return all values as a sequence
        visitor.visit_seq(QuerySeqAccess::new(self.values))
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(QuerySeqAccess::new(self.values))
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(QuerySeqAccess::new(self.values))
    }

    fn deserialize_map<V>(self, _visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(QueryExtractError::DeserializeError {
            message: "cannot deserialize query field as map".to_string(),
        })
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        visitor.visit_enum((*value).into_deserializer())
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        let value = self
            .values
            .first()
            .ok_or_else(|| QueryExtractError::MissingParam {
                name: self.name.to_string(),
            })?;
        visitor.visit_str(value)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_unit()
    }
}

// ============================================================================
// Application State Extractor
// ============================================================================

/// Application state container.
///
/// `AppState` holds typed state values that can be shared across request handlers.
/// State is typically set up when creating the application and injected into
/// requests by the router/server.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::{AppState, State};
/// use std::sync::Arc;
///
/// // Define your state types
/// struct DatabasePool { /* ... */ }
/// struct Config { api_key: String }
///
/// // Build the app state
/// let state = AppState::new()
///     .with(Arc::new(DatabasePool::new()))
///     .with(Arc::new(Config { api_key: "secret".into() }));
///
/// // In handlers, extract the state
/// async fn handler(db: State<Arc<DatabasePool>>, config: State<Arc<Config>>) {
///     // Use db and config...
/// }
/// ```
#[derive(Debug, Default, Clone)]
pub struct AppState {
    inner: std::sync::Arc<
        std::collections::HashMap<
            std::any::TypeId,
            std::sync::Arc<dyn std::any::Any + Send + Sync>,
        >,
    >,
}

impl AppState {
    /// Create an empty application state container.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(std::collections::HashMap::new()),
        }
    }

    /// Add a typed state value.
    ///
    /// The value must be `Send + Sync + 'static` to be safely shared across
    /// requests and threads.
    #[must_use]
    pub fn with<T: Send + Sync + 'static>(self, value: T) -> Self {
        let mut map = match std::sync::Arc::try_unwrap(self.inner) {
            Ok(map) => map,
            Err(arc) => (*arc).clone(),
        };
        map.insert(std::any::TypeId::of::<T>(), std::sync::Arc::new(value));
        Self {
            inner: std::sync::Arc::new(map),
        }
    }

    /// Get a reference to a typed state value.
    #[must_use]
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.inner
            .get(&std::any::TypeId::of::<T>())
            .and_then(|arc| arc.downcast_ref::<T>())
    }

    /// Check if state contains a value of type T.
    #[must_use]
    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.inner.contains_key(&std::any::TypeId::of::<T>())
    }

    /// Return the number of state values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns true if no state values are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// State extractor for application-wide shared state.
///
/// Extracts a typed state value from the application state stored in request
/// extensions. The state must have been previously registered with the application.
///
/// # Type Requirements
///
/// The type `T` must be `Clone + Send + Sync + 'static`.
///
/// # Error Responses
///
/// - **500 Internal Server Error**: State type not found (server configuration error)
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::State;
/// use std::sync::Arc;
///
/// struct DatabasePool { /* ... */ }
///
/// #[get("/users")]
/// async fn list_users(db: State<Arc<DatabasePool>>) -> impl IntoResponse {
///     // db.0 contains the Arc<DatabasePool>
///     let users = db.query_users().await;
///     Json(users)
/// }
/// ```
#[derive(Debug, Clone)]
pub struct State<T>(pub T);

impl<T> State<T> {
    /// Unwrap the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for State<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for State<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Error returned when state extraction fails.
#[derive(Debug)]
pub enum StateExtractError {
    /// Application state not found in request extensions.
    ///
    /// This indicates the server was not configured to inject state into requests.
    MissingAppState,
    /// Requested state type not found.
    ///
    /// The type was not registered with the application state.
    MissingStateType {
        /// The name of the type that was not found.
        type_name: &'static str,
    },
}

impl std::fmt::Display for StateExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingAppState => {
                write!(f, "Application state not configured in request")
            }
            Self::MissingStateType { type_name } => {
                write!(f, "State type not found: {type_name}")
            }
        }
    }
}

impl std::error::Error for StateExtractError {}

impl IntoResponse for StateExtractError {
    fn into_response(self) -> crate::response::Response {
        // State extraction failures are server configuration errors (500)
        HttpError::internal()
            .with_detail(self.to_string())
            .into_response()
    }
}

impl<T> FromRequest for State<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Error = StateExtractError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get the AppState from request extensions
        let app_state = req
            .get_extension::<AppState>()
            .ok_or(StateExtractError::MissingAppState)?;

        // Get the specific state type
        let value = app_state
            .get::<T>()
            .ok_or(StateExtractError::MissingStateType {
                type_name: std::any::type_name::<T>(),
            })?;

        Ok(State(value.clone()))
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    #[derive(Clone, Debug, PartialEq)]
    struct DatabasePool {
        connection_string: String,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct AppConfig {
        debug: bool,
        port: u16,
    }

    #[test]
    fn app_state_new_is_empty() {
        let state = AppState::new();
        assert!(state.is_empty());
        assert_eq!(state.len(), 0);
    }

    #[test]
    fn app_state_with_single_type() {
        let db = DatabasePool {
            connection_string: "postgres://localhost".into(),
        };
        let state = AppState::new().with(db.clone());

        assert!(!state.is_empty());
        assert_eq!(state.len(), 1);
        assert!(state.contains::<DatabasePool>());
        assert_eq!(state.get::<DatabasePool>(), Some(&db));
    }

    #[test]
    fn app_state_with_multiple_types() {
        let db = DatabasePool {
            connection_string: "postgres://localhost".into(),
        };
        let config = AppConfig {
            debug: true,
            port: 8080,
        };

        let state = AppState::new().with(db.clone()).with(config.clone());

        assert_eq!(state.len(), 2);
        assert_eq!(state.get::<DatabasePool>(), Some(&db));
        assert_eq!(state.get::<AppConfig>(), Some(&config));
    }

    #[test]
    fn app_state_get_missing_type() {
        let state = AppState::new().with(42i32);
        assert!(state.get::<String>().is_none());
        assert!(!state.contains::<String>());
    }

    #[test]
    fn state_deref() {
        let state = State(42i32);
        assert_eq!(*state, 42);
    }

    #[test]
    fn state_into_inner() {
        let state = State("hello".to_string());
        assert_eq!(state.into_inner(), "hello");
    }

    #[test]
    fn state_extract_success() {
        let ctx = test_context();
        let db = DatabasePool {
            connection_string: "postgres://localhost".into(),
        };
        let app_state = AppState::new().with(db.clone());

        let mut req = Request::new(Method::Get, "/test");
        req.insert_extension(app_state);

        let result =
            futures_executor::block_on(State::<DatabasePool>::from_request(&ctx, &mut req));
        let State(extracted) = result.unwrap();
        assert_eq!(extracted, db);
    }

    #[test]
    fn state_extract_multiple_types() {
        let ctx = test_context();
        let db = DatabasePool {
            connection_string: "postgres://localhost".into(),
        };
        let config = AppConfig {
            debug: true,
            port: 8080,
        };
        let app_state = AppState::new().with(db.clone()).with(config.clone());

        let mut req = Request::new(Method::Get, "/test");
        req.insert_extension(app_state);

        // Extract DatabasePool
        let result =
            futures_executor::block_on(State::<DatabasePool>::from_request(&ctx, &mut req));
        let State(extracted_db) = result.unwrap();
        assert_eq!(extracted_db, db);

        // Extract AppConfig
        let result = futures_executor::block_on(State::<AppConfig>::from_request(&ctx, &mut req));
        let State(extracted_config) = result.unwrap();
        assert_eq!(extracted_config, config);
    }

    #[test]
    fn state_extract_missing_app_state() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        // Don't insert AppState

        let result =
            futures_executor::block_on(State::<DatabasePool>::from_request(&ctx, &mut req));
        assert!(matches!(result, Err(StateExtractError::MissingAppState)));
    }

    #[test]
    fn state_extract_missing_type() {
        let ctx = test_context();
        let app_state = AppState::new().with(42i32);

        let mut req = Request::new(Method::Get, "/test");
        req.insert_extension(app_state);

        let result =
            futures_executor::block_on(State::<DatabasePool>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(StateExtractError::MissingStateType { .. })
        ));
    }

    #[test]
    fn state_error_display() {
        let err = StateExtractError::MissingAppState;
        assert!(err.to_string().contains("not configured"));

        let err = StateExtractError::MissingStateType {
            type_name: "DatabasePool",
        };
        assert!(err.to_string().contains("DatabasePool"));
    }

    #[test]
    fn app_state_clone() {
        let db = DatabasePool {
            connection_string: "postgres://localhost".into(),
        };
        let state1 = AppState::new().with(db.clone());
        let state2 = state1.clone();

        assert_eq!(state2.get::<DatabasePool>(), Some(&db));
    }

    #[test]
    fn state_with_arc() {
        use std::sync::Arc;

        let ctx = test_context();
        let db = Arc::new(DatabasePool {
            connection_string: "postgres://localhost".into(),
        });
        let app_state = AppState::new().with(db.clone());

        let mut req = Request::new(Method::Get, "/test");
        req.insert_extension(app_state);

        let result =
            futures_executor::block_on(State::<Arc<DatabasePool>>::from_request(&ctx, &mut req));
        let State(extracted) = result.unwrap();
        assert_eq!(extracted.connection_string, "postgres://localhost");
    }

    // ========================================================================
    // bd-2u8l: Atomic State Mutation Tests
    // ========================================================================

    #[test]
    fn atomic_counter_fetch_add_concurrent() {
        // Test concurrent fetch_add operations don't lose increments (bd-2u8l)
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::thread;

        const NUM_THREADS: usize = 100;
        const INCREMENTS_PER_THREAD: usize = 1000;

        let counter = Arc::new(AtomicUsize::new(0));
        let app_state = AppState::new().with(counter.clone());

        let handles: Vec<_> = (0..NUM_THREADS)
            .map(|_| {
                let state = app_state.clone();
                thread::spawn(move || {
                    // Each thread gets the counter from state and increments it
                    let counter = state.get::<Arc<AtomicUsize>>().expect("Counter not found");
                    for _ in 0..INCREMENTS_PER_THREAD {
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        // Wait for all threads
        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // Verify no lost increments
        let final_value = counter.load(Ordering::SeqCst);
        let expected = NUM_THREADS * INCREMENTS_PER_THREAD;
        assert_eq!(
            final_value, expected,
            "Lost increments: expected {expected}, got {final_value}"
        );
    }

    #[test]
    fn atomic_compare_and_swap_concurrent() {
        // Test compare-and-swap (CAS) patterns under concurrency (bd-2u8l)
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::thread;

        const NUM_THREADS: usize = 50;
        const CAS_ATTEMPTS_PER_THREAD: usize = 100;

        // Counter that tracks successful CAS operations
        let counter = Arc::new(AtomicUsize::new(0));
        let success_count = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..NUM_THREADS)
            .map(|_| {
                let counter = counter.clone();
                let success_count = success_count.clone();
                thread::spawn(move || {
                    for _ in 0..CAS_ATTEMPTS_PER_THREAD {
                        // Try to increment using CAS
                        let mut current = counter.load(Ordering::SeqCst);
                        loop {
                            match counter.compare_exchange(
                                current,
                                current + 1,
                                Ordering::SeqCst,
                                Ordering::SeqCst,
                            ) {
                                Ok(_) => {
                                    success_count.fetch_add(1, Ordering::SeqCst);
                                    break;
                                }
                                Err(actual) => {
                                    // Retry with the actual value
                                    current = actual;
                                }
                            }
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        let final_counter = counter.load(Ordering::SeqCst);
        let total_successes = success_count.load(Ordering::SeqCst);
        let expected = NUM_THREADS * CAS_ATTEMPTS_PER_THREAD;

        // All CAS operations should succeed (with retries)
        assert_eq!(total_successes, expected);
        // Counter should equal total successful CAS operations
        assert_eq!(final_counter, expected);
    }

    #[test]
    fn atomic_state_concurrent_reads() {
        // Test concurrent reads don't corrupt state (bd-2u8l)
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::thread;

        const NUM_READERS: usize = 100;
        const READS_PER_THREAD: usize = 1000;
        const INITIAL_VALUE: usize = 42;

        let counter = Arc::new(AtomicUsize::new(INITIAL_VALUE));
        let app_state = AppState::new().with(counter.clone());

        let handles: Vec<_> = (0..NUM_READERS)
            .map(|_| {
                let state = app_state.clone();
                thread::spawn(move || {
                    let counter = state.get::<Arc<AtomicUsize>>().expect("Counter not found");
                    for _ in 0..READS_PER_THREAD {
                        let value = counter.load(Ordering::SeqCst);
                        assert_eq!(value, INITIAL_VALUE, "Value corrupted during read");
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        // Value should be unchanged
        assert_eq!(counter.load(Ordering::SeqCst), INITIAL_VALUE);
    }

    #[test]
    fn atomic_rate_limiter_pattern() {
        // Test a rate-limiter pattern using atomics (bd-2u8l)
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::thread;

        const MAX_REQUESTS: usize = 100;
        const NUM_CLIENTS: usize = 50;
        const REQUESTS_PER_CLIENT: usize = 5;

        // Simple rate limiter state
        #[derive(Clone)]
        struct RateLimiter {
            current_count: Arc<AtomicUsize>,
            max_count: usize,
        }

        impl RateLimiter {
            fn new(max: usize) -> Self {
                Self {
                    current_count: Arc::new(AtomicUsize::new(0)),
                    max_count: max,
                }
            }

            fn try_acquire(&self) -> bool {
                let mut current = self.current_count.load(Ordering::SeqCst);
                loop {
                    if current >= self.max_count {
                        return false;
                    }
                    match self.current_count.compare_exchange(
                        current,
                        current + 1,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => return true,
                        Err(actual) => current = actual,
                    }
                }
            }

            fn count(&self) -> usize {
                self.current_count.load(Ordering::SeqCst)
            }
        }

        let limiter = RateLimiter::new(MAX_REQUESTS);
        let app_state = AppState::new().with(limiter.clone());
        let allowed_count = Arc::new(AtomicUsize::new(0));
        let denied_count = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..NUM_CLIENTS)
            .map(|_| {
                let state = app_state.clone();
                let allowed = allowed_count.clone();
                let denied = denied_count.clone();
                thread::spawn(move || {
                    let limiter = state.get::<RateLimiter>().expect("Limiter not found");
                    for _ in 0..REQUESTS_PER_CLIENT {
                        if limiter.try_acquire() {
                            allowed.fetch_add(1, Ordering::SeqCst);
                        } else {
                            denied.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        let total_allowed = allowed_count.load(Ordering::SeqCst);
        let total_denied = denied_count.load(Ordering::SeqCst);
        let total_requests = NUM_CLIENTS * REQUESTS_PER_CLIENT;

        // Exactly MAX_REQUESTS should be allowed
        assert_eq!(total_allowed, MAX_REQUESTS, "Allowed count mismatch");
        // Rest should be denied
        assert_eq!(
            total_denied,
            total_requests - MAX_REQUESTS,
            "Denied count mismatch"
        );
        // Counter should never exceed max
        assert!(limiter.count() <= MAX_REQUESTS, "Rate limiter exceeded max");
    }

    #[test]
    fn atomic_concurrent_queue_pattern() {
        // Test a lock-free queue pattern using atomics (bd-2u8l)
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::thread;

        const NUM_PRODUCERS: usize = 10;
        const ITEMS_PER_PRODUCER: usize = 100;

        // Simple monotonic counter to simulate queue indices
        let head = Arc::new(AtomicUsize::new(0));
        let tail = Arc::new(AtomicUsize::new(0));
        let produced_ids = Arc::new(parking_lot::Mutex::new(Vec::new()));

        let handles: Vec<_> = (0..NUM_PRODUCERS)
            .map(|_| {
                let tail = tail.clone();
                let ids = produced_ids.clone();
                thread::spawn(move || {
                    for _ in 0..ITEMS_PER_PRODUCER {
                        // Atomically claim a slot in the "queue"
                        let slot = tail.fetch_add(1, Ordering::SeqCst);
                        ids.lock().push(slot);
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        let produced = produced_ids.lock();
        let expected_count = NUM_PRODUCERS * ITEMS_PER_PRODUCER;

        // Should have produced the expected number of items
        assert_eq!(produced.len(), expected_count);

        // Each ID should be unique (no lost slots)
        let mut sorted: Vec<_> = produced.iter().cloned().collect();
        sorted.sort();
        for (i, &id) in sorted.iter().enumerate() {
            assert_eq!(id, i, "Slot {i} missing or duplicated");
        }

        // Tail should match expected
        assert_eq!(tail.load(Ordering::SeqCst), expected_count);
    }

    // ========================================================================
    // bd-tnw0: Concurrent State Reads Under Load
    // ========================================================================

    #[test]
    fn concurrent_reads_basic_consistency() {
        // Test basic concurrent reads return consistent values (bd-tnw0)
        use std::sync::Arc;
        use std::thread;

        const NUM_READERS: usize = 100;
        const READS_PER_THREAD: usize = 100;

        // Create immutable state values
        let config_value = "production";
        let port_value = 8080u16;
        let enabled_value = true;

        let app_state = AppState::new()
            .with(config_value.to_string())
            .with(port_value)
            .with(enabled_value);

        let error_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let handles: Vec<_> = (0..NUM_READERS)
            .map(|_| {
                let state = app_state.clone();
                let errors = error_count.clone();
                thread::spawn(move || {
                    for _ in 0..READS_PER_THREAD {
                        // Read all state values (get returns Option<&T>)
                        let config = state.get::<String>();
                        let port = state.get::<u16>();
                        let enabled = state.get::<bool>();

                        // Verify consistency
                        if config.map(|s| s.as_str()) != Some(config_value) {
                            errors.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        if port != Some(&port_value) {
                            errors.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        if enabled != Some(&enabled_value) {
                            errors.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        assert_eq!(
            error_count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "Some reads returned inconsistent values"
        );
    }

    #[test]
    fn concurrent_reads_varying_payload_sizes() {
        // Test reads with different payload sizes (bd-tnw0)
        use std::sync::Arc;
        use std::thread;

        const NUM_READERS: usize = 50;
        const READS_PER_THREAD: usize = 50;

        // Small payload (just a number)
        let small: i32 = 42;
        // Medium payload (string)
        let medium: String = "a".repeat(1000);
        // Large payload (vector)
        let large: Vec<u8> = vec![0u8; 10_000];

        let app_state = AppState::new()
            .with(small)
            .with(medium.clone())
            .with(large.clone());

        let error_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let handles: Vec<_> = (0..NUM_READERS)
            .map(|_| {
                let state = app_state.clone();
                let expected_medium = medium.clone();
                let expected_large = large.clone();
                let errors = error_count.clone();
                thread::spawn(move || {
                    for _ in 0..READS_PER_THREAD {
                        // Read all payload sizes
                        if state.get::<i32>() != Some(&42) {
                            errors.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        if state.get::<String>().map(|s| s.len()) != Some(1000) {
                            errors.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        if state.get::<Vec<u8>>() != Some(&expected_large) {
                            errors.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        assert_eq!(
            error_count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "Payload size affected read consistency"
        );
    }

    #[test]
    fn concurrent_reads_nested_structures() {
        // Test reads with nested state structures (bd-tnw0)
        use std::sync::Arc;
        use std::thread;

        const NUM_READERS: usize = 50;

        #[derive(Clone, Debug, PartialEq)]
        struct OuterConfig {
            inner: InnerConfig,
            name: String,
        }

        #[derive(Clone, Debug, PartialEq)]
        struct InnerConfig {
            values: Vec<i32>,
            enabled: bool,
        }

        let nested = OuterConfig {
            inner: InnerConfig {
                values: vec![1, 2, 3, 4, 5],
                enabled: true,
            },
            name: "nested_test".to_string(),
        };

        let app_state = AppState::new().with(nested.clone());
        let error_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let handles: Vec<_> = (0..NUM_READERS)
            .map(|_| {
                let state = app_state.clone();
                let expected = nested.clone();
                let errors = error_count.clone();
                thread::spawn(move || {
                    for _ in 0..100 {
                        let read = state.get::<OuterConfig>();
                        if read != Some(&expected) {
                            errors.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                        // Also verify nested field access
                        if let Some(outer) = read {
                            if outer.inner.values.len() != 5 {
                                errors.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            }
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        assert_eq!(
            error_count.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "Nested structure reads inconsistent"
        );
    }

    #[test]
    fn concurrent_reads_with_arc_rwlock_pattern() {
        // Test Arc<RwLock<T>> patterns for mutable shared state (bd-tnw0)
        use parking_lot::RwLock;
        use std::sync::Arc;
        use std::thread;

        const NUM_READERS: usize = 80;
        const NUM_WRITERS: usize = 20;
        const OPS_PER_THREAD: usize = 100;

        #[derive(Clone)]
        struct MutableState {
            data: Arc<RwLock<Vec<i32>>>,
        }

        impl MutableState {
            fn new() -> Self {
                Self {
                    data: Arc::new(RwLock::new(Vec::new())),
                }
            }

            fn push(&self, value: i32) {
                self.data.write().push(value);
            }

            fn len(&self) -> usize {
                self.data.read().len()
            }
        }

        let mutable_state = MutableState::new();
        let app_state = AppState::new().with(mutable_state.clone());
        let read_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let write_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // Spawn reader threads
        let reader_handles: Vec<_> = (0..NUM_READERS)
            .map(|_| {
                let state = app_state.clone();
                let reads = read_count.clone();
                thread::spawn(move || {
                    for _ in 0..OPS_PER_THREAD {
                        let ms = state.get::<MutableState>().expect("State not found");
                        let _ = ms.len(); // Just read the length
                        reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                })
            })
            .collect();

        // Spawn writer threads
        let writer_handles: Vec<_> = (0..NUM_WRITERS)
            .map(|i| {
                let state = app_state.clone();
                let writes = write_count.clone();
                thread::spawn(move || {
                    for j in 0..OPS_PER_THREAD {
                        let ms = state.get::<MutableState>().expect("State not found");
                        ms.push((i * OPS_PER_THREAD + j) as i32);
                        writes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                })
            })
            .collect();

        // Wait for all threads
        for handle in reader_handles {
            handle.join().expect("Reader thread panicked");
        }
        for handle in writer_handles {
            handle.join().expect("Writer thread panicked");
        }

        // Verify all operations completed
        let total_reads = read_count.load(std::sync::atomic::Ordering::SeqCst);
        let total_writes = write_count.load(std::sync::atomic::Ordering::SeqCst);

        assert_eq!(total_reads, NUM_READERS * OPS_PER_THREAD);
        assert_eq!(total_writes, NUM_WRITERS * OPS_PER_THREAD);

        // Verify final state has all writes
        assert_eq!(mutable_state.len(), NUM_WRITERS * OPS_PER_THREAD);
    }
}

// ============================================================================
// Special Parameter Extractors (Request/Response Injection)
// ============================================================================

/// Read-only request data access.
///
/// Provides access to request metadata without consuming the body.
/// For body access, use the `Json`, `Form`, or other body extractors.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::RequestRef;
///
/// async fn handler(req: RequestRef) -> impl IntoResponse {
///     format!("Method: {}, Path: {}", req.method(), req.path())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RequestRef {
    method: crate::request::Method,
    path: String,
    query: Option<String>,
    headers: Vec<(String, Vec<u8>)>,
}

impl RequestRef {
    /// Get the HTTP method.
    #[must_use]
    pub fn method(&self) -> crate::request::Method {
        self.method
    }

    /// Get the request path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Get the query string.
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    /// Get a header value by name (case-insensitive).
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&[u8]> {
        let name_lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| n.to_ascii_lowercase() == name_lower)
            .map(|(_, v)| v.as_slice())
    }

    /// Iterate over all headers.
    pub fn headers(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.headers.iter().map(|(n, v)| (n.as_str(), v.as_slice()))
    }
}

impl FromRequest for RequestRef {
    type Error = std::convert::Infallible;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        Ok(RequestRef {
            method: req.method(),
            path: req.path().to_string(),
            query: req.query().map(String::from),
            headers: req
                .headers()
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_vec()))
                .collect(),
        })
    }
}

/// Mutable response container for setting response headers and cookies.
///
/// This extractor allows handlers to set additional response headers and cookies
/// that will be merged into the final response. The handler's return value
/// determines the status code and body; `ResponseMut` adds headers on top.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::ResponseMut;
/// use fastapi_core::response::Json;
///
/// async fn handler(mut resp: ResponseMut) -> Json<Data> {
///     resp.header("X-Custom-Header", "custom-value");
///     resp.cookie("session", "abc123");
///     Json(data)
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ResponseMutations {
    /// Headers to add to the response.
    pub headers: Vec<(String, Vec<u8>)>,
    /// Cookies to set (name, value, attributes).
    pub cookies: Vec<Cookie>,
    /// Cookies to delete.
    pub delete_cookies: Vec<String>,
}

impl ResponseMutations {
    /// Create empty response mutations.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a header.
    pub fn add_header(&mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.headers.push((name.into(), value.into()));
    }

    /// Set a cookie.
    pub fn add_cookie(&mut self, cookie: Cookie) {
        self.cookies.push(cookie);
    }

    /// Delete a cookie by name.
    pub fn remove_cookie(&mut self, name: impl Into<String>) {
        self.delete_cookies.push(name.into());
    }

    /// Apply mutations to a response.
    #[must_use]
    pub fn apply(self, mut response: crate::response::Response) -> crate::response::Response {
        // Add headers
        for (name, value) in self.headers {
            response = response.header(name, value);
        }

        // Add Set-Cookie headers for cookies
        for cookie in self.cookies {
            response = response.header("Set-Cookie", cookie.to_header_value().into_bytes());
        }

        // Add Set-Cookie headers to delete cookies
        for name in self.delete_cookies {
            // Sanitize the cookie name to prevent injection
            let sanitized_name = sanitize_cookie_token(&name);
            let delete_cookie = format!("{}=; Max-Age=0; Path=/", sanitized_name);
            response = response.header("Set-Cookie", delete_cookie.into_bytes());
        }

        response
    }
}

// ============================================================================
// Cookie Sanitization Helpers
// ============================================================================

/// Sanitize a cookie name to prevent injection attacks.
///
/// RFC 6265 specifies that cookie-name must be a valid HTTP token.
/// This removes control characters and separators that could be misinterpreted.
fn sanitize_cookie_token(name: &str) -> String {
    name.chars()
        .filter(|&c| {
            // Token characters per RFC 7230: any VCHAR except delimiters
            // Delimiters: "(),/:;<=>?@[\]{} and control chars
            c.is_ascii()
                && !c.is_ascii_control()
                && c != ' '
                && c != '"'
                && c != '('
                && c != ')'
                && c != ','
                && c != '/'
                && c != ':'
                && c != ';'
                && c != '<'
                && c != '='
                && c != '>'
                && c != '?'
                && c != '@'
                && c != '['
                && c != '\\'
                && c != ']'
                && c != '{'
                && c != '}'
        })
        .collect()
}

/// Sanitize a cookie value to prevent injection attacks.
///
/// RFC 6265 specifies that cookie-value excludes CTLs, whitespace,
/// DQUOTE, comma, semicolon, and backslash (unless using quoted form).
fn sanitize_cookie_value(value: &str) -> String {
    value
        .chars()
        .filter(|&c| {
            c.is_ascii()
                && !c.is_ascii_control()
                && c != ' '
                && c != '"'
                && c != ','
                && c != ';'
                && c != '\\'
        })
        .collect()
}

/// Sanitize a cookie attribute value (path, domain) to prevent injection.
///
/// Removes characters that could be interpreted as attribute delimiters.
fn sanitize_cookie_attr(attr: &str) -> String {
    attr.chars()
        .filter(|&c| c != ';' && c != '\r' && c != '\n' && c != '\0')
        .collect()
}

/// A cookie to set in the response.
#[derive(Debug, Clone)]
pub struct Cookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Max-Age in seconds (None = session cookie).
    pub max_age: Option<i64>,
    /// Path (defaults to /).
    pub path: Option<String>,
    /// Domain.
    pub domain: Option<String>,
    /// Secure flag.
    pub secure: bool,
    /// HttpOnly flag.
    pub http_only: bool,
    /// SameSite attribute.
    pub same_site: Option<SameSite>,
}

impl Cookie {
    /// Create a new cookie with name and value.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            max_age: None,
            path: None,
            domain: None,
            secure: false,
            http_only: false,
            same_site: None,
        }
    }

    /// Set the Max-Age attribute.
    #[must_use]
    pub fn max_age(mut self, seconds: i64) -> Self {
        self.max_age = Some(seconds);
        self
    }

    /// Set the Path attribute.
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Set the Domain attribute.
    #[must_use]
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Set the Secure flag.
    #[must_use]
    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    /// Set the HttpOnly flag.
    #[must_use]
    pub fn http_only(mut self, http_only: bool) -> Self {
        self.http_only = http_only;
        self
    }

    /// Set the SameSite attribute.
    #[must_use]
    pub fn same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = Some(same_site);
        self
    }

    /// Convert to Set-Cookie header value.
    ///
    /// # Security
    ///
    /// Cookie names, values, and attribute values are sanitized to prevent
    /// attribute injection attacks. Characters that could be interpreted as
    /// attribute delimiters (`;`, `\r`, `\n`, `\0`) are removed.
    #[must_use]
    pub fn to_header_value(&self) -> String {
        // Sanitize cookie name: remove any characters that aren't valid tokens
        // RFC 6265: cookie-name = token (excludes CTLs, separators)
        let sanitized_name = sanitize_cookie_token(&self.name);

        // Sanitize cookie value: remove characters that could inject attributes
        // RFC 6265: cookie-value excludes CTLs, whitespace, DQUOTE, comma, semicolon, backslash
        let sanitized_value = sanitize_cookie_value(&self.value);

        let mut parts = vec![format!("{}={}", sanitized_name, sanitized_value)];

        if let Some(max_age) = self.max_age {
            parts.push(format!("Max-Age={}", max_age));
        }
        if let Some(ref path) = self.path {
            // Sanitize path to prevent attribute injection
            let sanitized_path = sanitize_cookie_attr(path);
            parts.push(format!("Path={}", sanitized_path));
        }
        if let Some(ref domain) = self.domain {
            // Sanitize domain to prevent attribute injection
            let sanitized_domain = sanitize_cookie_attr(domain);
            parts.push(format!("Domain={}", sanitized_domain));
        }
        if self.secure {
            parts.push("Secure".to_string());
        }
        if self.http_only {
            parts.push("HttpOnly".to_string());
        }
        if let Some(ref same_site) = self.same_site {
            parts.push(format!("SameSite={}", same_site.as_str()));
        }

        parts.join("; ")
    }

    // =========================================================================
    // Secure Cookie Configuration Helpers
    // =========================================================================

    /// Create a session cookie with secure defaults.
    ///
    /// Session cookies are:
    /// - HttpOnly (not accessible to JavaScript)
    /// - Secure (HTTPS only, unless `production` is false)
    /// - SameSite=Lax (sent with top-level navigations)
    /// - Path=/ (accessible site-wide)
    ///
    /// # Arguments
    ///
    /// * `name` - Cookie name
    /// * `value` - Cookie value
    /// * `production` - If true, sets Secure flag; if false, omits it for local development
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::extract::Cookie;
    ///
    /// // Production session cookie
    /// let cookie = Cookie::session("session_id", "abc123", true);
    ///
    /// // Development session cookie (no Secure flag)
    /// let cookie = Cookie::session("session_id", "abc123", false);
    /// ```
    #[must_use]
    pub fn session(name: impl Into<String>, value: impl Into<String>, production: bool) -> Self {
        Self::new(name, value)
            .http_only(true)
            .secure(production)
            .same_site(SameSite::Lax)
            .path("/")
    }

    /// Create an authentication cookie with strict security.
    ///
    /// Auth cookies are:
    /// - HttpOnly (not accessible to JavaScript)
    /// - Secure (HTTPS only, unless `production` is false)
    /// - SameSite=Strict (only sent in first-party context)
    /// - Path=/ (accessible site-wide)
    ///
    /// Use this for authentication tokens that should never be sent in cross-site requests.
    ///
    /// # Arguments
    ///
    /// * `name` - Cookie name
    /// * `value` - Cookie value
    /// * `production` - If true, sets Secure flag; if false, omits it for local development
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::extract::Cookie;
    ///
    /// let cookie = Cookie::auth("auth_token", "jwt_here", true)
    ///     .max_age(86400); // 1 day
    /// ```
    #[must_use]
    pub fn auth(name: impl Into<String>, value: impl Into<String>, production: bool) -> Self {
        Self::new(name, value)
            .http_only(true)
            .secure(production)
            .same_site(SameSite::Strict)
            .path("/")
    }

    /// Create a CSRF token cookie.
    ///
    /// CSRF cookies are:
    /// - NOT HttpOnly (must be readable by JavaScript to include in requests)
    /// - Secure (HTTPS only, unless `production` is false)
    /// - SameSite=Strict (only sent in first-party context)
    /// - Path=/ (accessible site-wide)
    ///
    /// The CSRF token must be accessible to JavaScript so it can be included in
    /// request headers or form data for validation.
    ///
    /// # Arguments
    ///
    /// * `name` - Cookie name (commonly "csrf_token" or "_csrf")
    /// * `value` - The CSRF token value
    /// * `production` - If true, sets Secure flag; if false, omits it for local development
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::extract::Cookie;
    ///
    /// let cookie = Cookie::csrf("csrf_token", "random_token_here", true);
    /// ```
    #[must_use]
    pub fn csrf(name: impl Into<String>, value: impl Into<String>, production: bool) -> Self {
        Self::new(name, value)
            .http_only(false)
            .secure(production)
            .same_site(SameSite::Strict)
            .path("/")
    }

    /// Create a cookie with the `__Host-` prefix.
    ///
    /// The `__Host-` prefix enforces that the cookie:
    /// - MUST have the Secure flag
    /// - MUST NOT have a Domain attribute
    /// - MUST have Path=/
    ///
    /// This provides the strongest cookie security by preventing the cookie from
    /// being set by subdomains or accessed across different paths.
    ///
    /// # Arguments
    ///
    /// * `name` - Cookie name (without the `__Host-` prefix - it will be added)
    /// * `value` - Cookie value
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::extract::Cookie;
    ///
    /// // Creates cookie named "__Host-session"
    /// let cookie = Cookie::host_prefixed("session", "abc123")
    ///     .http_only(true)
    ///     .same_site(SameSite::Strict);
    /// ```
    #[must_use]
    pub fn host_prefixed(name: impl Into<String>, value: impl Into<String>) -> Self {
        let prefixed_name = format!("__Host-{}", name.into());
        Self::new(prefixed_name, value).secure(true).path("/")
    }

    /// Create a cookie with the `__Secure-` prefix.
    ///
    /// The `__Secure-` prefix enforces that the cookie:
    /// - MUST have the Secure flag
    ///
    /// Unlike `__Host-`, this allows Domain and Path attributes.
    ///
    /// # Arguments
    ///
    /// * `name` - Cookie name (without the `__Secure-` prefix - it will be added)
    /// * `value` - Cookie value
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::extract::Cookie;
    ///
    /// // Creates cookie named "__Secure-token"
    /// let cookie = Cookie::secure_prefixed("token", "abc123")
    ///     .domain(".example.com")
    ///     .http_only(true);
    /// ```
    #[must_use]
    pub fn secure_prefixed(name: impl Into<String>, value: impl Into<String>) -> Self {
        let prefixed_name = format!("__Secure-{}", name.into());
        Self::new(prefixed_name, value).secure(true)
    }

    /// Validate that the cookie meets its prefix requirements.
    ///
    /// Returns `Ok(())` if valid, or `Err` with a description of the violation.
    ///
    /// # Cookie Prefix Rules
    ///
    /// - `__Host-`: Must have Secure=true, Path="/", and no Domain
    /// - `__Secure-`: Must have Secure=true
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::extract::Cookie;
    ///
    /// let cookie = Cookie::host_prefixed("session", "abc123");
    /// assert!(cookie.validate_prefix().is_ok());
    ///
    /// // This would fail validation
    /// let invalid = Cookie::new("__Host-session", "abc123")
    ///     .domain("example.com"); // __Host- cannot have Domain
    /// assert!(invalid.validate_prefix().is_err());
    /// ```
    pub fn validate_prefix(&self) -> Result<(), CookiePrefixError> {
        if self.name.starts_with("__Host-") {
            if !self.secure {
                return Err(CookiePrefixError::HostRequiresSecure);
            }
            if self.domain.is_some() {
                return Err(CookiePrefixError::HostCannotHaveDomain);
            }
            if self.path.as_deref() != Some("/") {
                return Err(CookiePrefixError::HostRequiresRootPath);
            }
        } else if self.name.starts_with("__Secure-") && !self.secure {
            return Err(CookiePrefixError::SecureRequiresSecure);
        }
        Ok(())
    }

    /// Check if this cookie has a security prefix.
    #[must_use]
    pub fn has_security_prefix(&self) -> bool {
        self.name.starts_with("__Host-") || self.name.starts_with("__Secure-")
    }

    /// Get the security prefix type, if any.
    #[must_use]
    pub fn prefix(&self) -> Option<CookiePrefix> {
        if self.name.starts_with("__Host-") {
            Some(CookiePrefix::Host)
        } else if self.name.starts_with("__Secure-") {
            Some(CookiePrefix::Secure)
        } else {
            None
        }
    }
}

/// Cookie security prefix types.
///
/// Modern browsers support cookie prefixes that enforce security requirements:
/// - `__Host-`: Strongest protection, locks cookie to a single origin
/// - `__Secure-`: Requires HTTPS, but allows subdomain/path configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookiePrefix {
    /// The `__Host-` prefix.
    ///
    /// Requires: Secure=true, Path="/", no Domain attribute.
    Host,
    /// The `__Secure-` prefix.
    ///
    /// Requires: Secure=true only.
    Secure,
}

impl CookiePrefix {
    /// Get the string representation of the prefix.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Host => "__Host-",
            Self::Secure => "__Secure-",
        }
    }
}

/// Errors that can occur when validating cookie prefixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CookiePrefixError {
    /// `__Host-` prefix requires Secure flag.
    HostRequiresSecure,
    /// `__Host-` prefix cannot have a Domain attribute.
    HostCannotHaveDomain,
    /// `__Host-` prefix requires Path="/".
    HostRequiresRootPath,
    /// `__Secure-` prefix requires Secure flag.
    SecureRequiresSecure,
}

impl std::fmt::Display for CookiePrefixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostRequiresSecure => {
                write!(f, "__Host- prefix requires Secure flag to be true")
            }
            Self::HostCannotHaveDomain => {
                write!(f, "__Host- prefix cannot have a Domain attribute")
            }
            Self::HostRequiresRootPath => {
                write!(f, "__Host- prefix requires Path=\"/\"")
            }
            Self::SecureRequiresSecure => {
                write!(f, "__Secure- prefix requires Secure flag to be true")
            }
        }
    }
}

impl std::error::Error for CookiePrefixError {}

/// SameSite cookie attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    /// Strict: Cookie only sent in first-party context.
    Strict,
    /// Lax: Cookie sent with top-level navigations.
    Lax,
    /// None: Cookie sent in all contexts (requires Secure).
    None,
}

impl SameSite {
    /// Get the string representation.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Lax => "Lax",
            Self::None => "None",
        }
    }
}

// ============================================================================
// Cookie Request Extractors
// ============================================================================

/// Extract all cookies from the incoming request as a map.
///
/// Parses the `Cookie` header and provides access to all cookies by name.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::RequestCookies;
///
/// async fn handler(cookies: RequestCookies) -> impl IntoResponse {
///     if let Some(session_id) = cookies.get("session_id") {
///         format!("Session: {}", session_id)
///     } else {
///         "No session".to_string()
///     }
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct RequestCookies {
    cookies: std::collections::HashMap<String, String>,
}

impl RequestCookies {
    /// Create an empty cookie collection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse cookies from a Cookie header value.
    #[must_use]
    pub fn from_header(header_value: &str) -> Self {
        let mut cookies = std::collections::HashMap::new();

        // Cookie header format: "name1=value1; name2=value2"
        for pair in header_value.split(';') {
            let pair = pair.trim();
            if let Some((name, value)) = pair.split_once('=') {
                let name = name.trim().to_string();
                let value = value.trim().to_string();
                if !name.is_empty() {
                    cookies.insert(name, value);
                }
            }
        }

        Self { cookies }
    }

    /// Get a cookie value by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.cookies.get(name).map(String::as_str)
    }

    /// Check if a cookie exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.cookies.contains_key(name)
    }

    /// Get the number of cookies.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Check if there are no cookies.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Iterate over all cookie name-value pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.cookies.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Get all cookie names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.cookies.keys().map(String::as_str)
    }
}

impl FromRequest for RequestCookies {
    type Error = std::convert::Infallible;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let cookies = req
            .headers()
            .get("cookie")
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(Self::from_header)
            .unwrap_or_default();

        Ok(cookies)
    }
}

/// Extract a single cookie value by name from the incoming request.
///
/// The cookie name is specified via the `CookieName` trait, similar to how
/// `Header<T>` works with `HeaderName`.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::{RequestCookie, CookieName};
///
/// // Define a cookie name
/// struct SessionId;
/// impl CookieName for SessionId {
///     const NAME: &'static str = "session_id";
/// }
///
/// async fn handler(session: RequestCookie<SessionId>) -> impl IntoResponse {
///     format!("Session: {}", session.value())
/// }
///
/// // For optional cookies:
/// async fn optional_handler(session: Option<RequestCookie<SessionId>>) -> impl IntoResponse {
///     match session {
///         Some(s) => format!("Session: {}", s.value()),
///         None => "No session".to_string(),
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RequestCookie<T> {
    value: String,
    _marker: std::marker::PhantomData<T>,
}

impl<T> RequestCookie<T> {
    /// Create a new cookie extractor with the given value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            _marker: std::marker::PhantomData,
        }
    }

    /// Get the cookie value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Consume and return the cookie value.
    #[must_use]
    pub fn into_value(self) -> String {
        self.value
    }
}

impl<T> Deref for RequestCookie<T> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> AsRef<str> for RequestCookie<T> {
    fn as_ref(&self) -> &str {
        &self.value
    }
}

/// Trait for defining cookie names used with `RequestCookie<T>`.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::CookieName;
///
/// struct SessionId;
/// impl CookieName for SessionId {
///     const NAME: &'static str = "session_id";
/// }
/// ```
pub trait CookieName {
    /// The cookie name to extract.
    const NAME: &'static str;
}

/// Error type for cookie extraction failures.
#[derive(Debug)]
pub enum CookieExtractError {
    /// The requested cookie was not found.
    NotFound {
        /// The name of the missing cookie.
        name: &'static str,
    },
    /// The cookie value could not be parsed.
    InvalidValue {
        /// The cookie name.
        name: &'static str,
        /// The raw value that couldn't be parsed.
        value: String,
        /// Description of the expected format.
        expected: &'static str,
    },
}

impl fmt::Display for CookieExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { name } => {
                write!(f, "Cookie '{}' not found", name)
            }
            Self::InvalidValue {
                name,
                value,
                expected,
            } => {
                write!(
                    f,
                    "Invalid cookie '{}' value '{}': expected {}",
                    name, value, expected
                )
            }
        }
    }
}

impl std::error::Error for CookieExtractError {}

impl IntoResponse for CookieExtractError {
    fn into_response(self) -> crate::response::Response {
        match self {
            Self::NotFound { name } => ValidationErrors::single(
                ValidationError::missing(crate::error::loc::cookie(name))
                    .with_msg("Cookie is required"),
            )
            .into_response(),
            Self::InvalidValue {
                name,
                value,
                expected,
            } => ValidationErrors::single(
                ValidationError::type_error(crate::error::loc::cookie(name), expected)
                    .with_msg(format!("Expected {expected}"))
                    .with_input(serde_json::Value::String(value)),
            )
            .into_response(),
        }
    }
}

impl<T: CookieName> FromRequest for RequestCookie<T> {
    type Error = CookieExtractError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let cookies = req
            .headers()
            .get("cookie")
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(RequestCookies::from_header)
            .unwrap_or_default();

        cookies
            .get(T::NAME)
            .map(|v| RequestCookie::new(v))
            .ok_or(CookieExtractError::NotFound { name: T::NAME })
    }
}

// Common cookie name types for convenience

/// Session ID cookie name marker.
pub struct SessionIdCookie;
impl CookieName for SessionIdCookie {
    const NAME: &'static str = "session_id";
}

/// CSRF token cookie name marker.
pub struct CsrfTokenCookie;
impl CookieName for CsrfTokenCookie {
    const NAME: &'static str = "csrf_token";
}

// ============================================================================
// Response Mutation Extractor
// ============================================================================

/// Mutable response wrapper for setting headers and cookies.
///
/// This is the extractor type that handlers receive. Mutations made through
/// this wrapper are stored in request extensions and applied after the handler
/// returns.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::ResponseMut;
///
/// async fn handler(mut resp: ResponseMut) -> &'static str {
///     resp.header("X-Powered-By", "fastapi-rust");
///     resp.cookie("visited", "true");
///     "Hello"
/// }
/// ```
pub struct ResponseMut<'a> {
    mutations: &'a mut ResponseMutations,
}

impl<'a> ResponseMut<'a> {
    /// Set a response header.
    pub fn header(&mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.mutations.add_header(name, value);
    }

    /// Set a cookie.
    pub fn cookie(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.mutations.add_cookie(Cookie::new(name, value));
    }

    /// Set a cookie with full configuration.
    pub fn set_cookie(&mut self, cookie: Cookie) {
        self.mutations.add_cookie(cookie);
    }

    /// Delete a cookie by name.
    pub fn delete_cookie(&mut self, name: impl Into<String>) {
        self.mutations.remove_cookie(name);
    }
}

impl<'a> std::fmt::Debug for ResponseMut<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseMut")
            .field("mutations", &self.mutations)
            .finish()
    }
}

// Note: ResponseMut cannot implement FromRequest because it returns a borrowed
// reference. Instead, handlers should extract ResponseMutations and get a &mut
// reference to it. The App will apply mutations after handler execution.

impl FromRequest for ResponseMutations {
    type Error = std::convert::Infallible;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get existing mutations or create new ones
        if let Some(mutations) = req.get_extension::<ResponseMutations>() {
            Ok(mutations.clone())
        } else {
            let mutations = ResponseMutations::new();
            req.insert_extension(mutations.clone());
            Ok(mutations)
        }
    }
}

// ============================================================================
// Background Tasks Extractor
// ============================================================================

use std::sync::Arc;

/// Formats a panic payload into a human-readable message.
///
/// This helper extracts the panic message from the `Box<dyn Any>` payload
/// returned by `catch_unwind`.
fn format_panic_message(panic_info: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic_info.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic_info.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Type alias for a boxed async task function.
pub type BackgroundTask =
    Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

/// Internal storage for background tasks (thread-safe).
///
/// This uses `parking_lot::Mutex` for interior mutability while being Send + Sync.
#[derive(Default, Clone)]
pub struct BackgroundTasksInner {
    inner: Arc<parking_lot::Mutex<Vec<BackgroundTask>>>,
}

impl BackgroundTasksInner {
    /// Create a new empty task storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(parking_lot::Mutex::new(Vec::new())),
        }
    }

    /// Add a task to the queue.
    pub fn push(&self, task: BackgroundTask) {
        self.inner.lock().push(task);
    }

    /// Take all tasks from the queue.
    pub fn take(&self) -> Vec<BackgroundTask> {
        std::mem::take(&mut *self.inner.lock())
    }

    /// Returns the number of tasks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// Returns true if there are no tasks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
}

impl std::fmt::Debug for BackgroundTasksInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundTasksInner")
            .field("task_count", &self.len())
            .finish()
    }
}

/// Background task queue for running tasks after response is sent.
///
/// Tasks are executed in the order they are added, after the response
/// has been sent to the client. This is useful for:
/// - Sending emails
/// - Writing to external logs
/// - Triggering webhooks
/// - Updating caches
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::BackgroundTasks;
///
/// async fn handler(mut tasks: BackgroundTasks) -> &'static str {
///     tasks.add_task(|| async {
///         // Send notification email
///         send_email("user@example.com", "Welcome!").await;
///     });
///     "Response sent, email will be sent in background"
/// }
/// ```
///
/// # Note
///
/// Background tasks run after the response is sent but before the request
/// context is fully cleaned up. They share the same cancellation context
/// as the request, so long-running tasks should check for cancellation.
#[derive(Clone)]
pub struct BackgroundTasks {
    inner: BackgroundTasksInner,
}

impl Default for BackgroundTasks {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundTasks {
    /// Create a new empty task queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: BackgroundTasksInner::new(),
        }
    }

    /// Create from inner storage.
    #[must_use]
    pub(crate) fn from_inner(inner: BackgroundTasksInner) -> Self {
        Self { inner }
    }

    /// Add a background task.
    ///
    /// The task will be executed after the response is sent.
    pub fn add_task<F, Fut>(&mut self, task: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.inner.push(Box::new(move || Box::pin(task())));
    }

    /// Add a synchronous background task.
    ///
    /// The task will be executed after the response is sent.
    pub fn add_sync_task<F>(&mut self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.inner.push(Box::new(move || {
            Box::pin(async move {
                task();
            })
        }));
    }

    /// Take all tasks from the queue.
    pub fn take_tasks(&mut self) -> Vec<BackgroundTask> {
        self.inner.take()
    }

    /// Returns true if there are no tasks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of tasks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Execute all tasks sequentially.
    ///
    /// This is called by the framework after the response is sent.
    /// Tasks run in the order they were added (FIFO).
    ///
    /// # Error Handling
    ///
    /// This method does NOT provide error isolation. If a task panics,
    /// subsequent tasks will not run. For panic isolation, use
    /// [`execute_with_panic_isolation()`] instead.
    ///
    /// Since tasks run after the response is sent, panics do not affect
    /// the HTTP response that was already delivered to the client.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let response = app.handle(&ctx, &mut request).await;
    /// // Send response to client...
    /// if let Some(tasks) = App::take_background_tasks(&mut request) {
    ///     tasks.execute_all().await;
    /// }
    /// ```
    pub async fn execute_all(mut self) {
        for task in self.take_tasks() {
            let future = task();
            future.await;
        }
    }

    /// Execute all tasks with cancellation support via RequestContext.
    ///
    /// This version checks for cancellation between tasks and respects
    /// the request's cancellation state. Use this when you want background
    /// tasks to be cancelled along with the request.
    ///
    /// # Arguments
    ///
    /// * `ctx` - The request context for cancellation checking
    ///
    /// # Cancellation Behavior
    ///
    /// - Checks `ctx.is_cancelled()` before starting each task
    /// - If cancelled, remaining tasks are skipped
    /// - Already-running tasks complete before the check
    /// - Logs the number of skipped tasks
    ///
    /// # Integration with asupersync
    ///
    /// Background tasks run in the same region as the request. When the
    /// region is cancelled (client disconnect, timeout, etc.), subsequent
    /// tasks will be skipped. This ensures proper structured concurrency.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let response = app.handle(&ctx, &mut request).await;
    /// // Send response to client...
    /// if let Some(tasks) = App::take_background_tasks(&mut request) {
    ///     tasks.execute_with_context(&ctx).await;
    /// }
    /// ```
    pub async fn execute_with_context(mut self, ctx: &RequestContext) {
        let tasks = self.take_tasks();
        let task_count = tasks.len();
        let mut executed_count = 0;

        for (index, task) in tasks.into_iter().enumerate() {
            // Check for cancellation before starting each task
            if ctx.is_cancelled() {
                let remaining = task_count - index;
                if remaining > 0 {
                    ctx.trace(&format!(
                        "BackgroundTasks: Cancellation requested, skipping {} remaining task(s)",
                        remaining
                    ));
                }
                break;
            }

            // Execute the task
            let future = task();
            future.await;
            executed_count += 1;
        }

        // Trace completion for observability
        if task_count > 0 {
            ctx.trace(&format!(
                "BackgroundTasks: Executed {}/{} tasks",
                executed_count, task_count
            ));
        }
    }

    /// Execute tasks with per-task panic isolation using catch_unwind.
    ///
    /// This version catches panics that occur when **calling the task closure**
    /// (i.e., when creating the future), allowing subsequent tasks to continue.
    ///
    /// # What Is Caught vs. Not Caught
    ///
    /// ```ignore
    /// // CAUGHT: Panic in closure before returning future
    /// tasks.add_task(|| {
    ///     panic!("this is caught");
    ///     async {}  // never reached
    /// });
    ///
    /// // NOT CAUGHT: Panic inside async block (during .await)
    /// tasks.add_task(|| async {
    ///     panic!("this is NOT caught - propagates and stops remaining tasks");
    /// });
    /// ```
    ///
    /// Due to Rust's async limitation, panics inside the async task body
    /// cannot be caught without additional crate dependencies (like `futures`).
    ///
    /// For most use cases where task code is well-behaved, `execute_all()`
    /// or `execute_with_context()` is sufficient.
    pub async fn execute_with_panic_isolation(mut self) {
        let tasks = self.take_tasks();
        let task_count = tasks.len();
        let mut success_count = 0;
        let mut panic_count = 0;

        for (index, task) in tasks.into_iter().enumerate() {
            // Attempt to get the future - catch panics in the closure
            let future_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task()));

            match future_result {
                Ok(future) => {
                    // Got the future, now await it
                    // Note: Panics during await will still propagate
                    future.await;
                    success_count += 1;
                }
                Err(panic_info) => {
                    // Task closure panicked
                    panic_count += 1;
                    let panic_msg = format_panic_message(&panic_info);
                    eprintln!(
                        "[BackgroundTasks] Task {}/{} panicked: {}",
                        index + 1,
                        task_count,
                        panic_msg
                    );
                }
            }
        }

        if panic_count > 0 {
            eprintln!(
                "[BackgroundTasks] Completed with {}/{} successful, {} panicked",
                success_count, task_count, panic_count
            );
        }
    }

    /// Get the inner storage for request extensions.
    pub fn into_inner(self) -> BackgroundTasksInner {
        self.inner
    }
}

impl std::fmt::Debug for BackgroundTasks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundTasks")
            .field("task_count", &self.len())
            .finish()
    }
}

impl FromRequest for BackgroundTasks {
    type Error = std::convert::Infallible;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get existing task storage or create new one
        if let Some(inner) = req.get_extension::<BackgroundTasksInner>() {
            Ok(BackgroundTasks::from_inner(inner.clone()))
        } else {
            let inner = BackgroundTasksInner::new();
            req.insert_extension(inner.clone());
            Ok(BackgroundTasks::from_inner(inner))
        }
    }
}

#[cfg(test)]
mod special_extractor_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    #[test]
    fn request_ref_extracts_metadata() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/users/42");
        req.set_query(Some("page=1".to_string()));
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());

        let result = futures_executor::block_on(RequestRef::from_request(&ctx, &mut req));
        let req_ref = result.unwrap();

        assert_eq!(req_ref.method(), Method::Get);
        assert_eq!(req_ref.path(), "/users/42");
        assert_eq!(req_ref.query(), Some("page=1"));
        assert_eq!(
            req_ref.header("content-type"),
            Some(b"application/json".as_slice())
        );
    }

    #[test]
    fn request_ref_header_case_insensitive() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("X-Custom-Header", b"value".to_vec());

        let result = futures_executor::block_on(RequestRef::from_request(&ctx, &mut req));
        let req_ref = result.unwrap();

        assert_eq!(req_ref.header("x-custom-header"), Some(b"value".as_slice()));
        assert_eq!(req_ref.header("X-CUSTOM-HEADER"), Some(b"value".as_slice()));
    }

    #[test]
    fn cookie_to_header_value_simple() {
        let cookie = Cookie::new("session", "abc123");
        assert_eq!(cookie.to_header_value(), "session=abc123");
    }

    #[test]
    fn cookie_to_header_value_with_attributes() {
        let cookie = Cookie::new("session", "abc123")
            .max_age(3600)
            .path("/")
            .secure(true)
            .http_only(true)
            .same_site(SameSite::Strict);

        let header = cookie.to_header_value();
        assert!(header.contains("session=abc123"));
        assert!(header.contains("Max-Age=3600"));
        assert!(header.contains("Path=/"));
        assert!(header.contains("Secure"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Strict"));
    }

    #[test]
    fn response_mutations_apply_headers() {
        let mut mutations = ResponseMutations::new();
        mutations.add_header("X-Custom", "value");
        mutations.add_header("X-Another", "other");

        let response = crate::response::Response::ok();
        let response = mutations.apply(response);

        let headers = response.headers();
        assert!(
            headers
                .iter()
                .any(|(n, v)| n == "X-Custom" && v == b"value")
        );
        assert!(
            headers
                .iter()
                .any(|(n, v)| n == "X-Another" && v == b"other")
        );
    }

    #[test]
    fn response_mutations_apply_cookies() {
        let mut mutations = ResponseMutations::new();
        mutations.add_cookie(Cookie::new("session", "abc").http_only(true));

        let response = crate::response::Response::ok();
        let response = mutations.apply(response);

        let headers = response.headers();
        let set_cookie = headers
            .iter()
            .find(|(n, _)| n == "Set-Cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert!(set_cookie.is_some());
        assert!(set_cookie.unwrap().contains("session=abc"));
    }

    #[test]
    fn response_mutations_delete_cookie() {
        let mut mutations = ResponseMutations::new();
        mutations.remove_cookie("session");

        let response = crate::response::Response::ok();
        let response = mutations.apply(response);

        let headers = response.headers();
        let set_cookie = headers
            .iter()
            .find(|(n, _)| n == "Set-Cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert!(set_cookie.is_some());
        let cookie_header = set_cookie.unwrap();
        assert!(cookie_header.contains("session="));
        assert!(cookie_header.contains("Max-Age=0"));
    }

    #[test]
    fn response_mutations_extract() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        let result = futures_executor::block_on(ResponseMutations::from_request(&ctx, &mut req));
        let mutations = result.unwrap();
        assert!(mutations.headers.is_empty());
        assert!(mutations.cookies.is_empty());
    }

    // =========================================================================
    // Cookie Sanitization Security Tests
    // =========================================================================

    #[test]
    fn cookie_sanitizes_semicolon_injection_in_value() {
        // Attacker tries to inject Domain attribute via value
        let cookie = Cookie::new("session", "abc; Domain=.evil.com");
        let header = cookie.to_header_value();
        // Semicolon should be removed, preventing attribute injection
        assert_eq!(header, "session=abcDomain=.evil.com");
        assert!(!header.contains("; Domain"));
    }

    #[test]
    fn cookie_sanitizes_semicolon_injection_in_name() {
        // Attacker tries to inject via cookie name
        let cookie = Cookie::new("session; HttpOnly", "value");
        let header = cookie.to_header_value();
        // Semicolon should be removed from name
        assert!(!header.starts_with("session; "));
        assert!(header.starts_with("sessionHttpOnly="));
    }

    #[test]
    fn cookie_sanitizes_path_injection() {
        // Attacker tries to inject attributes via path
        let cookie = Cookie::new("session", "abc").path("/; HttpOnly; Domain=.evil.com");
        let header = cookie.to_header_value();
        // Semicolons should be removed from path, preventing attribute injection
        // The path becomes "/HttpOnlyDomain=.evil.com" (gibberish but safe)
        assert!(!header.contains("; Domain"));
        assert!(!header.contains("; HttpOnly"));
        // Verify path is present but sanitized (no semicolons)
        assert!(header.contains("Path=/"));
    }

    #[test]
    fn cookie_sanitizes_domain_injection() {
        // Attacker tries to inject attributes via domain
        let cookie = Cookie::new("session", "abc").domain(".example.com; HttpOnly=false");
        let header = cookie.to_header_value();
        // Semicolons should be removed from domain, preventing attribute injection
        assert!(!header.contains("; HttpOnly=false"));
        // Domain is sanitized (semicolon removed), but space is preserved in attr values
        assert!(header.contains("Domain=.example.com HttpOnly=false"));
    }

    #[test]
    fn cookie_sanitizes_control_characters() {
        // Attacker tries CRLF injection
        let cookie = Cookie::new("session", "abc\r\nSet-Cookie: evil=value");
        let header = cookie.to_header_value();
        // Control characters and spaces should be removed
        assert!(!header.contains("\r"));
        assert!(!header.contains("\n"));
        assert!(!header.contains(" ")); // Space is also removed from cookie values
        // The sanitized value is "abcSet-Cookie:evil=value" (no CRLF injection possible)
        assert!(header.contains("session=abcSet-Cookie:evil=value"));
    }

    #[test]
    fn delete_cookie_sanitizes_name() {
        // Attacker tries to inject via delete cookie name
        let mut mutations = ResponseMutations::new();
        mutations.remove_cookie("session; Domain=.evil.com");

        let response = crate::response::Response::ok();
        let response = mutations.apply(response);

        let headers = response.headers();
        let set_cookie = headers
            .iter()
            .find(|(n, _)| n == "Set-Cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert!(set_cookie.is_some());
        let cookie_header = set_cookie.unwrap();
        // Semicolon should be removed, no Domain attribute injected
        assert!(!cookie_header.contains("; Domain"));
    }

    // =========================================================================
    // Secure Cookie Configuration Helper Tests
    // =========================================================================

    #[test]
    fn session_cookie_production() {
        let cookie = Cookie::session("session_id", "abc123", true);
        assert_eq!(cookie.name, "session_id");
        assert_eq!(cookie.value, "abc123");
        assert!(cookie.http_only);
        assert!(cookie.secure);
        assert_eq!(cookie.same_site, Some(SameSite::Lax));
        assert_eq!(cookie.path, Some("/".to_string()));
    }

    #[test]
    fn session_cookie_development() {
        let cookie = Cookie::session("session_id", "abc123", false);
        assert!(cookie.http_only);
        assert!(!cookie.secure); // No Secure flag in development
        assert_eq!(cookie.same_site, Some(SameSite::Lax));
    }

    #[test]
    fn auth_cookie_production() {
        let cookie = Cookie::auth("auth_token", "jwt_token", true);
        assert_eq!(cookie.name, "auth_token");
        assert!(cookie.http_only);
        assert!(cookie.secure);
        assert_eq!(cookie.same_site, Some(SameSite::Strict)); // Stricter than session
        assert_eq!(cookie.path, Some("/".to_string()));
    }

    #[test]
    fn csrf_cookie_is_readable_by_js() {
        let cookie = Cookie::csrf("csrf_token", "random_value", true);
        assert_eq!(cookie.name, "csrf_token");
        assert!(!cookie.http_only); // Must be readable by JS
        assert!(cookie.secure);
        assert_eq!(cookie.same_site, Some(SameSite::Strict));
    }

    #[test]
    fn host_prefixed_cookie() {
        let cookie = Cookie::host_prefixed("session", "abc123");
        assert_eq!(cookie.name, "__Host-session");
        assert!(cookie.secure);
        assert_eq!(cookie.path, Some("/".to_string()));
        assert!(cookie.domain.is_none());
        assert!(cookie.validate_prefix().is_ok());
    }

    #[test]
    fn host_prefixed_cookie_validation_fails_without_secure() {
        let cookie = Cookie::new("__Host-session", "abc123")
            .path("/")
            .secure(false);
        assert_eq!(
            cookie.validate_prefix(),
            Err(CookiePrefixError::HostRequiresSecure)
        );
    }

    #[test]
    fn host_prefixed_cookie_validation_fails_with_domain() {
        let cookie = Cookie::new("__Host-session", "abc123")
            .path("/")
            .secure(true)
            .domain("example.com");
        assert_eq!(
            cookie.validate_prefix(),
            Err(CookiePrefixError::HostCannotHaveDomain)
        );
    }

    #[test]
    fn host_prefixed_cookie_validation_fails_without_root_path() {
        let cookie = Cookie::new("__Host-session", "abc123")
            .path("/api")
            .secure(true);
        assert_eq!(
            cookie.validate_prefix(),
            Err(CookiePrefixError::HostRequiresRootPath)
        );
    }

    #[test]
    fn secure_prefixed_cookie() {
        let cookie = Cookie::secure_prefixed("token", "abc123");
        assert_eq!(cookie.name, "__Secure-token");
        assert!(cookie.secure);
        // __Secure- allows Domain and Path
        let cookie = cookie.domain("example.com").path("/api");
        assert!(cookie.validate_prefix().is_ok());
    }

    #[test]
    fn secure_prefixed_cookie_validation_fails_without_secure() {
        let cookie = Cookie::new("__Secure-token", "abc123").secure(false);
        assert_eq!(
            cookie.validate_prefix(),
            Err(CookiePrefixError::SecureRequiresSecure)
        );
    }

    #[test]
    fn cookie_prefix_detection() {
        let host_cookie = Cookie::host_prefixed("session", "abc");
        assert!(host_cookie.has_security_prefix());
        assert_eq!(host_cookie.prefix(), Some(CookiePrefix::Host));

        let secure_cookie = Cookie::secure_prefixed("token", "abc");
        assert!(secure_cookie.has_security_prefix());
        assert_eq!(secure_cookie.prefix(), Some(CookiePrefix::Secure));

        let normal_cookie = Cookie::new("regular", "abc");
        assert!(!normal_cookie.has_security_prefix());
        assert_eq!(normal_cookie.prefix(), None);
    }

    #[test]
    fn cookie_prefix_as_str() {
        assert_eq!(CookiePrefix::Host.as_str(), "__Host-");
        assert_eq!(CookiePrefix::Secure.as_str(), "__Secure-");
    }

    #[test]
    fn cookie_prefix_error_display() {
        assert_eq!(
            CookiePrefixError::HostRequiresSecure.to_string(),
            "__Host- prefix requires Secure flag to be true"
        );
        assert_eq!(
            CookiePrefixError::HostCannotHaveDomain.to_string(),
            "__Host- prefix cannot have a Domain attribute"
        );
        assert_eq!(
            CookiePrefixError::HostRequiresRootPath.to_string(),
            "__Host- prefix requires Path=\"/\""
        );
        assert_eq!(
            CookiePrefixError::SecureRequiresSecure.to_string(),
            "__Secure- prefix requires Secure flag to be true"
        );
    }

    #[test]
    fn session_cookie_header_format() {
        let cookie = Cookie::session("sid", "abc", true);
        let header = cookie.to_header_value();
        assert!(header.contains("sid=abc"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("Secure"));
        assert!(header.contains("SameSite=Lax"));
        assert!(header.contains("Path=/"));
    }

    #[test]
    fn host_prefixed_cookie_header_format() {
        let cookie = Cookie::host_prefixed("session", "abc")
            .http_only(true)
            .same_site(SameSite::Strict);
        let header = cookie.to_header_value();
        assert!(header.contains("__Host-session=abc"));
        assert!(header.contains("Secure"));
        assert!(header.contains("Path=/"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Strict"));
    }

    // =========================================================================
    // Request Cookie Extractor Tests
    // =========================================================================

    #[test]
    fn request_cookies_parses_single_cookie() {
        let cookies = RequestCookies::from_header("session_id=abc123");
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies.get("session_id"), Some("abc123"));
    }

    #[test]
    fn request_cookies_parses_multiple_cookies() {
        let cookies = RequestCookies::from_header("session_id=abc123; user=bob; theme=dark");
        assert_eq!(cookies.len(), 3);
        assert_eq!(cookies.get("session_id"), Some("abc123"));
        assert_eq!(cookies.get("user"), Some("bob"));
        assert_eq!(cookies.get("theme"), Some("dark"));
    }

    #[test]
    fn request_cookies_handles_whitespace() {
        let cookies = RequestCookies::from_header("  session_id = abc123 ;  user=bob  ");
        assert_eq!(cookies.get("session_id"), Some("abc123"));
        assert_eq!(cookies.get("user"), Some("bob"));
    }

    #[test]
    fn request_cookies_handles_empty_header() {
        let cookies = RequestCookies::from_header("");
        assert!(cookies.is_empty());
    }

    #[test]
    fn request_cookies_handles_malformed_pairs() {
        // Malformed pairs without = should be skipped
        let cookies = RequestCookies::from_header("valid=value; malformed; another=good");
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies.get("valid"), Some("value"));
        assert_eq!(cookies.get("another"), Some("good"));
        assert!(!cookies.contains("malformed"));
    }

    #[test]
    fn request_cookies_contains_check() {
        let cookies = RequestCookies::from_header("session=abc");
        assert!(cookies.contains("session"));
        assert!(!cookies.contains("missing"));
    }

    #[test]
    fn request_cookies_iter() {
        let cookies = RequestCookies::from_header("a=1; b=2");
        let pairs: Vec<_> = cookies.iter().collect();
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&("a", "1")));
        assert!(pairs.contains(&("b", "2")));
    }

    #[test]
    fn request_cookies_from_request() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("cookie", b"session=xyz; user=alice".to_vec());

        let result = futures_executor::block_on(RequestCookies::from_request(&ctx, &mut req));
        let cookies = result.unwrap();
        assert_eq!(cookies.get("session"), Some("xyz"));
        assert_eq!(cookies.get("user"), Some("alice"));
    }

    #[test]
    fn request_cookies_from_request_no_cookie_header() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        let result = futures_executor::block_on(RequestCookies::from_request(&ctx, &mut req));
        let cookies = result.unwrap();
        assert!(cookies.is_empty());
    }

    #[test]
    fn request_cookie_extractor_found() {
        #[derive(Debug)]
        struct TestCookie;
        impl CookieName for TestCookie {
            const NAME: &'static str = "test_cookie";
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("cookie", b"test_cookie=hello_world".to_vec());

        let result =
            futures_executor::block_on(RequestCookie::<TestCookie>::from_request(&ctx, &mut req));
        let cookie = result.unwrap();
        assert_eq!(cookie.value(), "hello_world");
    }

    #[test]
    fn request_cookie_extractor_not_found() {
        #[derive(Debug)]
        struct MissingCookie;
        impl CookieName for MissingCookie {
            const NAME: &'static str = "missing";
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut().insert("cookie", b"other=value".to_vec());

        let result = futures_executor::block_on(RequestCookie::<MissingCookie>::from_request(
            &ctx, &mut req,
        ));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            CookieExtractError::NotFound { name: "missing" }
        ));
    }

    #[test]
    fn request_cookie_deref() {
        #[derive(Debug)]
        struct TestCookie;
        impl CookieName for TestCookie {
            const NAME: &'static str = "test";
        }

        let cookie = RequestCookie::<TestCookie>::new("test_value");
        // Test Deref to str
        assert_eq!(&*cookie, "test_value");
        // Test AsRef
        assert_eq!(cookie.as_ref(), "test_value");
    }

    #[test]
    fn session_id_cookie_marker() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("cookie", b"session_id=sess123".to_vec());

        let result = futures_executor::block_on(RequestCookie::<SessionIdCookie>::from_request(
            &ctx, &mut req,
        ));
        let cookie = result.unwrap();
        assert_eq!(cookie.value(), "sess123");
    }

    #[test]
    fn csrf_token_cookie_marker() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        req.headers_mut()
            .insert("cookie", b"csrf_token=csrf_abc".to_vec());

        let result = futures_executor::block_on(RequestCookie::<CsrfTokenCookie>::from_request(
            &ctx, &mut req,
        ));
        let cookie = result.unwrap();
        assert_eq!(cookie.value(), "csrf_abc");
    }
}

#[cfg(test)]
mod background_tasks_tests {
    use super::*;
    use crate::request::Method;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    #[test]
    fn background_tasks_inner_new_is_empty() {
        let inner = BackgroundTasksInner::new();
        assert!(inner.take().is_empty());
    }

    #[test]
    fn background_tasks_inner_push_and_take() {
        let inner = BackgroundTasksInner::new();

        let executed = Arc::new(AtomicBool::new(false));
        let executed_clone = executed.clone();

        // BackgroundTask is FnOnce() -> Pin<Box<Future>>
        inner.push(Box::new(move || {
            Box::pin(async move {
                executed_clone.store(true, Ordering::SeqCst);
            })
        }));

        let tasks = inner.take();
        assert_eq!(tasks.len(), 1);

        // Execute the task: call to get future, then block_on
        let task_fn = tasks.into_iter().next().unwrap();
        let future = task_fn();
        futures_executor::block_on(future);
        assert!(executed.load(Ordering::SeqCst));
    }

    #[test]
    fn background_tasks_inner_take_empties_queue() {
        let inner = BackgroundTasksInner::new();

        inner.push(Box::new(|| Box::pin(async {})));
        inner.push(Box::new(|| Box::pin(async {})));

        let tasks = inner.take();
        assert_eq!(tasks.len(), 2);

        // Taking again should be empty
        let tasks = inner.take();
        assert!(tasks.is_empty());
    }

    #[test]
    fn background_tasks_add_async_task() {
        let mut tasks = BackgroundTasks::new();

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        tasks.add_task(move || async move {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        });

        let queued = tasks.take_tasks();
        assert_eq!(queued.len(), 1);

        // Execute the task
        let task_fn = queued.into_iter().next().unwrap();
        futures_executor::block_on(task_fn());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn background_tasks_add_sync_task() {
        let mut tasks = BackgroundTasks::new();

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        tasks.add_sync_task(move || {
            counter_clone.fetch_add(10, Ordering::SeqCst);
        });

        let queued = tasks.take_tasks();
        assert_eq!(queued.len(), 1);

        let task_fn = queued.into_iter().next().unwrap();
        futures_executor::block_on(task_fn());
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    #[test]
    fn background_tasks_multiple_tasks_execute_in_order() {
        let mut tasks = BackgroundTasks::new();

        let order = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let order1 = order.clone();
        let order2 = order.clone();
        let order3 = order.clone();

        tasks.add_task(move || async move {
            order1.lock().push(1);
        });
        tasks.add_task(move || async move {
            order2.lock().push(2);
        });
        tasks.add_task(move || async move {
            order3.lock().push(3);
        });

        let queued = tasks.take_tasks();
        assert_eq!(queued.len(), 3);

        // Execute tasks in order
        for task_fn in queued {
            futures_executor::block_on(task_fn());
        }

        assert_eq!(*order.lock(), vec![1, 2, 3]);
    }

    #[test]
    fn background_tasks_from_request_creates_new() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        let tasks = futures_executor::block_on(BackgroundTasks::from_request(&ctx, &mut req))
            .expect("extraction should succeed");

        // Should be empty initially
        let inner_tasks = tasks.into_inner().take();
        assert!(inner_tasks.is_empty());
    }

    #[test]
    fn background_tasks_from_request_shares_inner() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        // First extraction creates inner
        let mut tasks1 = futures_executor::block_on(BackgroundTasks::from_request(&ctx, &mut req))
            .expect("extraction should succeed");

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        tasks1.add_task(move || async move {
            counter_clone.store(42, Ordering::SeqCst);
        });

        // Second extraction should get the same inner
        let tasks2 = futures_executor::block_on(BackgroundTasks::from_request(&ctx, &mut req))
            .expect("extraction should succeed");

        // Inner should have the task added by tasks1
        let queued = tasks2.into_inner().take();
        assert_eq!(queued.len(), 1);

        // Execute and verify
        let task_fn = queued.into_iter().next().unwrap();
        futures_executor::block_on(task_fn());
        assert_eq!(counter.load(Ordering::SeqCst), 42);
    }

    #[test]
    fn background_tasks_debug_shows_task_count() {
        let mut tasks = BackgroundTasks::new();

        let debug_empty = format!("{:?}", tasks);
        assert!(debug_empty.contains("task_count"));
        assert!(debug_empty.contains("BackgroundTasks"));

        tasks.add_task(|| async {});
        tasks.add_task(|| async {});

        let debug_with_tasks = format!("{:?}", tasks);
        assert!(debug_with_tasks.contains("task_count"));
    }

    #[test]
    fn background_tasks_inner_thread_safe() {
        let inner = BackgroundTasksInner::new();
        let inner_clone = inner.clone();

        let counter = Arc::new(AtomicU32::new(0));
        let counter1 = counter.clone();
        let counter2 = counter.clone();

        // Push from two "threads" (simulated via sequential calls)
        inner.push(Box::new(move || {
            Box::pin(async move {
                counter1.fetch_add(1, Ordering::SeqCst);
            })
        }));
        inner_clone.push(Box::new(move || {
            Box::pin(async move {
                counter2.fetch_add(10, Ordering::SeqCst);
            })
        }));

        // Both tasks should be in the queue
        let tasks = inner.take();
        assert_eq!(tasks.len(), 2);

        for task_fn in tasks {
            futures_executor::block_on(task_fn());
        }

        assert_eq!(counter.load(Ordering::SeqCst), 11);
    }

    #[test]
    fn background_tasks_into_inner_conversion() {
        let mut tasks = BackgroundTasks::new();

        tasks.add_task(|| async {});
        tasks.add_task(|| async {});

        let inner = tasks.into_inner();
        let queued = inner.take();
        assert_eq!(queued.len(), 2);
    }

    #[test]
    fn background_tasks_is_empty_and_len() {
        let mut tasks = BackgroundTasks::new();

        assert!(tasks.is_empty());
        assert_eq!(tasks.len(), 0);

        tasks.add_task(|| async {});
        assert!(!tasks.is_empty());
        assert_eq!(tasks.len(), 1);

        tasks.add_task(|| async {});
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn background_tasks_inner_len_and_is_empty() {
        let inner = BackgroundTasksInner::new();

        assert!(inner.is_empty());
        assert_eq!(inner.len(), 0);

        inner.push(Box::new(|| Box::pin(async {})));
        assert!(!inner.is_empty());
        assert_eq!(inner.len(), 1);

        inner.push(Box::new(|| Box::pin(async {})));
        assert_eq!(inner.len(), 2);

        // Take empties it
        let _ = inner.take();
        assert!(inner.is_empty());
        assert_eq!(inner.len(), 0);
    }

    #[test]
    fn background_tasks_execute_all_runs_all_tasks() {
        let mut tasks = BackgroundTasks::new();

        let counter = Arc::new(AtomicU32::new(0));
        let c1 = counter.clone();
        let c2 = counter.clone();
        let c3 = counter.clone();

        tasks.add_task(move || async move {
            c1.fetch_add(1, Ordering::SeqCst);
        });
        tasks.add_task(move || async move {
            c2.fetch_add(10, Ordering::SeqCst);
        });
        tasks.add_task(move || async move {
            c3.fetch_add(100, Ordering::SeqCst);
        });

        futures_executor::block_on(tasks.execute_all());
        assert_eq!(counter.load(Ordering::SeqCst), 111);
    }

    #[test]
    fn background_tasks_execute_with_context_respects_cancellation() {
        let ctx = test_context();
        let mut tasks = BackgroundTasks::new();

        let counter = Arc::new(AtomicU32::new(0));
        let c1 = counter.clone();
        let c2 = counter.clone();

        tasks.add_task(move || async move {
            c1.fetch_add(1, Ordering::SeqCst);
        });
        tasks.add_task(move || async move {
            c2.fetch_add(10, Ordering::SeqCst);
        });

        // Execute with non-cancelled context - all tasks should run
        futures_executor::block_on(tasks.execute_with_context(&ctx));
        assert_eq!(counter.load(Ordering::SeqCst), 11);
    }

    #[test]
    fn background_tasks_execute_with_panic_isolation_handles_closure_panic() {
        let mut tasks = BackgroundTasks::new();

        let counter = Arc::new(AtomicU32::new(0));
        let c1 = counter.clone();
        let c2 = counter.clone();

        // First task succeeds
        tasks.add_task(move || async move {
            c1.fetch_add(1, Ordering::SeqCst);
        });

        // Second task panics in closure (not in async block)
        tasks.inner.push(Box::new(|| {
            panic!("intentional test panic");
        }));

        // Third task should still run after second panics
        tasks.add_task(move || async move {
            c2.fetch_add(100, Ordering::SeqCst);
        });

        // Execute with panic isolation
        futures_executor::block_on(tasks.execute_with_panic_isolation());

        // First task ran, second panicked, third ran
        // Note: Due to the way the test is structured, the third task should run
        assert_eq!(counter.load(Ordering::SeqCst), 101);
    }

    #[test]
    fn format_panic_message_extracts_str() {
        let panic_info: Box<dyn std::any::Any + Send> = Box::new("test panic message");
        let msg = super::format_panic_message(&panic_info);
        assert_eq!(msg, "test panic message");
    }

    #[test]
    fn format_panic_message_extracts_string() {
        let panic_info: Box<dyn std::any::Any + Send> = Box::new(String::from("string panic"));
        let msg = super::format_panic_message(&panic_info);
        assert_eq!(msg, "string panic");
    }

    #[test]
    fn format_panic_message_handles_unknown() {
        let panic_info: Box<dyn std::any::Any + Send> = Box::new(42i32);
        let msg = super::format_panic_message(&panic_info);
        assert_eq!(msg, "unknown panic");
    }

    // =========================================================================
    // Execution Timing Tests (bd-1ktc)
    // These tests verify background task execution timing behavior as it would
    // be used by the HTTP server: response is built first, then tasks execute.
    // =========================================================================

    #[test]
    fn background_tasks_timing_single_task_after_response() {
        // Simulates: handler queues task, response returned, task executes after
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        // Phase 1: Handler adds task to the request (simulating FromRequest extraction)
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        let mut tasks = futures_executor::block_on(BackgroundTasks::from_request(&ctx, &mut req))
            .expect("extraction should succeed");

        tasks.add_task(move || async move {
            counter_clone.store(42, Ordering::SeqCst);
        });

        // Phase 2: Response is "returned" - counter should still be 0
        // (In real usage, response would be sent to client here)
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "task should not run before take_tasks"
        );

        // Phase 3: Server takes tasks after response is "sent"
        // Get the inner from the request extension (simulating App::take_background_tasks)
        let taken_tasks = req
            .get_extension::<BackgroundTasksInner>()
            .map(|inner| BackgroundTasks::from_inner(inner.clone()))
            .expect("tasks should be in extension");

        // Counter still 0 - tasks haven't executed yet
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "task should not run before execute_all"
        );

        // Phase 4: Execute tasks (server does this after sending response)
        futures_executor::block_on(taken_tasks.execute_all());

        // Now the counter should be set
        assert_eq!(
            counter.load(Ordering::SeqCst),
            42,
            "task should have executed"
        );
    }

    #[test]
    fn background_tasks_timing_multiple_tasks_in_order() {
        // Verify that multiple tasks execute in the order they were queued
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        let execution_order = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let order1 = execution_order.clone();
        let order2 = execution_order.clone();
        let order3 = execution_order.clone();

        let mut tasks = futures_executor::block_on(BackgroundTasks::from_request(&ctx, &mut req))
            .expect("extraction should succeed");

        // Queue tasks in order: 1, 2, 3
        tasks.add_task(move || async move {
            order1.lock().push(1);
        });
        tasks.add_task(move || async move {
            order2.lock().push(2);
        });
        tasks.add_task(move || async move {
            order3.lock().push(3);
        });

        // Simulate response being built/sent
        assert!(
            execution_order.lock().is_empty(),
            "no tasks should run during response building"
        );

        // Take and execute tasks
        let taken_tasks = req
            .get_extension::<BackgroundTasksInner>()
            .map(|inner| BackgroundTasks::from_inner(inner.clone()))
            .expect("tasks should be in extension");

        futures_executor::block_on(taken_tasks.execute_all());

        // Verify order: 1, 2, 3
        assert_eq!(
            *execution_order.lock(),
            vec![1, 2, 3],
            "tasks should execute in queue order"
        );
    }

    #[test]
    fn background_tasks_timing_tasks_can_spawn_more_tasks() {
        // Test that a task can add more tasks to its own BackgroundTasks
        // Note: In real usage, the inner is shared, so adding during execution
        // would affect the same queue (but those tasks wouldn't run until next iteration)

        let mut tasks = BackgroundTasks::new();

        let counter = Arc::new(AtomicU32::new(0));
        let c1 = counter.clone();
        let c2 = counter.clone();
        let c3 = counter.clone();

        // First batch of tasks
        tasks.add_task(move || async move {
            c1.fetch_add(1, Ordering::SeqCst);
        });
        tasks.add_task(move || async move {
            c2.fetch_add(10, Ordering::SeqCst);
        });

        // Execute first batch
        futures_executor::block_on(tasks.execute_all());
        assert_eq!(
            counter.load(Ordering::SeqCst),
            11,
            "first batch should complete"
        );

        // Create new tasks (simulating a task that spawns more work)
        let mut more_tasks = BackgroundTasks::new();
        more_tasks.add_task(move || async move {
            c3.fetch_add(100, Ordering::SeqCst);
        });

        // Execute second batch
        futures_executor::block_on(more_tasks.execute_all());
        assert_eq!(
            counter.load(Ordering::SeqCst),
            111,
            "spawned tasks should also run"
        );
    }

    #[test]
    fn background_tasks_timing_independent_requests() {
        // Verify that tasks from different requests are independent
        // (simulating concurrent request handling)
        let ctx1 = test_context();
        let ctx2 = test_context();

        let mut req1 = Request::new(Method::Get, "/request1");
        let mut req2 = Request::new(Method::Get, "/request2");

        let counter1 = Arc::new(AtomicU32::new(0));
        let counter2 = Arc::new(AtomicU32::new(0));
        let c1 = counter1.clone();
        let c2 = counter2.clone();

        // Request 1 adds task
        let mut tasks1 =
            futures_executor::block_on(BackgroundTasks::from_request(&ctx1, &mut req1))
                .expect("extraction should succeed");
        tasks1.add_task(move || async move {
            c1.store(100, Ordering::SeqCst);
        });

        // Request 2 adds task
        let mut tasks2 =
            futures_executor::block_on(BackgroundTasks::from_request(&ctx2, &mut req2))
                .expect("extraction should succeed");
        tasks2.add_task(move || async move {
            c2.store(200, Ordering::SeqCst);
        });

        // Execute request 1's tasks
        let taken1 = req1
            .get_extension::<BackgroundTasksInner>()
            .map(|inner| BackgroundTasks::from_inner(inner.clone()))
            .expect("tasks should be in extension");
        futures_executor::block_on(taken1.execute_all());

        // Only counter1 should be affected
        assert_eq!(
            counter1.load(Ordering::SeqCst),
            100,
            "request 1 task should run"
        );
        assert_eq!(
            counter2.load(Ordering::SeqCst),
            0,
            "request 2 task should not run yet"
        );

        // Execute request 2's tasks
        let taken2 = req2
            .get_extension::<BackgroundTasksInner>()
            .map(|inner| BackgroundTasks::from_inner(inner.clone()))
            .expect("tasks should be in extension");
        futures_executor::block_on(taken2.execute_all());

        // Now both should be set
        assert_eq!(
            counter1.load(Ordering::SeqCst),
            100,
            "request 1 task unchanged"
        );
        assert_eq!(
            counter2.load(Ordering::SeqCst),
            200,
            "request 2 task should run"
        );
    }

    #[test]
    fn background_tasks_timing_nonblocking_next_request() {
        // Verify that one request's tasks don't block handling of next request
        // This is implicit in the design: tasks are separate from response handling

        let ctx = test_context();
        let mut req1 = Request::new(Method::Get, "/first");
        let mut req2 = Request::new(Method::Get, "/second");

        let req1_done = Arc::new(AtomicBool::new(false));
        let req2_done = Arc::new(AtomicBool::new(false));
        let r1 = req1_done.clone();
        let r2 = req2_done.clone();

        // First request adds a task
        let mut tasks1 = futures_executor::block_on(BackgroundTasks::from_request(&ctx, &mut req1))
            .expect("extraction should succeed");
        tasks1.add_task(move || async move {
            // Simulate some work
            r1.store(true, Ordering::SeqCst);
        });

        // Second request can be handled immediately (before first request's tasks run)
        let mut tasks2 = futures_executor::block_on(BackgroundTasks::from_request(&ctx, &mut req2))
            .expect("extraction should succeed");
        tasks2.add_task(move || async move {
            r2.store(true, Ordering::SeqCst);
        });

        // At this point, neither request's tasks have run
        assert!(
            !req1_done.load(Ordering::SeqCst),
            "req1 tasks not yet executed"
        );
        assert!(
            !req2_done.load(Ordering::SeqCst),
            "req2 tasks not yet executed"
        );

        // The second request's response could be "sent" before first request's tasks run
        // This demonstrates non-blocking behavior

        // Now execute both sets of tasks
        let taken1 = req1
            .get_extension::<BackgroundTasksInner>()
            .map(|inner| BackgroundTasks::from_inner(inner.clone()))
            .expect("tasks should be in extension");
        let taken2 = req2
            .get_extension::<BackgroundTasksInner>()
            .map(|inner| BackgroundTasks::from_inner(inner.clone()))
            .expect("tasks should be in extension");

        futures_executor::block_on(taken1.execute_all());
        futures_executor::block_on(taken2.execute_all());

        assert!(
            req1_done.load(Ordering::SeqCst),
            "req1 tasks should be done"
        );
        assert!(
            req2_done.load(Ordering::SeqCst),
            "req2 tasks should be done"
        );
    }
}

// ============================================================================
// Header Extractor
// ============================================================================

/// Header extractor for individual HTTP headers.
///
/// Extracts a single header value by name from the request. The header name
/// is derived from the generic type's name, converting from snake_case to
/// Header-Case (e.g., `x_request_id` -> `X-Request-Id`).
///
/// For required headers, extraction failure returns 400 Bad Request.
/// Use `Option<Header<T>>` for optional headers.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::Header;
///
/// // Extract Authorization header (required)
/// async fn protected(auth: Header<String>) -> impl IntoResponse {
///     format!("Authorized with: {}", auth.0)
/// }
///
/// // Extract optional header
/// async fn optional_header(trace_id: Option<Header<String>>) -> impl IntoResponse {
///     match trace_id {
///         Some(Header(id)) => format!("Trace: {id}"),
///         None => "No trace".into(),
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Header<T> {
    /// The extracted header value.
    pub value: T,
    /// The original header name used for extraction.
    pub name: String,
}

impl<T> Header<T> {
    /// Create a new Header wrapper.
    #[must_use]
    pub fn new(name: impl Into<String>, value: T) -> Self {
        Self {
            value,
            name: name.into(),
        }
    }

    /// Unwrap the inner value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T> Deref for Header<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> DerefMut for Header<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

/// Convert a snake_case name to Header-Case.
///
/// Examples:
/// - `x_request_id` -> `X-Request-Id`
/// - `content_type` -> `Content-Type`
/// - `authorization` -> `Authorization`
#[must_use]
pub fn snake_to_header_case(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut result = first.to_uppercase().to_string();
                    result.extend(chars);
                    result
                }
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Error returned when header extraction fails.
#[derive(Debug)]
pub enum HeaderExtractError {
    /// Required header is missing from the request.
    MissingHeader {
        /// The header name that was expected.
        name: String,
    },
    /// Header value could not be parsed as UTF-8.
    InvalidUtf8 {
        /// The header name.
        name: String,
    },
    /// Header value could not be parsed to the target type.
    ParseError {
        /// The header name.
        name: String,
        /// The raw value that couldn't be parsed.
        value: String,
        /// Description of the expected type.
        expected: &'static str,
        /// The parse error message.
        message: String,
    },
}

impl std::fmt::Display for HeaderExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingHeader { name } => {
                write!(f, "Missing required header: {name}")
            }
            Self::InvalidUtf8 { name } => {
                write!(f, "Header '{name}' contains invalid UTF-8")
            }
            Self::ParseError {
                name,
                value,
                expected,
                message,
            } => {
                write!(
                    f,
                    "Failed to parse header '{name}' value '{value}' as {expected}: {message}"
                )
            }
        }
    }
}

impl std::error::Error for HeaderExtractError {}

impl IntoResponse for HeaderExtractError {
    fn into_response(self) -> crate::response::Response {
        // Missing or invalid headers are client errors (400)
        let error = match &self {
            HeaderExtractError::MissingHeader { name } => {
                ValidationError::missing(crate::error::loc::header(name))
                    .with_msg(format!("Missing required header: {name}"))
            }
            HeaderExtractError::InvalidUtf8 { name } => {
                ValidationError::type_error(crate::error::loc::header(name), "string")
                    .with_msg(format!("Header '{name}' contains invalid UTF-8"))
            }
            HeaderExtractError::ParseError {
                name,
                value,
                expected,
                message,
            } => ValidationError::type_error(crate::error::loc::header(name), expected)
                .with_msg(format!("Failed to parse as {expected}: {message}"))
                .with_input(serde_json::Value::String(value.clone())),
        };
        ValidationErrors::single(error).into_response()
    }
}

/// Trait for types that can be extracted from header values.
pub trait FromHeaderValue: Sized {
    /// Parse the header value.
    fn from_header_value(value: &str) -> Result<Self, String>;

    /// Return the expected type name for error messages.
    fn type_name() -> &'static str;
}

impl FromHeaderValue for String {
    fn from_header_value(value: &str) -> Result<Self, String> {
        Ok(value.to_string())
    }

    fn type_name() -> &'static str {
        "String"
    }
}

impl FromHeaderValue for i32 {
    fn from_header_value(value: &str) -> Result<Self, String> {
        value.parse().map_err(|e| format!("{e}"))
    }

    fn type_name() -> &'static str {
        "i32"
    }
}

impl FromHeaderValue for i64 {
    fn from_header_value(value: &str) -> Result<Self, String> {
        value.parse().map_err(|e| format!("{e}"))
    }

    fn type_name() -> &'static str {
        "i64"
    }
}

impl FromHeaderValue for u32 {
    fn from_header_value(value: &str) -> Result<Self, String> {
        value.parse().map_err(|e| format!("{e}"))
    }

    fn type_name() -> &'static str {
        "u32"
    }
}

impl FromHeaderValue for u64 {
    fn from_header_value(value: &str) -> Result<Self, String> {
        value.parse().map_err(|e| format!("{e}"))
    }

    fn type_name() -> &'static str {
        "u64"
    }
}

impl FromHeaderValue for bool {
    fn from_header_value(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(format!("invalid boolean: {value}")),
        }
    }

    fn type_name() -> &'static str {
        "bool"
    }
}

/// Named header extractor with explicit header name.
///
/// Use this when the header name doesn't match a type name.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::NamedHeader;
///
/// async fn handler(
///     auth: NamedHeader<String, AuthorizationHeader>,
///     trace: NamedHeader<String, XRequestIdHeader>,
/// ) -> impl IntoResponse {
///     // ...
/// }
///
/// struct AuthorizationHeader;
/// impl HeaderName for AuthorizationHeader {
///     const NAME: &'static str = "Authorization";
/// }
/// ```
#[derive(Debug, Clone)]
pub struct NamedHeader<T, N> {
    /// The extracted header value.
    pub value: T,
    _marker: std::marker::PhantomData<N>,
}

/// Trait for header name markers.
pub trait HeaderName {
    /// The HTTP header name.
    const NAME: &'static str;
}

impl<T, N> NamedHeader<T, N> {
    /// Create a new named header wrapper.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            value,
            _marker: std::marker::PhantomData,
        }
    }

    /// Unwrap the inner value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T, N> Deref for NamedHeader<T, N> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T, N> DerefMut for NamedHeader<T, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

impl<T, N> FromRequest for NamedHeader<T, N>
where
    T: FromHeaderValue + Send + Sync + 'static,
    N: HeaderName + Send + Sync + 'static,
{
    type Error = HeaderExtractError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let header_name = N::NAME;

        let value_bytes =
            req.headers()
                .get(header_name)
                .ok_or_else(|| HeaderExtractError::MissingHeader {
                    name: header_name.to_string(),
                })?;

        let value_str =
            std::str::from_utf8(value_bytes).map_err(|_| HeaderExtractError::InvalidUtf8 {
                name: header_name.to_string(),
            })?;

        let value =
            T::from_header_value(value_str).map_err(|message| HeaderExtractError::ParseError {
                name: header_name.to_string(),
                value: value_str.to_string(),
                expected: T::type_name(),
                message,
            })?;

        Ok(NamedHeader::new(value))
    }
}

// Common header name markers
/// Authorization header marker.
pub struct Authorization;
impl HeaderName for Authorization {
    const NAME: &'static str = "authorization";
}

/// Content-Type header marker.
pub struct ContentType;
impl HeaderName for ContentType {
    const NAME: &'static str = "content-type";
}

/// Accept header marker.
pub struct Accept;
impl HeaderName for Accept {
    const NAME: &'static str = "accept";
}

/// X-Request-Id header marker.
pub struct XRequestId;
impl HeaderName for XRequestId {
    const NAME: &'static str = "x-request-id";
}

/// User-Agent header marker.
pub struct UserAgent;
impl HeaderName for UserAgent {
    const NAME: &'static str = "user-agent";
}

/// Host header marker.
pub struct Host;
impl HeaderName for Host {
    const NAME: &'static str = "host";
}

// ============================================================================
// OAuth2 Security Extractors
// ============================================================================

/// OAuth2 password bearer security scheme extractor.
///
/// Extracts a bearer token from the `Authorization` header. This implements
/// the OAuth2 password bearer flow where the client sends a token in the
/// format `Bearer <token>`.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::OAuth2PasswordBearer;
///
/// async fn protected_route(token: OAuth2PasswordBearer) -> impl IntoResponse {
///     // Validate the token and get user
///     let user = validate_token(&token.token).await?;
///     format!("Hello, {}!", user.name)
/// }
/// ```
///
/// # Auto-Error Behavior
///
/// When `auto_error` is `true` (default), missing or invalid tokens result
/// in a 401 Unauthorized response with a `WWW-Authenticate: Bearer` header.
///
/// When `auto_error` is `false`, use `Option<OAuth2PasswordBearer>` to handle
/// missing tokens in your handler logic.
///
/// # OpenAPI
///
/// This extractor generates the following OpenAPI security scheme:
/// ```yaml
/// securitySchemes:
///   OAuth2PasswordBearer:
///     type: oauth2
///     flows:
///       password:
///         tokenUrl: "/token"
///         scopes: {}
/// ```
#[derive(Debug, Clone)]
pub struct OAuth2PasswordBearer {
    /// The extracted bearer token (without the "Bearer " prefix).
    pub token: String,
}

impl OAuth2PasswordBearer {
    /// Create a new OAuth2PasswordBearer with the given token.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// Get the token value.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Consume self and return the token.
    #[must_use]
    pub fn into_token(self) -> String {
        self.token
    }
}

impl Deref for OAuth2PasswordBearer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.token
    }
}

/// Configuration for OAuth2PasswordBearer extraction.
///
/// Use this to customize the token extraction behavior.
#[derive(Debug, Clone)]
pub struct OAuth2PasswordBearerConfig {
    /// URL to obtain the token. Required for OpenAPI documentation.
    pub token_url: String,
    /// URL to refresh the token. Optional.
    pub refresh_url: Option<String>,
    /// OAuth2 scopes with their descriptions.
    pub scopes: std::collections::HashMap<String, String>,
    /// Custom scheme name for OpenAPI documentation.
    pub scheme_name: Option<String>,
    /// Description for OpenAPI documentation.
    pub description: Option<String>,
    /// Whether to automatically return 401 on missing/invalid token.
    /// Default: true.
    pub auto_error: bool,
}

impl Default for OAuth2PasswordBearerConfig {
    fn default() -> Self {
        Self {
            token_url: "/token".to_string(),
            refresh_url: None,
            scopes: std::collections::HashMap::new(),
            scheme_name: None,
            description: None,
            auto_error: true,
        }
    }
}

impl OAuth2PasswordBearerConfig {
    /// Create a new configuration with the given token URL.
    #[must_use]
    pub fn new(token_url: impl Into<String>) -> Self {
        Self {
            token_url: token_url.into(),
            ..Default::default()
        }
    }

    /// Set the refresh URL.
    #[must_use]
    pub fn with_refresh_url(mut self, url: impl Into<String>) -> Self {
        self.refresh_url = Some(url.into());
        self
    }

    /// Add an OAuth2 scope.
    #[must_use]
    pub fn with_scope(mut self, scope: impl Into<String>, description: impl Into<String>) -> Self {
        self.scopes.insert(scope.into(), description.into());
        self
    }

    /// Set the scheme name for OpenAPI.
    #[must_use]
    pub fn with_scheme_name(mut self, name: impl Into<String>) -> Self {
        self.scheme_name = Some(name.into());
        self
    }

    /// Set the description for OpenAPI.
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set whether to auto-error on missing/invalid tokens.
    #[must_use]
    pub fn with_auto_error(mut self, auto_error: bool) -> Self {
        self.auto_error = auto_error;
        self
    }
}

/// Error when OAuth2 bearer token extraction fails.
#[derive(Debug, Clone)]
pub struct OAuth2BearerError {
    /// The kind of error that occurred.
    pub kind: OAuth2BearerErrorKind,
}

/// The specific kind of OAuth2 bearer error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuth2BearerErrorKind {
    /// Authorization header is missing.
    MissingHeader,
    /// Authorization header doesn't start with "Bearer ".
    InvalidScheme,
    /// Token is empty after "Bearer " prefix.
    EmptyToken,
}

impl OAuth2BearerError {
    /// Create a new missing header error.
    #[must_use]
    pub fn missing_header() -> Self {
        Self {
            kind: OAuth2BearerErrorKind::MissingHeader,
        }
    }

    /// Create a new invalid scheme error.
    #[must_use]
    pub fn invalid_scheme() -> Self {
        Self {
            kind: OAuth2BearerErrorKind::InvalidScheme,
        }
    }

    /// Create a new empty token error.
    #[must_use]
    pub fn empty_token() -> Self {
        Self {
            kind: OAuth2BearerErrorKind::EmptyToken,
        }
    }
}

impl fmt::Display for OAuth2BearerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            OAuth2BearerErrorKind::MissingHeader => {
                write!(f, "Missing Authorization header")
            }
            OAuth2BearerErrorKind::InvalidScheme => {
                write!(f, "Authorization header must use Bearer scheme")
            }
            OAuth2BearerErrorKind::EmptyToken => {
                write!(f, "Bearer token is empty")
            }
        }
    }
}

impl IntoResponse for OAuth2BearerError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let message = match self.kind {
            OAuth2BearerErrorKind::MissingHeader => "Not authenticated",
            OAuth2BearerErrorKind::InvalidScheme => "Invalid authentication credentials",
            OAuth2BearerErrorKind::EmptyToken => "Invalid authentication credentials",
        };

        let body = serde_json::json!({
            "detail": message
        });

        Response::with_status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", b"Bearer".to_vec())
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for OAuth2PasswordBearer {
    type Error = OAuth2BearerError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get the Authorization header
        let auth_header = req
            .headers()
            .get("authorization")
            .ok_or_else(OAuth2BearerError::missing_header)?;

        // Convert to string
        let auth_str =
            std::str::from_utf8(auth_header).map_err(|_| OAuth2BearerError::invalid_scheme())?;

        // Check for "Bearer " prefix (case-insensitive)
        const BEARER_PREFIX: &str = "Bearer ";
        const BEARER_PREFIX_LOWER: &str = "bearer ";

        let token = if auth_str.starts_with(BEARER_PREFIX) {
            &auth_str[BEARER_PREFIX.len()..]
        } else if auth_str.starts_with(BEARER_PREFIX_LOWER) {
            &auth_str[BEARER_PREFIX_LOWER.len()..]
        } else {
            return Err(OAuth2BearerError::invalid_scheme());
        };

        // Check token isn't empty
        let token = token.trim();
        if token.is_empty() {
            return Err(OAuth2BearerError::empty_token());
        }

        Ok(OAuth2PasswordBearer::new(token))
    }
}

// ============================================================================
// OAuth2 Password Request Form
// ============================================================================

/// Error when OAuth2 password request form extraction fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuth2PasswordFormError {
    /// Content-Type is not `application/x-www-form-urlencoded`.
    UnsupportedMediaType {
        /// The actual Content-Type, if any.
        actual: Option<String>,
    },
    /// Request body exceeds the configured limit.
    PayloadTooLarge {
        /// The actual body size.
        size: usize,
        /// The configured limit.
        limit: usize,
    },
    /// The `username` field is missing.
    MissingUsername,
    /// The `password` field is missing.
    MissingPassword,
    /// The `grant_type` must be `"password"` (strict mode).
    InvalidGrantType {
        /// The actual grant_type value.
        actual: String,
    },
    /// The `grant_type` field is missing (strict mode).
    MissingGrantType,
    /// Request body is not valid UTF-8.
    InvalidUtf8,
    /// Streaming bodies are not supported.
    StreamingNotSupported,
}

impl std::fmt::Display for OAuth2PasswordFormError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedMediaType { actual } => {
                if let Some(ct) = actual {
                    write!(f, "Expected application/x-www-form-urlencoded, got: {ct}")
                } else {
                    write!(f, "Missing Content-Type header")
                }
            }
            Self::PayloadTooLarge { size, limit } => {
                write!(f, "Body too large: {size} > {limit}")
            }
            Self::MissingUsername => write!(f, "Missing required field: username"),
            Self::MissingPassword => write!(f, "Missing required field: password"),
            Self::InvalidGrantType { actual } => {
                write!(f, "grant_type must be \"password\", got: \"{actual}\"")
            }
            Self::MissingGrantType => write!(f, "Missing required field: grant_type"),
            Self::InvalidUtf8 => write!(f, "Invalid UTF-8 in form body"),
            Self::StreamingNotSupported => write!(f, "Streaming bodies not supported"),
        }
    }
}

impl std::error::Error for OAuth2PasswordFormError {}

impl IntoResponse for OAuth2PasswordFormError {
    fn into_response(self) -> Response {
        match &self {
            OAuth2PasswordFormError::UnsupportedMediaType { .. } => {
                HttpError::unsupported_media_type().into_response()
            }
            OAuth2PasswordFormError::PayloadTooLarge { size, limit } => {
                HttpError::payload_too_large()
                    .with_detail(format!("Body {size} > {limit}"))
                    .into_response()
            }
            OAuth2PasswordFormError::MissingUsername => ValidationErrors::single(
                ValidationError::new(
                    crate::error::error_types::MISSING,
                    vec![
                        crate::error::LocItem::field("body"),
                        crate::error::LocItem::field("username"),
                    ],
                )
                .with_msg("Field required".to_string()),
            )
            .into_response(),
            OAuth2PasswordFormError::MissingPassword => ValidationErrors::single(
                ValidationError::new(
                    crate::error::error_types::MISSING,
                    vec![
                        crate::error::LocItem::field("body"),
                        crate::error::LocItem::field("password"),
                    ],
                )
                .with_msg("Field required".to_string()),
            )
            .into_response(),
            OAuth2PasswordFormError::InvalidGrantType { actual } => ValidationErrors::single(
                ValidationError::new(
                    crate::error::error_types::VALUE_ERROR,
                    vec![
                        crate::error::LocItem::field("body"),
                        crate::error::LocItem::field("grant_type"),
                    ],
                )
                .with_msg(format!(
                    "grant_type must be \"password\", got: \"{actual}\""
                )),
            )
            .into_response(),
            OAuth2PasswordFormError::MissingGrantType => ValidationErrors::single(
                ValidationError::new(
                    crate::error::error_types::MISSING,
                    vec![
                        crate::error::LocItem::field("body"),
                        crate::error::LocItem::field("grant_type"),
                    ],
                )
                .with_msg("Field required".to_string()),
            )
            .into_response(),
            OAuth2PasswordFormError::InvalidUtf8 => HttpError::bad_request()
                .with_detail("Invalid UTF-8")
                .into_response(),
            OAuth2PasswordFormError::StreamingNotSupported => {
                HttpError::bad_request().into_response()
            }
        }
    }
}

/// OAuth2 password request form data.
///
/// Extracts the standard OAuth2 password grant fields from a
/// `application/x-www-form-urlencoded` request body, matching FastAPI's
/// `OAuth2PasswordRequestForm`.
///
/// # Fields
///
/// - `grant_type`: Optional, should be `"password"` per spec (not enforced here; use
///   [`OAuth2PasswordRequestFormStrict`] for strict validation).
/// - `username`: Required.
/// - `password`: Required.
/// - `scope`: Space-separated scopes (empty string if not provided).
/// - `client_id`: Optional.
/// - `client_secret`: Optional.
///
/// # Computed
///
/// - [`scopes()`](OAuth2PasswordRequestForm::scopes): Returns the `scope` string split into a `Vec<String>`.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::OAuth2PasswordRequestForm;
///
/// async fn login(form: OAuth2PasswordRequestForm) -> Response {
///     let username = &form.username;
///     let password = &form.password;
///     let scopes = form.scopes();
///     // ... authenticate user ...
/// }
/// ```
#[derive(Debug, Clone)]
pub struct OAuth2PasswordRequestForm {
    /// The grant type. Should be `"password"` per the OAuth2 spec.
    pub grant_type: Option<String>,
    /// The username (required).
    pub username: String,
    /// The password (required).
    pub password: String,
    /// Space-separated scopes. Defaults to empty string.
    pub scope: String,
    /// Optional client ID.
    pub client_id: Option<String>,
    /// Optional client secret.
    pub client_secret: Option<String>,
}

impl OAuth2PasswordRequestForm {
    /// Parse the `scope` field into a vector of individual scope strings.
    #[must_use]
    pub fn scopes(&self) -> Vec<String> {
        if self.scope.is_empty() {
            Vec::new()
        } else {
            self.scope.split(' ').map(String::from).collect()
        }
    }
}

/// Parse form body into key-value pairs and extract a field by name.
fn extract_form_body(
    ctx: &RequestContext,
    req: &mut Request,
) -> Result<QueryParams, OAuth2PasswordFormError> {
    // Validate Content-Type
    let ct = req
        .headers()
        .get("content-type")
        .and_then(|v| std::str::from_utf8(v).ok());
    let is_form = ct.is_some_and(|c| {
        c.to_ascii_lowercase()
            .starts_with("application/x-www-form-urlencoded")
    });
    if !is_form {
        return Err(OAuth2PasswordFormError::UnsupportedMediaType {
            actual: ct.map(String::from),
        });
    }

    // Extract body
    let body = req.take_body();
    let bytes = match body {
        Body::Empty => Vec::new(),
        Body::Bytes(b) => b,
        Body::Stream(_) => return Err(OAuth2PasswordFormError::StreamingNotSupported),
    };

    // Check size limit
    let limit = ctx.max_body_size();
    if bytes.len() > limit {
        return Err(OAuth2PasswordFormError::PayloadTooLarge {
            size: bytes.len(),
            limit,
        });
    }

    // Parse form body
    let body_str = std::str::from_utf8(&bytes).map_err(|_| OAuth2PasswordFormError::InvalidUtf8)?;
    Ok(QueryParams::parse(body_str))
}

impl FromRequest for OAuth2PasswordRequestForm {
    type Error = OAuth2PasswordFormError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let _ = ctx.checkpoint();
        let params = extract_form_body(ctx, req)?;
        let _ = ctx.checkpoint();

        let username = params
            .get("username")
            .ok_or(OAuth2PasswordFormError::MissingUsername)?
            .to_string();

        let password = params
            .get("password")
            .ok_or(OAuth2PasswordFormError::MissingPassword)?
            .to_string();

        let grant_type = params.get("grant_type").map(String::from);
        let scope = params.get("scope").map(String::from).unwrap_or_default();
        let client_id = params.get("client_id").map(String::from);
        let client_secret = params.get("client_secret").map(String::from);

        let _ = ctx.checkpoint();
        Ok(OAuth2PasswordRequestForm {
            grant_type,
            username,
            password,
            scope,
            client_id,
            client_secret,
        })
    }
}

/// Strict variant of [`OAuth2PasswordRequestForm`].
///
/// Same as [`OAuth2PasswordRequestForm`], but requires `grant_type` to be
/// present and equal to `"password"`. Returns an error if `grant_type` is
/// missing or has any other value.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::OAuth2PasswordRequestFormStrict;
///
/// async fn login(form: OAuth2PasswordRequestFormStrict) -> Response {
///     // grant_type is guaranteed to be "password"
///     let username = &form.form.username;
///     let scopes = form.form.scopes();
///     // ...
/// }
/// ```
#[derive(Debug, Clone)]
pub struct OAuth2PasswordRequestFormStrict {
    /// The validated form data. The `grant_type` is guaranteed to be `Some("password")`.
    pub form: OAuth2PasswordRequestForm,
}

impl OAuth2PasswordRequestFormStrict {
    /// Get a reference to the inner form.
    #[must_use]
    pub fn inner(&self) -> &OAuth2PasswordRequestForm {
        &self.form
    }

    /// Consume self and return the inner form.
    #[must_use]
    pub fn into_inner(self) -> OAuth2PasswordRequestForm {
        self.form
    }
}

impl std::ops::Deref for OAuth2PasswordRequestFormStrict {
    type Target = OAuth2PasswordRequestForm;

    fn deref(&self) -> &Self::Target {
        &self.form
    }
}

impl FromRequest for OAuth2PasswordRequestFormStrict {
    type Error = OAuth2PasswordFormError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let _ = ctx.checkpoint();
        let params = extract_form_body(ctx, req)?;
        let _ = ctx.checkpoint();

        let grant_type_value = params
            .get("grant_type")
            .ok_or(OAuth2PasswordFormError::MissingGrantType)?;

        if grant_type_value != "password" {
            return Err(OAuth2PasswordFormError::InvalidGrantType {
                actual: grant_type_value.to_string(),
            });
        }

        let username = params
            .get("username")
            .ok_or(OAuth2PasswordFormError::MissingUsername)?
            .to_string();

        let password = params
            .get("password")
            .ok_or(OAuth2PasswordFormError::MissingPassword)?
            .to_string();

        let scope = params.get("scope").map(String::from).unwrap_or_default();
        let client_id = params.get("client_id").map(String::from);
        let client_secret = params.get("client_secret").map(String::from);

        let _ = ctx.checkpoint();
        Ok(OAuth2PasswordRequestFormStrict {
            form: OAuth2PasswordRequestForm {
                grant_type: Some("password".to_string()),
                username,
                password,
                scope,
                client_id,
                client_secret,
            },
        })
    }
}

#[cfg(test)]
mod oauth2_password_form_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    fn form_request(body: &str) -> Request {
        let mut req = Request::new(Method::Post, "/token");
        req.headers_mut().insert(
            "content-type",
            b"application/x-www-form-urlencoded".to_vec(),
        );
        req.set_body(Body::Bytes(body.as_bytes().to_vec()));
        req
    }

    // ---- OAuth2PasswordRequestForm tests ----

    #[test]
    fn basic_form_extraction() {
        let ctx = test_context();
        let mut req = form_request("username=alice&password=secret123");
        let form =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap();

        assert_eq!(form.username, "alice");
        assert_eq!(form.password, "secret123");
        assert!(form.grant_type.is_none());
        assert_eq!(form.scope, "");
        assert!(form.client_id.is_none());
        assert!(form.client_secret.is_none());
    }

    #[test]
    fn full_form_extraction() {
        let ctx = test_context();
        let body = "grant_type=password&username=bob&password=s3cret&scope=read+write&client_id=myapp&client_secret=appsecret";
        let mut req = form_request(body);
        let form =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap();

        assert_eq!(form.grant_type.as_deref(), Some("password"));
        assert_eq!(form.username, "bob");
        assert_eq!(form.password, "s3cret");
        assert_eq!(form.scope, "read write");
        assert_eq!(form.client_id.as_deref(), Some("myapp"));
        assert_eq!(form.client_secret.as_deref(), Some("appsecret"));
    }

    #[test]
    fn scopes_parsing() {
        let ctx = test_context();
        let mut req = form_request("username=u&password=p&scope=read+write+admin");
        let form =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap();

        let scopes = form.scopes();
        assert_eq!(scopes, vec!["read", "write", "admin"]);
    }

    #[test]
    fn empty_scope_returns_empty_vec() {
        let ctx = test_context();
        let mut req = form_request("username=u&password=p");
        let form =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap();

        assert!(form.scopes().is_empty());
    }

    #[test]
    fn missing_username_error() {
        let ctx = test_context();
        let mut req = form_request("password=secret");
        let err =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap_err();

        assert_eq!(err, OAuth2PasswordFormError::MissingUsername);
        assert!(err.to_string().contains("username"));
    }

    #[test]
    fn missing_password_error() {
        let ctx = test_context();
        let mut req = form_request("username=alice");
        let err =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap_err();

        assert_eq!(err, OAuth2PasswordFormError::MissingPassword);
        assert!(err.to_string().contains("password"));
    }

    #[test]
    fn wrong_content_type_error() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/token");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"username=a&password=b".to_vec()));

        let err =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap_err();

        match err {
            OAuth2PasswordFormError::UnsupportedMediaType { actual } => {
                assert_eq!(actual.as_deref(), Some("application/json"));
            }
            other => panic!("Expected UnsupportedMediaType, got: {other:?}"),
        }
    }

    #[test]
    fn missing_content_type_error() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/token");
        req.set_body(Body::Bytes(b"username=a&password=b".to_vec()));

        let err =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap_err();

        match err {
            OAuth2PasswordFormError::UnsupportedMediaType { actual } => {
                assert!(actual.is_none());
            }
            other => panic!("Expected UnsupportedMediaType, got: {other:?}"),
        }
    }

    #[test]
    fn url_encoded_values() {
        let ctx = test_context();
        let mut req = form_request("username=user%40example.com&password=p%26ss%3Dword");
        let form =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap();

        assert_eq!(form.username, "user@example.com");
        assert_eq!(form.password, "p&ss=word");
    }

    #[test]
    fn plus_decoded_as_space_in_scope() {
        let ctx = test_context();
        let mut req = form_request("username=u&password=p&scope=read+write+admin");
        let form =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap();

        // '+' is decoded as space in application/x-www-form-urlencoded
        assert_eq!(form.scope, "read write admin");
    }

    #[test]
    fn empty_body_returns_missing_username() {
        let ctx = test_context();
        let mut req = form_request("");
        let err =
            futures_executor::block_on(OAuth2PasswordRequestForm::from_request(&ctx, &mut req))
                .unwrap_err();
        assert_eq!(err, OAuth2PasswordFormError::MissingUsername);
    }

    #[test]
    fn streaming_not_supported_error_type() {
        // Verify the streaming error variant exists and has correct display
        let err = OAuth2PasswordFormError::StreamingNotSupported;
        assert!(err.to_string().contains("Streaming"));

        // Verify it converts to a 400 Bad Request response
        let resp = err.into_response();
        assert_eq!(resp.status().as_u16(), 400);
    }

    #[test]
    fn error_display_messages() {
        assert!(
            OAuth2PasswordFormError::MissingUsername
                .to_string()
                .contains("username")
        );
        assert!(
            OAuth2PasswordFormError::MissingPassword
                .to_string()
                .contains("password")
        );
        assert!(
            OAuth2PasswordFormError::InvalidUtf8
                .to_string()
                .contains("UTF-8")
        );
        assert!(
            OAuth2PasswordFormError::StreamingNotSupported
                .to_string()
                .contains("Streaming")
        );

        let err = OAuth2PasswordFormError::InvalidGrantType {
            actual: "code".to_string(),
        };
        assert!(err.to_string().contains("code"));

        let err = OAuth2PasswordFormError::PayloadTooLarge {
            size: 100,
            limit: 50,
        };
        assert!(err.to_string().contains("100"));
    }

    #[test]
    fn error_into_response_status_codes() {
        let resp = OAuth2PasswordFormError::MissingUsername.into_response();
        assert_eq!(resp.status().as_u16(), 422);

        let resp = OAuth2PasswordFormError::MissingPassword.into_response();
        assert_eq!(resp.status().as_u16(), 422);

        let resp = OAuth2PasswordFormError::UnsupportedMediaType { actual: None }.into_response();
        assert_eq!(resp.status().as_u16(), 415);

        let resp = OAuth2PasswordFormError::PayloadTooLarge {
            size: 100,
            limit: 50,
        }
        .into_response();
        assert_eq!(resp.status().as_u16(), 413);

        let resp = OAuth2PasswordFormError::InvalidGrantType {
            actual: "code".to_string(),
        }
        .into_response();
        assert_eq!(resp.status().as_u16(), 422);
    }

    // ---- OAuth2PasswordRequestFormStrict tests ----

    #[test]
    fn strict_accepts_password_grant_type() {
        let ctx = test_context();
        let mut req = form_request("grant_type=password&username=alice&password=secret");
        let form = futures_executor::block_on(OAuth2PasswordRequestFormStrict::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        assert_eq!(form.form.grant_type.as_deref(), Some("password"));
        assert_eq!(form.username, "alice");
        assert_eq!(form.password, "secret");
    }

    #[test]
    fn strict_rejects_missing_grant_type() {
        let ctx = test_context();
        let mut req = form_request("username=alice&password=secret");
        let err = futures_executor::block_on(OAuth2PasswordRequestFormStrict::from_request(
            &ctx, &mut req,
        ))
        .unwrap_err();

        assert_eq!(err, OAuth2PasswordFormError::MissingGrantType);
    }

    #[test]
    fn strict_rejects_wrong_grant_type() {
        let ctx = test_context();
        let mut req = form_request("grant_type=authorization_code&username=alice&password=secret");
        let err = futures_executor::block_on(OAuth2PasswordRequestFormStrict::from_request(
            &ctx, &mut req,
        ))
        .unwrap_err();

        match err {
            OAuth2PasswordFormError::InvalidGrantType { actual } => {
                assert_eq!(actual, "authorization_code");
            }
            other => panic!("Expected InvalidGrantType, got: {other:?}"),
        }
    }

    #[test]
    fn strict_with_all_fields() {
        let ctx = test_context();
        let body = "grant_type=password&username=bob&password=pw&scope=read+write&client_id=app&client_secret=sec";
        let mut req = form_request(body);
        let form = futures_executor::block_on(OAuth2PasswordRequestFormStrict::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        assert_eq!(form.username, "bob");
        assert_eq!(form.password, "pw");
        assert_eq!(form.scope, "read write");
        assert_eq!(form.client_id.as_deref(), Some("app"));
        assert_eq!(form.client_secret.as_deref(), Some("sec"));
        assert_eq!(form.scopes(), vec!["read", "write"]);
    }

    #[test]
    fn strict_deref_to_inner() {
        let ctx = test_context();
        let mut req = form_request("grant_type=password&username=alice&password=pw");
        let strict = futures_executor::block_on(OAuth2PasswordRequestFormStrict::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        // Deref allows accessing inner form fields directly
        let _: &str = &strict.username;
        let _: &str = &strict.password;
        assert_eq!(strict.inner().username, "alice");
    }

    #[test]
    fn strict_into_inner() {
        let ctx = test_context();
        let mut req = form_request("grant_type=password&username=alice&password=pw");
        let strict = futures_executor::block_on(OAuth2PasswordRequestFormStrict::from_request(
            &ctx, &mut req,
        ))
        .unwrap();

        let form = strict.into_inner();
        assert_eq!(form.username, "alice");
        assert_eq!(form.grant_type.as_deref(), Some("password"));
    }

    #[test]
    fn strict_missing_username_after_grant_type() {
        let ctx = test_context();
        let mut req = form_request("grant_type=password&password=secret");
        let err = futures_executor::block_on(OAuth2PasswordRequestFormStrict::from_request(
            &ctx, &mut req,
        ))
        .unwrap_err();
        assert_eq!(err, OAuth2PasswordFormError::MissingUsername);
    }

    #[test]
    fn strict_missing_password_after_grant_type() {
        let ctx = test_context();
        let mut req = form_request("grant_type=password&username=alice");
        let err = futures_executor::block_on(OAuth2PasswordRequestFormStrict::from_request(
            &ctx, &mut req,
        ))
        .unwrap_err();
        assert_eq!(err, OAuth2PasswordFormError::MissingPassword);
    }
}

// ============================================================================
// OAuth2 Authorization Code Bearer Extractor
// ============================================================================

/// Configuration for OAuth2 authorization code bearer extraction.
///
/// This configures the authorization code flow for OpenAPI documentation.
/// The actual token extraction is identical to [`OAuth2PasswordBearer`] —
/// the difference is in the OpenAPI security scheme generated.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::OAuth2AuthorizationCodeBearerConfig;
///
/// let config = OAuth2AuthorizationCodeBearerConfig::new(
///     "https://auth.example.com/authorize",
///     "https://auth.example.com/token",
/// )
/// .with_refresh_url("https://auth.example.com/refresh")
/// .with_scope("read", "Read access")
/// .with_scope("write", "Write access");
/// ```
#[derive(Debug, Clone)]
pub struct OAuth2AuthorizationCodeBearerConfig {
    /// URL for the authorization endpoint. Required for OpenAPI documentation.
    pub authorization_url: String,
    /// URL for the token endpoint. Required for OpenAPI documentation.
    pub token_url: String,
    /// URL to refresh the token. Optional.
    pub refresh_url: Option<String>,
    /// OAuth2 scopes with their descriptions.
    pub scopes: std::collections::HashMap<String, String>,
    /// Custom scheme name for OpenAPI documentation.
    pub scheme_name: Option<String>,
    /// Description for OpenAPI documentation.
    pub description: Option<String>,
    /// Whether to automatically return 401 on missing/invalid token.
    /// Default: true.
    pub auto_error: bool,
}

impl OAuth2AuthorizationCodeBearerConfig {
    /// Create a new configuration with the given authorization and token URLs.
    #[must_use]
    pub fn new(authorization_url: impl Into<String>, token_url: impl Into<String>) -> Self {
        Self {
            authorization_url: authorization_url.into(),
            token_url: token_url.into(),
            refresh_url: None,
            scopes: std::collections::HashMap::new(),
            scheme_name: None,
            description: None,
            auto_error: true,
        }
    }

    /// Set the refresh URL.
    #[must_use]
    pub fn with_refresh_url(mut self, url: impl Into<String>) -> Self {
        self.refresh_url = Some(url.into());
        self
    }

    /// Add an OAuth2 scope.
    #[must_use]
    pub fn with_scope(mut self, scope: impl Into<String>, description: impl Into<String>) -> Self {
        self.scopes.insert(scope.into(), description.into());
        self
    }

    /// Set the scheme name for OpenAPI.
    #[must_use]
    pub fn with_scheme_name(mut self, name: impl Into<String>) -> Self {
        self.scheme_name = Some(name.into());
        self
    }

    /// Set the description for OpenAPI.
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set whether to auto-error on missing/invalid tokens.
    #[must_use]
    pub fn with_auto_error(mut self, auto_error: bool) -> Self {
        self.auto_error = auto_error;
        self
    }
}

/// OAuth2 authorization code bearer extractor.
///
/// Extracts a bearer token from the `Authorization` header, identical to
/// [`OAuth2PasswordBearer`]. The difference is purely in OpenAPI documentation:
/// this generates an `oauth2` security scheme with `authorizationCode` flow
/// instead of `password` flow.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::OAuth2AuthorizationCodeBearer;
///
/// async fn protected(token: OAuth2AuthorizationCodeBearer) -> impl IntoResponse {
///     let access_token = token.token();
///     // Verify token with your auth provider...
/// }
/// ```
#[derive(Debug, Clone)]
pub struct OAuth2AuthorizationCodeBearer {
    /// The extracted bearer token (without the "Bearer " prefix).
    pub token: String,
}

impl OAuth2AuthorizationCodeBearer {
    /// Create a new instance with the given token.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// Get the token value.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Consume self and return the token.
    #[must_use]
    pub fn into_token(self) -> String {
        self.token
    }
}

impl Deref for OAuth2AuthorizationCodeBearer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.token
    }
}

impl FromRequest for OAuth2AuthorizationCodeBearer {
    type Error = OAuth2BearerError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Token extraction is identical to OAuth2PasswordBearer.
        // The difference is in the OpenAPI security scheme (authorizationCode flow).
        let auth_header = req
            .headers()
            .get("authorization")
            .ok_or_else(OAuth2BearerError::missing_header)?;

        let auth_str =
            std::str::from_utf8(auth_header).map_err(|_| OAuth2BearerError::invalid_scheme())?;

        const BEARER_PREFIX: &str = "Bearer ";
        const BEARER_PREFIX_LOWER: &str = "bearer ";

        let token = if auth_str.starts_with(BEARER_PREFIX) {
            &auth_str[BEARER_PREFIX.len()..]
        } else if auth_str.starts_with(BEARER_PREFIX_LOWER) {
            &auth_str[BEARER_PREFIX_LOWER.len()..]
        } else {
            return Err(OAuth2BearerError::invalid_scheme());
        };

        let token = token.trim();
        if token.is_empty() {
            return Err(OAuth2BearerError::empty_token());
        }

        Ok(OAuth2AuthorizationCodeBearer::new(token))
    }
}

#[cfg(test)]
mod oauth2_authcode_bearer_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    // ---- Config tests ----

    #[test]
    fn config_new() {
        let config = OAuth2AuthorizationCodeBearerConfig::new(
            "https://auth.example.com/authorize",
            "https://auth.example.com/token",
        );
        assert_eq!(
            config.authorization_url,
            "https://auth.example.com/authorize"
        );
        assert_eq!(config.token_url, "https://auth.example.com/token");
        assert!(config.refresh_url.is_none());
        assert!(config.scopes.is_empty());
        assert!(config.scheme_name.is_none());
        assert!(config.description.is_none());
        assert!(config.auto_error);
    }

    #[test]
    fn config_builder() {
        let config = OAuth2AuthorizationCodeBearerConfig::new(
            "https://auth.example.com/authorize",
            "https://auth.example.com/token",
        )
        .with_refresh_url("https://auth.example.com/refresh")
        .with_scope("read", "Read access")
        .with_scope("write", "Write access")
        .with_scheme_name("MyAuth")
        .with_description("Authorization code flow")
        .with_auto_error(false);

        assert_eq!(
            config.refresh_url.as_deref(),
            Some("https://auth.example.com/refresh")
        );
        assert_eq!(config.scopes.len(), 2);
        assert_eq!(config.scopes.get("read").unwrap(), "Read access");
        assert_eq!(config.scopes.get("write").unwrap(), "Write access");
        assert_eq!(config.scheme_name.as_deref(), Some("MyAuth"));
        assert_eq!(
            config.description.as_deref(),
            Some("Authorization code flow")
        );
        assert!(!config.auto_error);
    }

    // ---- Extractor tests ----

    #[test]
    fn extracts_bearer_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer my-access-token".to_vec());

        let result =
            futures_executor::block_on(OAuth2AuthorizationCodeBearer::from_request(&ctx, &mut req))
                .unwrap();

        assert_eq!(result.token(), "my-access-token");
        assert_eq!(result.into_token(), "my-access-token");
    }

    #[test]
    fn extracts_lowercase_bearer() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/protected");
        req.headers_mut()
            .insert("authorization", b"bearer my-token".to_vec());

        let result =
            futures_executor::block_on(OAuth2AuthorizationCodeBearer::from_request(&ctx, &mut req))
                .unwrap();

        assert_eq!(result.token(), "my-token");
    }

    #[test]
    fn missing_header_returns_401() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/protected");

        let err =
            futures_executor::block_on(OAuth2AuthorizationCodeBearer::from_request(&ctx, &mut req))
                .unwrap_err();

        assert_eq!(err.kind, OAuth2BearerErrorKind::MissingHeader);

        let resp = err.into_response();
        assert_eq!(resp.status().as_u16(), 401);
    }

    #[test]
    fn invalid_scheme_returns_401() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/protected");
        req.headers_mut()
            .insert("authorization", b"Basic dXNlcjpwYXNz".to_vec());

        let err =
            futures_executor::block_on(OAuth2AuthorizationCodeBearer::from_request(&ctx, &mut req))
                .unwrap_err();

        assert_eq!(err.kind, OAuth2BearerErrorKind::InvalidScheme);
    }

    #[test]
    fn empty_token_returns_error() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer ".to_vec());

        let err =
            futures_executor::block_on(OAuth2AuthorizationCodeBearer::from_request(&ctx, &mut req))
                .unwrap_err();

        assert_eq!(err.kind, OAuth2BearerErrorKind::EmptyToken);
    }

    #[test]
    fn whitespace_only_token_returns_error() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer   ".to_vec());

        let err =
            futures_executor::block_on(OAuth2AuthorizationCodeBearer::from_request(&ctx, &mut req))
                .unwrap_err();

        assert_eq!(err.kind, OAuth2BearerErrorKind::EmptyToken);
    }

    #[test]
    fn token_trimmed() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer  my-token  ".to_vec());

        let result =
            futures_executor::block_on(OAuth2AuthorizationCodeBearer::from_request(&ctx, &mut req))
                .unwrap();

        assert_eq!(result.token(), "my-token");
    }

    #[test]
    fn deref_to_str() {
        let bearer = OAuth2AuthorizationCodeBearer::new("abc123");
        let s: &str = &bearer;
        assert_eq!(s, "abc123");
    }

    #[test]
    fn new_constructor() {
        let bearer = OAuth2AuthorizationCodeBearer::new("token-value");
        assert_eq!(bearer.token, "token-value");
        assert_eq!(bearer.token(), "token-value");
    }

    #[test]
    fn www_authenticate_header_on_error() {
        let err = OAuth2BearerError::missing_header();
        let resp = err.into_response();

        let has_www_auth = resp
            .headers()
            .iter()
            .any(|(n, v)| n.eq_ignore_ascii_case("www-authenticate") && v == b"Bearer");
        assert!(
            has_www_auth,
            "Response should have WWW-Authenticate: Bearer header"
        );
    }
}

// ============================================================================
// Security Scopes
// ============================================================================

/// Required OAuth2 security scopes for a handler.
///
/// `SecurityScopes` aggregates the scopes required by the current handler's
/// security dependency chain. It is typically injected alongside a bearer token
/// extractor so the handler can verify that the token grants the required
/// scopes.
///
/// In Python FastAPI, `SecurityScopes` is automatically populated from the
/// `scopes` parameter on `Security(...)` dependencies. In this Rust
/// implementation, scopes are set via request extensions (populated by
/// middleware or route configuration) and read by the `FromRequest` impl.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{SecurityScopes, BearerToken};
///
/// async fn get_admin(
///     token: BearerToken,
///     scopes: SecurityScopes,
/// ) -> impl IntoResponse {
///     // scopes.scopes() returns ["admin", "users:read"]
///     // Verify the token grants all required scopes
///     for scope in scopes.scopes() {
///         // check token has scope...
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct SecurityScopes {
    /// Required scopes (order preserved, no duplicates).
    scopes: Vec<String>,
    /// Space-separated scope string.
    scope_str: String,
}

impl SecurityScopes {
    /// Create empty security scopes (no scopes required).
    #[must_use]
    pub fn new() -> Self {
        Self {
            scopes: Vec::new(),
            scope_str: String::new(),
        }
    }

    /// Create from a list of scope strings.
    ///
    /// Duplicates are removed while preserving order.
    #[must_use]
    pub fn from_scopes(scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<String> = scopes
            .into_iter()
            .map(Into::into)
            .filter(|s| seen.insert(s.clone()))
            .collect();
        let scope_str = deduped.join(" ");
        Self {
            scopes: deduped,
            scope_str,
        }
    }

    /// Create from a space-separated scope string.
    ///
    /// The string is split on spaces, empty segments are ignored, and
    /// duplicates are removed while preserving order.
    #[must_use]
    pub fn from_scope_str(scope_str: &str) -> Self {
        let parts = scope_str
            .split(' ')
            .filter(|s| !s.is_empty())
            .map(String::from);
        Self::from_scopes(parts)
    }

    /// Get the required scopes as a slice.
    #[must_use]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    /// Get the space-separated scope string.
    #[must_use]
    pub fn scope_str(&self) -> &str {
        &self.scope_str
    }

    /// Check if a specific scope is required.
    #[must_use]
    pub fn contains(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// Returns true if no scopes are required.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scopes.is_empty()
    }

    /// Returns the number of required scopes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.scopes.len()
    }

    /// Merge another set of scopes into this one.
    ///
    /// New scopes are appended, preserving order and deduplicating.
    pub fn merge(&mut self, other: &SecurityScopes) {
        let existing: std::collections::HashSet<String> = self.scopes.iter().cloned().collect();
        for scope in &other.scopes {
            if !existing.contains(scope) {
                self.scopes.push(scope.clone());
            }
        }
        self.scope_str = self.scopes.join(" ");
    }

    /// Create a new `SecurityScopes` by merging two sets.
    #[must_use]
    pub fn merged(&self, other: &SecurityScopes) -> Self {
        let mut result = self.clone();
        result.merge(other);
        result
    }
}

impl Default for SecurityScopes {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SecurityScopes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.scope_str)
    }
}

/// Error when SecurityScopes extraction fails.
///
/// This occurs when no security scopes have been configured for the route
/// (i.e., no `SecurityScopesData` extension exists on the request).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecurityScopesError;

impl std::fmt::Display for SecurityScopesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "No security scopes configured for this route")
    }
}

impl std::error::Error for SecurityScopesError {}

impl IntoResponse for SecurityScopesError {
    fn into_response(self) -> Response {
        HttpError::new(crate::response::StatusCode::INTERNAL_SERVER_ERROR)
            .with_detail("Security scopes not configured")
            .into_response()
    }
}

impl FromRequest for SecurityScopes {
    type Error = SecurityScopesError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Look for SecurityScopes stored as a request extension.
        // Middleware or route configuration should populate this before the handler runs.
        if let Some(scopes) = req.get_extension::<SecurityScopes>() {
            Ok(scopes.clone())
        } else {
            // If no scopes extension exists, return empty scopes (no scopes required).
            // This is lenient — handlers that don't configure scopes get an empty set.
            Ok(SecurityScopes::new())
        }
    }
}

#[cfg(test)]
mod security_scopes_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    // ---- Construction tests ----

    #[test]
    fn new_is_empty() {
        let scopes = SecurityScopes::new();
        assert!(scopes.is_empty());
        assert_eq!(scopes.len(), 0);
        assert_eq!(scopes.scope_str(), "");
        assert!(scopes.scopes().is_empty());
    }

    #[test]
    fn default_is_empty() {
        let scopes = SecurityScopes::default();
        assert!(scopes.is_empty());
    }

    #[test]
    fn from_scopes_preserves_order() {
        let scopes = SecurityScopes::from_scopes(["read", "write", "admin"]);
        assert_eq!(scopes.scopes(), &["read", "write", "admin"]);
        assert_eq!(scopes.scope_str(), "read write admin");
        assert_eq!(scopes.len(), 3);
    }

    #[test]
    fn from_scopes_deduplicates() {
        let scopes = SecurityScopes::from_scopes(["read", "write", "read", "admin", "write"]);
        assert_eq!(scopes.scopes(), &["read", "write", "admin"]);
        assert_eq!(scopes.scope_str(), "read write admin");
    }

    #[test]
    fn from_scope_str() {
        let scopes = SecurityScopes::from_scope_str("read write admin");
        assert_eq!(scopes.scopes(), &["read", "write", "admin"]);
        assert_eq!(scopes.scope_str(), "read write admin");
    }

    #[test]
    fn from_scope_str_deduplicates() {
        let scopes = SecurityScopes::from_scope_str("read write read admin");
        assert_eq!(scopes.scopes(), &["read", "write", "admin"]);
    }

    #[test]
    fn from_scope_str_ignores_empty_segments() {
        let scopes = SecurityScopes::from_scope_str("read  write   admin");
        assert_eq!(scopes.scopes(), &["read", "write", "admin"]);
    }

    #[test]
    fn from_empty_scope_str() {
        let scopes = SecurityScopes::from_scope_str("");
        assert!(scopes.is_empty());
        assert_eq!(scopes.scope_str(), "");
    }

    // ---- Query methods ----

    #[test]
    fn contains_scope() {
        let scopes = SecurityScopes::from_scopes(["read", "write"]);
        assert!(scopes.contains("read"));
        assert!(scopes.contains("write"));
        assert!(!scopes.contains("admin"));
    }

    #[test]
    fn display_format() {
        let scopes = SecurityScopes::from_scopes(["read", "write"]);
        assert_eq!(format!("{scopes}"), "read write");

        let empty = SecurityScopes::new();
        assert_eq!(format!("{empty}"), "");
    }

    // ---- Merge tests ----

    #[test]
    fn merge_appends_new_scopes() {
        let mut base = SecurityScopes::from_scopes(["read"]);
        let other = SecurityScopes::from_scopes(["write", "admin"]);
        base.merge(&other);

        assert_eq!(base.scopes(), &["read", "write", "admin"]);
        assert_eq!(base.scope_str(), "read write admin");
    }

    #[test]
    fn merge_deduplicates() {
        let mut base = SecurityScopes::from_scopes(["read", "write"]);
        let other = SecurityScopes::from_scopes(["write", "admin"]);
        base.merge(&other);

        assert_eq!(base.scopes(), &["read", "write", "admin"]);
    }

    #[test]
    fn merge_empty_into_nonempty() {
        let mut base = SecurityScopes::from_scopes(["read"]);
        let other = SecurityScopes::new();
        base.merge(&other);

        assert_eq!(base.scopes(), &["read"]);
    }

    #[test]
    fn merge_nonempty_into_empty() {
        let mut base = SecurityScopes::new();
        let other = SecurityScopes::from_scopes(["read", "write"]);
        base.merge(&other);

        assert_eq!(base.scopes(), &["read", "write"]);
    }

    #[test]
    fn merged_returns_new_instance() {
        let base = SecurityScopes::from_scopes(["read"]);
        let other = SecurityScopes::from_scopes(["write"]);
        let combined = base.merged(&other);

        assert_eq!(combined.scopes(), &["read", "write"]);
        // Original unchanged
        assert_eq!(base.scopes(), &["read"]);
    }

    // ---- FromRequest tests ----

    #[test]
    fn extract_with_extension() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/protected");
        req.insert_extension(SecurityScopes::from_scopes(["admin", "users:read"]));

        let scopes =
            futures_executor::block_on(SecurityScopes::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(scopes.scopes(), &["admin", "users:read"]);
        assert_eq!(scopes.scope_str(), "admin users:read");
    }

    #[test]
    fn extract_without_extension_returns_empty() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/public");

        let scopes =
            futures_executor::block_on(SecurityScopes::from_request(&ctx, &mut req)).unwrap();
        assert!(scopes.is_empty());
    }

    // ---- Error tests ----

    #[test]
    fn error_display() {
        let err = SecurityScopesError;
        assert!(err.to_string().contains("security scopes"));
    }

    #[test]
    fn error_into_response_is_500() {
        let resp = SecurityScopesError.into_response();
        assert_eq!(resp.status().as_u16(), 500);
    }
}

// ============================================================================
// HTTP Bearer Token Extractor
// ============================================================================

/// Simple HTTP bearer token extractor.
///
/// Extracts a bearer token from the `Authorization` header. This is a simpler
/// alternative to [`OAuth2PasswordBearer`] when you don't need OAuth2-specific
/// functionality like token URLs and scopes.
///
/// This corresponds to FastAPI's `HTTPBearer` security scheme, which generates
/// an OpenAPI security scheme with `type: "http"` and `scheme: "bearer"`.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::BearerToken;
///
/// async fn protected_route(token: BearerToken) -> impl IntoResponse {
///     // Validate the token
///     if verify_token(&token) {
///         format!("Token valid: {}", token.token())
///     } else {
///         // Return error response
///     }
/// }
/// ```
///
/// # Error Handling
///
/// When the token is missing or invalid, a 401 Unauthorized response is returned
/// with a `WWW-Authenticate: Bearer` header, following RFC 6750.
///
/// # Optional Extraction
///
/// Wrap in `Option` to make the token optional:
///
/// ```ignore
/// async fn maybe_auth(token: Option<BearerToken>) -> impl IntoResponse {
///     match token {
///         Some(t) => format!("Authenticated with: {}", t.token()),
///         None => "Anonymous access".to_string(),
///     }
/// }
/// ```
///
/// # OpenAPI
///
/// This extractor generates the following OpenAPI security scheme:
/// ```yaml
/// securitySchemes:
///   BearerToken:
///     type: http
///     scheme: bearer
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerToken {
    /// The extracted bearer token (without the "Bearer " prefix).
    token: String,
}

impl BearerToken {
    /// Create a new BearerToken with the given token value.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// Get the token value.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Consume self and return the token string.
    #[must_use]
    pub fn into_token(self) -> String {
        self.token
    }
}

impl Deref for BearerToken {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.token
    }
}

impl AsRef<str> for BearerToken {
    fn as_ref(&self) -> &str {
        &self.token
    }
}

/// Error when bearer token extraction fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BearerTokenError {
    /// The Authorization header is missing.
    MissingHeader,
    /// The Authorization header doesn't use the Bearer scheme.
    InvalidScheme,
    /// The token is empty after the "Bearer " prefix.
    EmptyToken,
}

impl BearerTokenError {
    /// Create a missing header error.
    #[must_use]
    pub fn missing_header() -> Self {
        Self::MissingHeader
    }

    /// Create an invalid scheme error.
    #[must_use]
    pub fn invalid_scheme() -> Self {
        Self::InvalidScheme
    }

    /// Create an empty token error.
    #[must_use]
    pub fn empty_token() -> Self {
        Self::EmptyToken
    }

    /// Get a human-readable description of this error.
    #[must_use]
    pub fn detail(&self) -> &'static str {
        match self {
            Self::MissingHeader => "Not authenticated",
            Self::InvalidScheme => "Invalid authentication credentials",
            Self::EmptyToken => "Invalid authentication credentials",
        }
    }
}

impl fmt::Display for BearerTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader => write!(f, "Missing Authorization header"),
            Self::InvalidScheme => write!(f, "Authorization header must use Bearer scheme"),
            Self::EmptyToken => write!(f, "Bearer token is empty"),
        }
    }
}

impl std::error::Error for BearerTokenError {}

impl IntoResponse for BearerTokenError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let body = serde_json::json!({
            "detail": self.detail()
        });

        Response::with_status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", b"Bearer".to_vec())
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for BearerToken {
    type Error = BearerTokenError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get the Authorization header
        let auth_header = req
            .headers()
            .get("authorization")
            .ok_or(BearerTokenError::MissingHeader)?;

        // Convert to string (invalid UTF-8 is treated as invalid scheme)
        let auth_str =
            std::str::from_utf8(auth_header).map_err(|_| BearerTokenError::InvalidScheme)?;

        // Check for "Bearer " prefix (case-sensitive per RFC 6750, but we allow lowercase)
        const BEARER_PREFIX: &str = "Bearer ";
        const BEARER_PREFIX_LOWER: &str = "bearer ";

        let token = if auth_str.starts_with(BEARER_PREFIX) {
            &auth_str[BEARER_PREFIX.len()..]
        } else if auth_str.starts_with(BEARER_PREFIX_LOWER) {
            &auth_str[BEARER_PREFIX_LOWER.len()..]
        } else {
            return Err(BearerTokenError::InvalidScheme);
        };

        // Trim whitespace and check for empty token
        let token = token.trim();
        if token.is_empty() {
            return Err(BearerTokenError::EmptyToken);
        }

        Ok(BearerToken::new(token))
    }
}

// ============================================================================
// API Key Header Extractor
// ============================================================================

/// Default header name for API key extraction.
pub const DEFAULT_API_KEY_HEADER: &str = "x-api-key";

/// Configuration for API key header extraction.
#[derive(Debug, Clone)]
pub struct ApiKeyHeaderConfig {
    /// Header name to extract API key from (case-insensitive).
    header_name: String,
}

impl Default for ApiKeyHeaderConfig {
    fn default() -> Self {
        Self {
            header_name: DEFAULT_API_KEY_HEADER.to_string(),
        }
    }
}

impl ApiKeyHeaderConfig {
    /// Create a new configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the header name to extract API key from.
    #[must_use]
    pub fn header_name(mut self, name: impl Into<String>) -> Self {
        self.header_name = name.into();
        self
    }

    /// Get the configured header name.
    #[must_use]
    pub fn get_header_name(&self) -> &str {
        &self.header_name
    }
}

/// API key extracted from a request header.
///
/// Extracts an API key from a configurable header (default: `X-API-Key`).
/// Returns 401 Unauthorized if the header is missing or empty.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::ApiKeyHeader;
///
/// async fn protected_route(api_key: ApiKeyHeader) -> impl IntoResponse {
///     // Validate the API key against your database/config
///     if is_valid_key(api_key.key()) {
///         "Access granted".to_string()
///     } else {
///         // Return error response
///     }
/// }
/// ```
///
/// # Custom Header Name
///
/// Configure a custom header name by adding `ApiKeyHeaderConfig` to request extensions:
///
/// ```ignore
/// // In middleware or app setup:
/// req.insert_extension(ApiKeyHeaderConfig::new().header_name("Authorization"));
/// ```
///
/// # OpenAPI Security Scheme
///
/// This generates the following OpenAPI security scheme:
/// ```yaml
/// securitySchemes:
///   ApiKeyHeader:
///     type: apiKey
///     in: header
///     name: X-API-Key
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyHeader {
    /// The extracted API key value.
    key: String,
    /// The header name it was extracted from.
    header_name: String,
}

impl ApiKeyHeader {
    /// Create a new ApiKeyHeader with the given key.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            header_name: DEFAULT_API_KEY_HEADER.to_string(),
        }
    }

    /// Create a new ApiKeyHeader with a custom header name.
    #[must_use]
    pub fn with_header_name(key: impl Into<String>, header_name: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            header_name: header_name.into(),
        }
    }

    /// Get the API key value.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Get the header name the key was extracted from.
    #[must_use]
    pub fn header_name(&self) -> &str {
        &self.header_name
    }

    /// Consume self and return the key string.
    #[must_use]
    pub fn into_key(self) -> String {
        self.key
    }
}

impl Deref for ApiKeyHeader {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.key
    }
}

impl AsRef<str> for ApiKeyHeader {
    fn as_ref(&self) -> &str {
        &self.key
    }
}

/// Implement SecureCompare for timing-safe API key validation.
impl SecureCompare for ApiKeyHeader {
    fn secure_eq(&self, other: &str) -> bool {
        constant_time_str_eq(&self.key, other)
    }

    fn secure_eq_bytes(&self, other: &[u8]) -> bool {
        constant_time_eq(self.key.as_bytes(), other)
    }
}

/// Error returned when API key header extraction fails.
#[derive(Debug, Clone)]
pub enum ApiKeyHeaderError {
    /// The API key header is missing.
    MissingHeader {
        /// Name of the expected header.
        header_name: String,
    },
    /// The API key header is empty.
    EmptyKey {
        /// Name of the header.
        header_name: String,
    },
    /// The header value is not valid UTF-8.
    InvalidUtf8 {
        /// Name of the header.
        header_name: String,
    },
}

impl ApiKeyHeaderError {
    /// Create a missing header error.
    #[must_use]
    pub fn missing_header(header_name: impl Into<String>) -> Self {
        Self::MissingHeader {
            header_name: header_name.into(),
        }
    }

    /// Create an empty key error.
    #[must_use]
    pub fn empty_key(header_name: impl Into<String>) -> Self {
        Self::EmptyKey {
            header_name: header_name.into(),
        }
    }

    /// Create an invalid UTF-8 error.
    #[must_use]
    pub fn invalid_utf8(header_name: impl Into<String>) -> Self {
        Self::InvalidUtf8 {
            header_name: header_name.into(),
        }
    }

    /// Get a human-readable description of this error.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::MissingHeader { header_name } => {
                format!("Missing required header: {header_name}")
            }
            Self::EmptyKey { header_name } => {
                format!("Empty API key in header: {header_name}")
            }
            Self::InvalidUtf8 { header_name } => {
                format!("Invalid API key encoding in header: {header_name}")
            }
        }
    }
}

impl fmt::Display for ApiKeyHeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader { header_name } => {
                write!(f, "Missing API key header: {header_name}")
            }
            Self::EmptyKey { header_name } => {
                write!(f, "Empty API key in header: {header_name}")
            }
            Self::InvalidUtf8 { header_name } => {
                write!(f, "Invalid UTF-8 in header: {header_name}")
            }
        }
    }
}

impl std::error::Error for ApiKeyHeaderError {}

impl IntoResponse for ApiKeyHeaderError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let body = serde_json::json!({
            "detail": self.detail()
        });

        Response::with_status(StatusCode::UNAUTHORIZED)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for ApiKeyHeader {
    type Error = ApiKeyHeaderError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get config from request extensions or use default
        let header_name = req.get_extension::<ApiKeyHeaderConfig>().map_or_else(
            || DEFAULT_API_KEY_HEADER.to_string(),
            |c| c.get_header_name().to_string(),
        );

        // Get the API key header (case-insensitive lookup)
        let key_header = req
            .headers()
            .get(&header_name)
            .ok_or_else(|| ApiKeyHeaderError::missing_header(&header_name))?;

        // Convert to string
        let key_str = std::str::from_utf8(key_header)
            .map_err(|_| ApiKeyHeaderError::invalid_utf8(&header_name))?;

        // Trim whitespace and check for empty key
        let key = key_str.trim();
        if key.is_empty() {
            return Err(ApiKeyHeaderError::empty_key(&header_name));
        }

        Ok(ApiKeyHeader::with_header_name(key, header_name))
    }
}

// ============================================================================
// API Key Query Parameter Extractor
// ============================================================================

/// Default query parameter name for API key extraction.
pub const DEFAULT_API_KEY_QUERY_PARAM: &str = "api_key";

/// Configuration for API key query parameter extraction.
#[derive(Debug, Clone)]
pub struct ApiKeyQueryConfig {
    /// Query parameter name to extract API key from.
    param_name: String,
}

impl Default for ApiKeyQueryConfig {
    fn default() -> Self {
        Self {
            param_name: DEFAULT_API_KEY_QUERY_PARAM.to_string(),
        }
    }
}

impl ApiKeyQueryConfig {
    /// Create a new configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the query parameter name to extract the API key from.
    #[must_use]
    pub fn param_name(mut self, name: impl Into<String>) -> Self {
        self.param_name = name.into();
        self
    }

    /// Get the configured parameter name.
    #[must_use]
    pub fn get_param_name(&self) -> &str {
        &self.param_name
    }
}

/// Extracts an API key from a query parameter.
///
/// This extractor pulls the API key from a configurable query parameter
/// (default: `api_key`). Returns 401 Unauthorized if missing or empty.
///
/// # Security Warning
///
/// Query parameter API keys are **less secure** than header-based keys:
/// - They appear in URL logs (browser history, server logs, proxies)
/// - They can leak via the Referer header
/// - They may be cached by browsers and intermediate caches
///
/// Use [`ApiKeyHeader`] for production-grade API key authentication.
/// Query parameter keys are primarily useful for:
/// - Quick testing/debugging
/// - Webhook callbacks where headers aren't controllable
/// - Legacy API compatibility
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::ApiKeyQuery;
///
/// async fn webhook_handler(api_key: ApiKeyQuery) -> impl IntoResponse {
///     // Validate the API key
///     if api_key.key() == expected_key {
///         "Webhook received"
///     } else {
///         "Invalid API key"
///     }
/// }
/// ```
///
/// # Custom Parameter Name
///
/// Configure a custom parameter name by adding `ApiKeyQueryConfig` to request extensions:
///
/// ```ignore
/// // In middleware or app setup:
/// req.insert_extension(ApiKeyQueryConfig::new().param_name("token"));
/// // Then ?token=xxx will be used instead of ?api_key=xxx
/// ```
///
/// # OpenAPI Security Scheme
///
/// This generates the following OpenAPI security scheme:
/// ```yaml
/// securitySchemes:
///   ApiKeyQuery:
///     type: apiKey
///     in: query
///     name: api_key
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyQuery {
    /// The extracted API key value.
    key: String,
    /// The parameter name it was extracted from.
    param_name: String,
}

impl ApiKeyQuery {
    /// Create a new ApiKeyQuery with the given key.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            param_name: DEFAULT_API_KEY_QUERY_PARAM.to_string(),
        }
    }

    /// Create a new ApiKeyQuery with a custom parameter name.
    #[must_use]
    pub fn with_param_name(key: impl Into<String>, param_name: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            param_name: param_name.into(),
        }
    }

    /// Get the API key value.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Get the parameter name it was extracted from.
    #[must_use]
    pub fn param_name(&self) -> &str {
        &self.param_name
    }

    /// Consume and return the key value.
    #[must_use]
    pub fn into_key(self) -> String {
        self.key
    }
}

impl Deref for ApiKeyQuery {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.key
    }
}

impl AsRef<str> for ApiKeyQuery {
    fn as_ref(&self) -> &str {
        &self.key
    }
}

/// Implement SecureCompare for timing-safe API key validation.
impl SecureCompare for ApiKeyQuery {
    fn secure_eq(&self, other: &str) -> bool {
        constant_time_str_eq(&self.key, other)
    }

    fn secure_eq_bytes(&self, other: &[u8]) -> bool {
        constant_time_eq(self.key.as_bytes(), other)
    }
}

/// Error returned when API key query parameter extraction fails.
#[derive(Debug, Clone)]
pub enum ApiKeyQueryError {
    /// The API key query parameter is missing.
    MissingParam {
        /// Name of the expected parameter.
        param_name: String,
    },
    /// The API key parameter is present but empty.
    EmptyKey {
        /// Name of the parameter.
        param_name: String,
    },
}

impl ApiKeyQueryError {
    /// Create a missing parameter error.
    #[must_use]
    pub fn missing_param(param_name: impl Into<String>) -> Self {
        Self::MissingParam {
            param_name: param_name.into(),
        }
    }

    /// Create an empty key error.
    #[must_use]
    pub fn empty_key(param_name: impl Into<String>) -> Self {
        Self::EmptyKey {
            param_name: param_name.into(),
        }
    }

    /// Get the detail message for error responses.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::MissingParam { param_name } => {
                format!("API key required. Include '{param_name}' query parameter.")
            }
            Self::EmptyKey { param_name } => {
                format!("API key cannot be empty. Provide a value for '{param_name}'.")
            }
        }
    }
}

impl fmt::Display for ApiKeyQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingParam { param_name } => {
                write!(f, "Missing API key query parameter: {param_name}")
            }
            Self::EmptyKey { param_name } => {
                write!(f, "Empty API key in query parameter: {param_name}")
            }
        }
    }
}

impl std::error::Error for ApiKeyQueryError {}

impl IntoResponse for ApiKeyQueryError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let body = serde_json::json!({
            "detail": self.detail()
        });

        Response::with_status(StatusCode::UNAUTHORIZED)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for ApiKeyQuery {
    type Error = ApiKeyQueryError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get config from request extensions or use default
        let param_name = req.get_extension::<ApiKeyQueryConfig>().map_or_else(
            || DEFAULT_API_KEY_QUERY_PARAM.to_string(),
            |c| c.get_param_name().to_string(),
        );

        // Parse the query string if present
        let query_params = req.query().map(QueryParams::parse).unwrap_or_default();

        // Get the API key parameter
        let key_value = query_params
            .get(&param_name)
            .ok_or_else(|| ApiKeyQueryError::missing_param(&param_name))?;

        // Trim whitespace and check for empty key
        let key = key_value.trim();
        if key.is_empty() {
            return Err(ApiKeyQueryError::empty_key(&param_name));
        }

        Ok(ApiKeyQuery::with_param_name(key, param_name))
    }
}

// ============================================================================
// API Key Cookie Extractor
// ============================================================================

/// Default cookie name for API key extraction.
pub const DEFAULT_API_KEY_COOKIE: &str = "api_key";

/// Configuration for API key cookie extraction.
#[derive(Debug, Clone)]
pub struct ApiKeyCookieConfig {
    /// Cookie name to extract API key from.
    cookie_name: String,
}

impl Default for ApiKeyCookieConfig {
    fn default() -> Self {
        Self {
            cookie_name: DEFAULT_API_KEY_COOKIE.to_string(),
        }
    }
}

impl ApiKeyCookieConfig {
    /// Create a new configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the cookie name to extract the API key from.
    #[must_use]
    pub fn cookie_name(mut self, name: impl Into<String>) -> Self {
        self.cookie_name = name.into();
        self
    }

    /// Get the configured cookie name.
    #[must_use]
    pub fn get_cookie_name(&self) -> &str {
        &self.cookie_name
    }
}

/// Extracts an API key from a cookie.
///
/// This extractor pulls the API key from a configurable cookie
/// (default: `api_key`). Returns 401 Unauthorized if missing or empty.
///
/// # Security Considerations
///
/// Cookie-based API keys have different security characteristics than headers:
/// - Automatically sent by browsers (enables browser-based API access)
/// - Subject to CSRF attacks (use with CSRF protection middleware)
/// - Can be marked `HttpOnly` to prevent JavaScript access
/// - Can be marked `Secure` to require HTTPS
///
/// For browser-based APIs, consider pairing with CSRF protection.
/// For programmatic API access, prefer [`ApiKeyHeader`].
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::ApiKeyCookie;
///
/// async fn protected_endpoint(api_key: ApiKeyCookie) -> impl IntoResponse {
///     // Validate the API key
///     if api_key.secure_eq(expected_key) {
///         "Access granted"
///     } else {
///         "Invalid API key"
///     }
/// }
/// ```
///
/// # Custom Cookie Name
///
/// Configure a custom cookie name by adding `ApiKeyCookieConfig` to request extensions:
///
/// ```ignore
/// // In middleware or app setup:
/// req.insert_extension(ApiKeyCookieConfig::new().cookie_name("auth_token"));
/// // Then the auth_token cookie will be used
/// ```
///
/// # OpenAPI Security Scheme
///
/// This generates the following OpenAPI security scheme:
/// ```yaml
/// securitySchemes:
///   ApiKeyCookie:
///     type: apiKey
///     in: cookie
///     name: api_key
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyCookie {
    /// The extracted API key value.
    key: String,
    /// The cookie name it was extracted from.
    cookie_name: String,
}

impl ApiKeyCookie {
    /// Create a new ApiKeyCookie with the given key.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            cookie_name: DEFAULT_API_KEY_COOKIE.to_string(),
        }
    }

    /// Create a new ApiKeyCookie with a custom cookie name.
    #[must_use]
    pub fn with_cookie_name(key: impl Into<String>, cookie_name: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            cookie_name: cookie_name.into(),
        }
    }

    /// Get the API key value.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Get the cookie name it was extracted from.
    #[must_use]
    pub fn cookie_name(&self) -> &str {
        &self.cookie_name
    }

    /// Consume and return the key value.
    #[must_use]
    pub fn into_key(self) -> String {
        self.key
    }
}

impl Deref for ApiKeyCookie {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.key
    }
}

impl AsRef<str> for ApiKeyCookie {
    fn as_ref(&self) -> &str {
        &self.key
    }
}

/// Implement SecureCompare for timing-safe API key validation.
impl SecureCompare for ApiKeyCookie {
    fn secure_eq(&self, other: &str) -> bool {
        constant_time_str_eq(&self.key, other)
    }

    fn secure_eq_bytes(&self, other: &[u8]) -> bool {
        constant_time_eq(self.key.as_bytes(), other)
    }
}

/// Error returned when API key cookie extraction fails.
#[derive(Debug, Clone)]
pub enum ApiKeyCookieError {
    /// The API key cookie is missing.
    MissingCookie {
        /// Name of the expected cookie.
        cookie_name: String,
    },
    /// The API key cookie is present but empty.
    EmptyKey {
        /// Name of the cookie.
        cookie_name: String,
    },
}

impl ApiKeyCookieError {
    /// Create a missing cookie error.
    #[must_use]
    pub fn missing_cookie(cookie_name: impl Into<String>) -> Self {
        Self::MissingCookie {
            cookie_name: cookie_name.into(),
        }
    }

    /// Create an empty key error.
    #[must_use]
    pub fn empty_key(cookie_name: impl Into<String>) -> Self {
        Self::EmptyKey {
            cookie_name: cookie_name.into(),
        }
    }

    /// Get the detail message for error responses.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::MissingCookie { cookie_name } => {
                format!("API key required. Include '{cookie_name}' cookie.")
            }
            Self::EmptyKey { cookie_name } => {
                format!("API key cannot be empty. Provide a value for '{cookie_name}' cookie.")
            }
        }
    }
}

impl fmt::Display for ApiKeyCookieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCookie { cookie_name } => {
                write!(f, "Missing API key cookie: {cookie_name}")
            }
            Self::EmptyKey { cookie_name } => {
                write!(f, "Empty API key in cookie: {cookie_name}")
            }
        }
    }
}

impl std::error::Error for ApiKeyCookieError {}

impl IntoResponse for ApiKeyCookieError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let body = serde_json::json!({
            "detail": self.detail()
        });

        Response::with_status(StatusCode::UNAUTHORIZED)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for ApiKeyCookie {
    type Error = ApiKeyCookieError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get config from request extensions or use default
        let cookie_name = req.get_extension::<ApiKeyCookieConfig>().map_or_else(
            || DEFAULT_API_KEY_COOKIE.to_string(),
            |c| c.get_cookie_name().to_string(),
        );

        // Parse cookies from the Cookie header
        let cookies = req
            .headers()
            .get("cookie")
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(RequestCookies::from_header)
            .unwrap_or_default();

        // Get the API key cookie
        let key_value = cookies
            .get(&cookie_name)
            .ok_or_else(|| ApiKeyCookieError::missing_cookie(&cookie_name))?;

        // Trim whitespace and check for empty key
        let key = key_value.trim();
        if key.is_empty() {
            return Err(ApiKeyCookieError::empty_key(&cookie_name));
        }

        Ok(ApiKeyCookie::with_cookie_name(key, cookie_name))
    }
}

// ============================================================================
// Basic Authentication Extractor
// ============================================================================

/// Extracts HTTP Basic authentication credentials from the `Authorization` header.
///
/// This implements the HTTP Basic authentication scheme as defined in RFC 7617.
/// The Authorization header should contain `Basic <base64(username:password)>`.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::BasicAuth;
///
/// async fn protected_route(auth: BasicAuth) -> impl IntoResponse {
///     format!("Hello, {}!", auth.username())
/// }
/// ```
///
/// # Error Handling
///
/// When credentials are missing or invalid, a 401 Unauthorized response is returned
/// with a `WWW-Authenticate: Basic` header, following RFC 7617.
///
/// # Optional Extraction
///
/// Wrap in `Option` to make authentication optional:
///
/// ```ignore
/// async fn maybe_auth(auth: Option<BasicAuth>) -> impl IntoResponse {
///     match auth {
///         Some(a) => format!("Hello, {}!", a.username()),
///         None => "Anonymous access".to_string(),
///     }
/// }
/// ```
///
/// # OpenAPI
///
/// This extractor generates the following OpenAPI security scheme:
/// ```yaml
/// securitySchemes:
///   BasicAuth:
///     type: http
///     scheme: basic
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicAuth {
    /// The username extracted from the credentials.
    username: String,
    /// The password extracted from the credentials.
    password: String,
}

impl BasicAuth {
    /// Create a new BasicAuth with the given username and password.
    #[must_use]
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Get the username.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Get the password.
    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Consume self and return the username and password as a tuple.
    #[must_use]
    pub fn into_credentials(self) -> (String, String) {
        (self.username, self.password)
    }
}

/// Error when basic auth extraction fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasicAuthError {
    /// The Authorization header is missing.
    MissingHeader,
    /// The Authorization header doesn't use the Basic scheme.
    InvalidScheme,
    /// The credentials are not valid base64.
    InvalidBase64,
    /// The decoded credentials don't contain a colon separator.
    MissingColon,
    /// The header value is not valid UTF-8.
    InvalidUtf8,
}

impl BasicAuthError {
    /// Create a missing header error.
    #[must_use]
    pub fn missing_header() -> Self {
        Self::MissingHeader
    }

    /// Create an invalid scheme error.
    #[must_use]
    pub fn invalid_scheme() -> Self {
        Self::InvalidScheme
    }

    /// Create an invalid base64 error.
    #[must_use]
    pub fn invalid_base64() -> Self {
        Self::InvalidBase64
    }

    /// Create a missing colon error.
    #[must_use]
    pub fn missing_colon() -> Self {
        Self::MissingColon
    }

    /// Create an invalid UTF-8 error.
    #[must_use]
    pub fn invalid_utf8() -> Self {
        Self::InvalidUtf8
    }

    /// Get a human-readable description of this error.
    #[must_use]
    pub fn detail(&self) -> &'static str {
        match self {
            Self::MissingHeader => "Not authenticated",
            Self::InvalidScheme => "Invalid authentication credentials",
            Self::InvalidBase64 => "Invalid authentication credentials",
            Self::MissingColon => "Invalid authentication credentials",
            Self::InvalidUtf8 => "Invalid authentication credentials",
        }
    }
}

impl fmt::Display for BasicAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHeader => write!(f, "Missing Authorization header"),
            Self::InvalidScheme => write!(f, "Authorization header must use Basic scheme"),
            Self::InvalidBase64 => write!(f, "Invalid base64 encoding in credentials"),
            Self::MissingColon => write!(f, "Credentials must contain username:password"),
            Self::InvalidUtf8 => write!(f, "Credentials contain invalid UTF-8"),
        }
    }
}

impl std::error::Error for BasicAuthError {}

impl IntoResponse for BasicAuthError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let body = serde_json::json!({
            "detail": self.detail()
        });

        Response::with_status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", b"Basic".to_vec())
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

/// Decode a base64 string to bytes.
///
/// This is a minimal implementation for Basic auth credential decoding.
/// Supports standard base64 alphabet (A-Za-z0-9+/) with optional padding.
fn decode_base64(input: &str) -> Result<Vec<u8>, BasicAuthError> {
    const INVALID: u8 = 0xFF;
    const DECODE_TABLE: [u8; 256] = {
        let mut table = [INVALID; 256];
        let mut i = 0u8;
        // A-Z = 0-25
        while i < 26 {
            table[(b'A' + i) as usize] = i;
            i += 1;
        }
        // a-z = 26-51
        i = 0;
        while i < 26 {
            table[(b'a' + i) as usize] = 26 + i;
            i += 1;
        }
        // 0-9 = 52-61
        i = 0;
        while i < 10 {
            table[(b'0' + i) as usize] = 52 + i;
            i += 1;
        }
        // + = 62, / = 63
        table[b'+' as usize] = 62;
        table[b'/' as usize] = 63;
        table
    };

    // Remove padding and whitespace
    let input = input.trim_end_matches('=').trim();
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let mut output = Vec::with_capacity((input.len() * 3) / 4);
    let mut buffer: u32 = 0;
    let mut bits_collected: u8 = 0;

    for byte in input.bytes() {
        let value = DECODE_TABLE[byte as usize];
        if value == INVALID {
            return Err(BasicAuthError::InvalidBase64);
        }

        buffer = (buffer << 6) | u32::from(value);
        bits_collected += 6;

        if bits_collected >= 8 {
            bits_collected -= 8;
            output.push((buffer >> bits_collected) as u8);
            buffer &= (1 << bits_collected) - 1;
        }
    }

    Ok(output)
}

impl FromRequest for BasicAuth {
    type Error = BasicAuthError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get the Authorization header
        let auth_header = req
            .headers()
            .get("authorization")
            .ok_or(BasicAuthError::MissingHeader)?;

        // Convert to string
        let auth_str = std::str::from_utf8(auth_header).map_err(|_| BasicAuthError::InvalidUtf8)?;

        // Check for "Basic " prefix (case-insensitive per RFC 7617)
        const BASIC_PREFIX: &str = "Basic ";
        const BASIC_PREFIX_LOWER: &str = "basic ";

        let encoded = if auth_str.starts_with(BASIC_PREFIX) {
            &auth_str[BASIC_PREFIX.len()..]
        } else if auth_str.starts_with(BASIC_PREFIX_LOWER) {
            &auth_str[BASIC_PREFIX_LOWER.len()..]
        } else {
            return Err(BasicAuthError::InvalidScheme);
        };

        // Decode base64
        let decoded_bytes = decode_base64(encoded.trim())?;

        // Convert to UTF-8 string
        let decoded = String::from_utf8(decoded_bytes).map_err(|_| BasicAuthError::InvalidUtf8)?;

        // Split on first colon (password may contain colons)
        let colon_pos = decoded.find(':').ok_or(BasicAuthError::MissingColon)?;
        let (username, password_with_colon) = decoded.split_at(colon_pos);
        let password = &password_with_colon[1..]; // Skip the colon

        Ok(BasicAuth::new(username, password))
    }
}

// ============================================================================
// Timing-Safe Comparison Utilities
// ============================================================================

/// Performs constant-time comparison of two byte slices.
///
/// This function compares two byte slices in a way that takes the same amount
/// of time regardless of where (or if) the slices differ. This prevents
/// [timing attacks](https://en.wikipedia.org/wiki/Timing_attack) where an
/// attacker can deduce secret values by measuring comparison time.
///
/// # Security Properties
///
/// - **Constant time**: Always iterates through all bytes regardless of mismatches
/// - **No early return**: Uses bitwise OR to accumulate differences
/// - **Length-safe**: Returns false for different lengths (but length itself may leak)
///
/// # Timing Attack Prevention
///
/// A naive comparison like `a == b` returns as soon as it finds a difference:
/// - `"secret" == "aaaaaa"` returns immediately (first byte differs)
/// - `"secret" == "saaaaa"` takes slightly longer (second byte differs)
/// - `"secret" == "seaaaa"` takes even longer, etc.
///
/// An attacker can exploit this to guess a secret character-by-character.
/// This function prevents that by always examining all bytes.
///
/// # Warning: Length Leakage
///
/// While the comparison itself is constant-time, this function returns `false`
/// immediately if the lengths differ. This is intentional for most use cases,
/// but be aware that an attacker may be able to determine the length of secret
/// values. For HMAC comparison, this is typically acceptable since HMACs have
/// fixed, known lengths.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::constant_time_eq;
///
/// let secret_token = b"supersecrettoken12345";
/// let user_input = b"supersecrettoken12345";
///
/// if constant_time_eq(secret_token, user_input) {
///     // Tokens match - grant access
/// } else {
///     // Tokens don't match - deny access
/// }
/// ```
///
/// # When to Use
///
/// Use this function when comparing:
/// - Authentication tokens
/// - API keys
/// - Session IDs
/// - HMAC signatures
/// - Password hashes (after hashing)
/// - Any secret value where timing attacks are a concern
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Length check - this does leak length information, but for most auth
    // scenarios (HMAC, tokens) the length is known/fixed
    if a.len() != b.len() {
        return false;
    }

    // Accumulate XOR of all byte pairs. Any difference sets bits in `diff`.
    // This always processes all bytes regardless of where differences occur.
    let diff = a
        .iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y));

    // diff == 0 only if all bytes matched
    diff == 0
}

/// Performs constant-time comparison of two strings.
///
/// This is a convenience wrapper around [`constant_time_eq`] that works with
/// string slices. Internally, it compares the UTF-8 byte representations.
///
/// See [`constant_time_eq`] for full documentation on timing attack prevention.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::constant_time_str_eq;
///
/// let stored_token = "user_api_key_xyz123";
/// let provided_token = get_token_from_header();
///
/// if constant_time_str_eq(stored_token, &provided_token) {
///     // Valid token
/// }
/// ```
#[must_use]
#[inline]
pub fn constant_time_str_eq(a: &str, b: &str) -> bool {
    constant_time_eq(a.as_bytes(), b.as_bytes())
}

/// Extension trait for constant-time equality comparison on `BearerToken`.
///
/// Provides a method to securely compare bearer tokens without leaking
/// timing information about where tokens differ.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{BearerToken, SecureCompare};
///
/// async fn validate_token(token: BearerToken) -> bool {
///     let expected = "valid_api_key_12345";
///     token.secure_eq(expected)
/// }
/// ```
pub trait SecureCompare {
    /// Compares this value with another using constant-time comparison.
    ///
    /// Returns `true` if the values are equal, `false` otherwise.
    /// The comparison time is independent of where (or if) the values differ.
    fn secure_eq(&self, other: &str) -> bool;

    /// Compares this value with a byte slice using constant-time comparison.
    fn secure_eq_bytes(&self, other: &[u8]) -> bool;
}

impl SecureCompare for BearerToken {
    #[inline]
    fn secure_eq(&self, other: &str) -> bool {
        constant_time_str_eq(self.token(), other)
    }

    #[inline]
    fn secure_eq_bytes(&self, other: &[u8]) -> bool {
        constant_time_eq(self.token().as_bytes(), other)
    }
}

impl SecureCompare for str {
    #[inline]
    fn secure_eq(&self, other: &str) -> bool {
        constant_time_str_eq(self, other)
    }

    #[inline]
    fn secure_eq_bytes(&self, other: &[u8]) -> bool {
        constant_time_eq(self.as_bytes(), other)
    }
}

impl SecureCompare for String {
    #[inline]
    fn secure_eq(&self, other: &str) -> bool {
        constant_time_str_eq(self, other)
    }

    #[inline]
    fn secure_eq_bytes(&self, other: &[u8]) -> bool {
        constant_time_eq(self.as_bytes(), other)
    }
}

impl SecureCompare for [u8] {
    #[inline]
    fn secure_eq(&self, other: &str) -> bool {
        constant_time_eq(self, other.as_bytes())
    }

    #[inline]
    fn secure_eq_bytes(&self, other: &[u8]) -> bool {
        constant_time_eq(self, other)
    }
}

impl<const N: usize> SecureCompare for [u8; N] {
    #[inline]
    fn secure_eq(&self, other: &str) -> bool {
        constant_time_eq(self, other.as_bytes())
    }

    #[inline]
    fn secure_eq_bytes(&self, other: &[u8]) -> bool {
        constant_time_eq(self, other)
    }
}

impl SecureCompare for Vec<u8> {
    #[inline]
    fn secure_eq(&self, other: &str) -> bool {
        constant_time_eq(self, other.as_bytes())
    }

    #[inline]
    fn secure_eq_bytes(&self, other: &[u8]) -> bool {
        constant_time_eq(self, other)
    }
}

// ============================================================================
// Pagination Extractor and Response
// ============================================================================

/// Default page number (1-indexed).
pub const DEFAULT_PAGE: u64 = 1;
/// Default items per page.
pub const DEFAULT_PER_PAGE: u64 = 20;
/// Maximum items per page (to prevent abuse).
pub const MAX_PER_PAGE: u64 = 100;

/// Pagination query parameters extractor.
///
/// Extracts common pagination parameters from the query string:
/// - `page`: Current page number (1-indexed, default: 1)
/// - `per_page` or `limit`: Items per page (default: 20, max: 100)
/// - `offset`: Alternative to page-based pagination (overrides page if set)
///
/// # Example
///
/// ```ignore
/// use fastapi_core::Pagination;
///
/// #[get("/items")]
/// async fn list_items(cx: &Cx, pagination: Pagination) -> impl IntoResponse {
///     let offset = pagination.offset();
///     let limit = pagination.limit();
///
///     // Fetch items from database with offset and limit
///     let items = db.fetch_items(offset, limit).await;
///
///     // Return paginated response
///     pagination.paginate(items, total_count, "/items")
/// }
/// ```
///
/// # Query String Formats
///
/// ```text
/// # Page-based (preferred)
/// ?page=2&per_page=10
///
/// # Using limit alias
/// ?page=2&limit=10
///
/// # Offset-based (for cursor-style pagination)
/// ?offset=20&limit=10
/// ```
///
/// # Configuration
///
/// Use [`PaginationConfig`] to customize defaults and limits:
///
/// ```ignore
/// use fastapi_core::{Pagination, PaginationConfig};
///
/// let config = PaginationConfig::new()
///     .default_per_page(50)
///     .max_per_page(200);
///
/// // The config can be stored in app state and used by handlers
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    /// Current page (1-indexed).
    page: u64,
    /// Items per page.
    per_page: u64,
    /// Explicit offset (overrides page calculation if set).
    offset: Option<u64>,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            page: DEFAULT_PAGE,
            per_page: DEFAULT_PER_PAGE,
            offset: None,
        }
    }
}

impl Pagination {
    /// Create pagination with specific values.
    #[must_use]
    pub fn new(page: u64, per_page: u64) -> Self {
        Self {
            page: page.max(1),
            per_page: per_page.clamp(1, MAX_PER_PAGE),
            offset: None,
        }
    }

    /// Create pagination from offset and limit.
    #[must_use]
    pub fn from_offset(offset: u64, limit: u64) -> Self {
        Self {
            page: (offset / limit.max(1)) + 1,
            per_page: limit.clamp(1, MAX_PER_PAGE),
            offset: Some(offset),
        }
    }

    /// Get the current page number (1-indexed).
    #[must_use]
    pub fn page(&self) -> u64 {
        self.page
    }

    /// Get the number of items per page.
    #[must_use]
    pub fn per_page(&self) -> u64 {
        self.per_page
    }

    /// Alias for `per_page()` - returns the page size limit.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.per_page
    }

    /// Calculate the offset for database queries.
    ///
    /// If an explicit offset was provided, returns that.
    /// Otherwise, calculates from page number: `(page - 1) * per_page`.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
            .unwrap_or_else(|| (self.page.saturating_sub(1)) * self.per_page)
    }

    /// Calculate total number of pages given a total item count.
    #[must_use]
    pub fn total_pages(&self, total_items: u64) -> u64 {
        if self.per_page == 0 {
            return 0;
        }
        total_items.div_ceil(self.per_page)
    }

    /// Check if there is a next page.
    #[must_use]
    pub fn has_next(&self, total_items: u64) -> bool {
        self.page < self.total_pages(total_items)
    }

    /// Check if there is a previous page.
    #[must_use]
    pub fn has_prev(&self) -> bool {
        self.page > 1
    }

    /// Create a paginated response from items.
    ///
    /// # Arguments
    ///
    /// * `items` - The items for the current page
    /// * `total` - Total number of items across all pages
    /// * `base_url` - Base URL for generating Link headers (e.g., "/api/items")
    #[must_use]
    pub fn paginate<T>(self, items: Vec<T>, total: u64, base_url: &str) -> Page<T> {
        Page::new(items, total, self, base_url.to_string())
    }
}

impl FromRequest for Pagination {
    type Error = std::convert::Infallible;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let query = req
            .get_extension::<QueryParams>()
            .cloned()
            .unwrap_or_default();

        // Parse page (1-indexed, default 1)
        let page = query
            .get("page")
            .and_then(|v: &str| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_PAGE)
            .max(1);

        // Parse per_page or limit (default 20, max 100)
        let per_page = query
            .get("per_page")
            .or_else(|| query.get("limit"))
            .and_then(|v: &str| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_PER_PAGE)
            .clamp(1, MAX_PER_PAGE);

        // Parse offset (optional, overrides page if present)
        let offset = query
            .get("offset")
            .and_then(|v: &str| v.parse::<u64>().ok());

        Ok(Pagination {
            page,
            per_page,
            offset,
        })
    }
}

/// Configuration for pagination behavior.
///
/// Use this to customize default values and limits for pagination.
#[derive(Debug, Clone, Copy)]
pub struct PaginationConfig {
    /// Default items per page when not specified.
    pub default_per_page: u64,
    /// Maximum allowed items per page.
    pub max_per_page: u64,
    /// Default page number (usually 1).
    pub default_page: u64,
}

impl Default for PaginationConfig {
    fn default() -> Self {
        Self {
            default_per_page: DEFAULT_PER_PAGE,
            max_per_page: MAX_PER_PAGE,
            default_page: DEFAULT_PAGE,
        }
    }
}

impl PaginationConfig {
    /// Create a new pagination configuration with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the default items per page.
    #[must_use]
    pub fn default_per_page(mut self, value: u64) -> Self {
        self.default_per_page = value;
        self
    }

    /// Set the maximum items per page.
    #[must_use]
    pub fn max_per_page(mut self, value: u64) -> Self {
        self.max_per_page = value;
        self
    }

    /// Set the default page number.
    #[must_use]
    pub fn default_page(mut self, value: u64) -> Self {
        self.default_page = value;
        self
    }
}

/// Paginated response wrapper.
///
/// Wraps a collection of items with pagination metadata and generates
/// RFC 5988 Link headers for navigation.
///
/// # JSON Response Format
///
/// ```json
/// {
///     "items": [...],
///     "total": 100,
///     "page": 2,
///     "per_page": 20,
///     "pages": 5
/// }
/// ```
///
/// # Link Headers
///
/// When converted to a response, includes RFC 5988 Link headers:
///
/// ```text
/// Link: </items?page=1&per_page=20>; rel="first",
///       </items?page=1&per_page=20>; rel="prev",
///       </items?page=3&per_page=20>; rel="next",
///       </items?page=5&per_page=20>; rel="last"
/// ```
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{Pagination, Page};
///
/// #[get("/users")]
/// async fn list_users(cx: &Cx, pagination: Pagination) -> impl IntoResponse {
///     let users = fetch_users(pagination.offset(), pagination.limit()).await;
///     let total = count_users().await;
///
///     pagination.paginate(users, total, "/users")
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Page<T> {
    /// Items for the current page.
    pub items: Vec<T>,
    /// Total number of items across all pages.
    pub total: u64,
    /// Current page number (1-indexed).
    pub page: u64,
    /// Items per page.
    pub per_page: u64,
    /// Total number of pages.
    pub pages: u64,
    /// Base URL for Link header generation.
    base_url: String,
}

impl<T> Page<T> {
    /// Create a new paginated response.
    #[must_use]
    pub fn new(items: Vec<T>, total: u64, pagination: Pagination, base_url: String) -> Self {
        let pages = pagination.total_pages(total);
        Self {
            items,
            total,
            page: pagination.page(),
            per_page: pagination.per_page(),
            pages,
            base_url,
        }
    }

    /// Create a page with explicit values (for testing or manual construction).
    #[must_use]
    pub fn with_values(
        items: Vec<T>,
        total: u64,
        page: u64,
        per_page: u64,
        base_url: impl Into<String>,
    ) -> Self {
        let pages = if per_page > 0 {
            total.div_ceil(per_page)
        } else {
            0
        };
        Self {
            items,
            total,
            page,
            per_page,
            pages,
            base_url: base_url.into(),
        }
    }

    /// Get the number of items on the current page.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if the page is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Check if there is a next page.
    #[must_use]
    pub fn has_next(&self) -> bool {
        self.page < self.pages
    }

    /// Check if there is a previous page.
    #[must_use]
    pub fn has_prev(&self) -> bool {
        self.page > 1
    }

    /// Generate RFC 5988 Link header value.
    ///
    /// Returns a string with Link headers for navigation:
    /// - `first`: Link to the first page
    /// - `prev`: Link to the previous page (if applicable)
    /// - `next`: Link to the next page (if applicable)
    /// - `last`: Link to the last page
    #[must_use]
    pub fn link_header(&self) -> String {
        let mut links = Vec::with_capacity(4);

        // Always include first and last
        links.push(format!(
            "<{}?page=1&per_page={}>; rel=\"first\"",
            self.base_url, self.per_page
        ));

        // Previous page (if not on first page)
        if self.has_prev() {
            links.push(format!(
                "<{}?page={}&per_page={}>; rel=\"prev\"",
                self.base_url,
                self.page - 1,
                self.per_page
            ));
        }

        // Next page (if not on last page)
        if self.has_next() {
            links.push(format!(
                "<{}?page={}&per_page={}>; rel=\"next\"",
                self.base_url,
                self.page + 1,
                self.per_page
            ));
        }

        // Last page
        links.push(format!(
            "<{}?page={}&per_page={}>; rel=\"last\"",
            self.base_url, self.pages, self.per_page
        ));

        links.join(", ")
    }

    /// Map the items using a transformation function.
    pub fn map<U, F>(self, f: F) -> Page<U>
    where
        F: FnMut(T) -> U,
    {
        Page {
            items: self.items.into_iter().map(f).collect(),
            total: self.total,
            page: self.page,
            per_page: self.per_page,
            pages: self.pages,
            base_url: self.base_url,
        }
    }
}

/// JSON representation of a paginated response.
#[derive(serde::Serialize)]
struct PageJson<'a, T: serde::Serialize> {
    items: &'a Vec<T>,
    total: u64,
    page: u64,
    per_page: u64,
    pages: u64,
}

impl<T: serde::Serialize> IntoResponse for Page<T> {
    fn into_response(self) -> crate::response::Response {
        let json_body = PageJson {
            items: &self.items,
            total: self.total,
            page: self.page,
            per_page: self.per_page,
            pages: self.pages,
        };

        // Serialize to JSON
        let Ok(body_bytes) = serde_json::to_vec(&json_body) else {
            // Fallback to empty error response on serialization failure
            return crate::response::Response::with_status(
                crate::response::StatusCode::INTERNAL_SERVER_ERROR,
            )
            .header("content-type", b"application/json".to_vec())
            .body(crate::response::ResponseBody::Bytes(
                b"{\"error\":\"Serialization failed\"}".to_vec(),
            ));
        };

        // Build response with Link header
        let link_header = self.link_header();

        crate::response::Response::ok()
            .header("content-type", b"application/json".to_vec())
            .header("link", link_header.into_bytes())
            .header("x-total-count", self.total.to_string().into_bytes())
            .header("x-page", self.page.to_string().into_bytes())
            .header("x-per-page", self.per_page.to_string().into_bytes())
            .header("x-total-pages", self.pages.to_string().into_bytes())
            .body(crate::response::ResponseBody::Bytes(body_bytes))
    }
}

/// Multiple header values extractor.
///
/// Extracts all values for a header that may appear multiple times.
#[derive(Debug, Clone)]
pub struct HeaderValues<T, N> {
    /// All extracted header values.
    pub values: Vec<T>,
    _marker: std::marker::PhantomData<N>,
}

impl<T, N> HeaderValues<T, N> {
    /// Create a new header values wrapper.
    #[must_use]
    pub fn new(values: Vec<T>) -> Self {
        Self {
            values,
            _marker: std::marker::PhantomData,
        }
    }

    /// Returns true if no values were extracted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns the number of values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }
}

impl<T, N> Deref for HeaderValues<T, N> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

// ============================================================================
// Content Negotiation
// ============================================================================

/// A parsed media type (MIME type) with optional parameters.
///
/// Represents types like `text/html`, `application/json`, or
/// `text/html; charset=utf-8`.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaType {
    /// The primary type (e.g., "text", "application", "image").
    pub typ: String,
    /// The subtype (e.g., "html", "json", "png").
    pub subtype: String,
    /// Optional parameters (e.g., charset, boundary).
    pub params: Vec<(String, String)>,
}

impl MediaType {
    /// Parse a media type string.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mt = MediaType::parse("text/html; charset=utf-8").unwrap();
    /// assert_eq!(mt.typ, "text");
    /// assert_eq!(mt.subtype, "html");
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (type_part, params_part) = match s.find(';') {
            Some(pos) => (&s[..pos], Some(&s[pos + 1..])),
            None => (s, None),
        };

        let (typ, subtype) = type_part.split_once('/')?;
        let typ = typ.trim().to_ascii_lowercase();
        let subtype = subtype.trim().to_ascii_lowercase();

        if typ.is_empty() || subtype.is_empty() {
            return None;
        }

        let mut params = Vec::new();
        if let Some(params_str) = params_part {
            for param in params_str.split(';') {
                let param = param.trim();
                if param.is_empty() {
                    continue;
                }
                if let Some((key, value)) = param.split_once('=') {
                    let key = key.trim().to_ascii_lowercase();
                    let value = value.trim().trim_matches('"').to_string();
                    // Skip the quality parameter, handled separately
                    if key != "q" {
                        params.push((key, value));
                    }
                }
            }
        }

        Some(Self {
            typ,
            subtype,
            params,
        })
    }

    /// Create a new media type without parameters.
    #[must_use]
    pub fn new(typ: impl Into<String>, subtype: impl Into<String>) -> Self {
        Self {
            typ: typ.into().to_ascii_lowercase(),
            subtype: subtype.into().to_ascii_lowercase(),
            params: Vec::new(),
        }
    }

    /// Check if this media type matches another, supporting wildcards.
    ///
    /// A wildcard `*` matches any value.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let html = MediaType::new("text", "html");
    /// let any_text = MediaType::new("text", "*");
    /// let any = MediaType::new("*", "*");
    ///
    /// assert!(html.matches(&any_text));
    /// assert!(html.matches(&any));
    /// assert!(!any_text.matches(&html)); // wildcard doesn't match specific
    /// ```
    #[must_use]
    pub fn matches(&self, other: &MediaType) -> bool {
        let type_matches = other.typ == "*" || self.typ == other.typ;
        let subtype_matches = other.subtype == "*" || self.subtype == other.subtype;
        type_matches && subtype_matches
    }

    /// Returns the media type as a string without parameters.
    #[must_use]
    pub fn essence(&self) -> String {
        format!("{}/{}", self.typ, self.subtype)
    }

    /// Get a parameter value by name.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&str> {
        let name_lower = name.to_ascii_lowercase();
        self.params
            .iter()
            .find(|(k, _)| k == &name_lower)
            .map(|(_, v)| v.as_str())
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.typ, self.subtype)?;
        for (key, value) in &self.params {
            write!(f, "; {}={}", key, value)?;
        }
        Ok(())
    }
}

/// A single entry from an Accept header with quality value.
#[derive(Debug, Clone)]
pub struct AcceptItem {
    /// The media type.
    pub media_type: MediaType,
    /// Quality value (0.0 to 1.0, default 1.0).
    pub quality: f32,
}

impl AcceptItem {
    /// Parse a single Accept header item.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let item = AcceptItem::parse("text/html;q=0.9").unwrap();
    /// assert_eq!(item.media_type.typ, "text");
    /// assert_eq!(item.quality, 0.9);
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let mut quality = 1.0f32;

        // Extract quality parameter
        let media_str = if let Some(q_pos) = s.to_ascii_lowercase().find(";q=") {
            let after_q = &s[q_pos + 3..];
            let q_end = after_q.find(';').unwrap_or(after_q.len());
            let q_str = &after_q[..q_end];
            if let Ok(q) = q_str.trim().parse::<f32>() {
                quality = q.clamp(0.0, 1.0);
            }
            // Remove the q parameter from the string for media type parsing
            let before = &s[..q_pos];
            let after = if q_end < after_q.len() {
                &after_q[q_end..]
            } else {
                ""
            };
            format!("{}{}", before, after)
        } else {
            s.to_string()
        };

        let media_type = MediaType::parse(&media_str)?;
        Some(Self {
            media_type,
            quality,
        })
    }
}

impl PartialEq for AcceptItem {
    fn eq(&self, other: &Self) -> bool {
        self.media_type == other.media_type && (self.quality - other.quality).abs() < f32::EPSILON
    }
}

/// Parsed Accept header with quality-ordered media types.
///
/// This extractor parses the Accept header and provides the list of
/// acceptable media types sorted by quality (highest first).
///
/// # Example
///
/// ```ignore
/// use fastapi_core::AcceptHeader;
///
/// async fn handler(accept: AcceptHeader) -> impl IntoResponse {
///     if accept.prefers("application/json") {
///         Json(data).into_response()
///     } else if accept.prefers("text/html") {
///         Html(template).into_response()
///     } else {
///         // Default to JSON
///         Json(data).into_response()
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct AcceptHeader {
    /// Media types sorted by quality (highest first).
    pub items: Vec<AcceptItem>,
}

impl AcceptHeader {
    /// Parse an Accept header value.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let accept = AcceptHeader::parse("text/html, application/json;q=0.9, */*;q=0.1");
    /// assert_eq!(accept.items.len(), 3);
    /// assert_eq!(accept.items[0].media_type.subtype, "html"); // q=1.0
    /// assert_eq!(accept.items[1].media_type.subtype, "json"); // q=0.9
    /// ```
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let mut items: Vec<AcceptItem> = s.split(',').filter_map(AcceptItem::parse).collect();

        // Sort by quality descending, then by specificity
        items.sort_by(|a, b| {
            // Higher quality first
            let q_cmp = b
                .quality
                .partial_cmp(&a.quality)
                .unwrap_or(std::cmp::Ordering::Equal);
            if q_cmp != std::cmp::Ordering::Equal {
                return q_cmp;
            }
            // More specific types first (fewer wildcards)
            let a_wildcards = (a.media_type.typ == "*") as u8 + (a.media_type.subtype == "*") as u8;
            let b_wildcards = (b.media_type.typ == "*") as u8 + (b.media_type.subtype == "*") as u8;
            a_wildcards.cmp(&b_wildcards)
        });

        Self { items }
    }

    /// Create an AcceptHeader that accepts anything.
    #[must_use]
    pub fn any() -> Self {
        Self {
            items: vec![AcceptItem {
                media_type: MediaType::new("*", "*"),
                quality: 1.0,
            }],
        }
    }

    /// Check if a media type is acceptable.
    #[must_use]
    pub fn accepts(&self, media_type: &str) -> bool {
        if self.items.is_empty() {
            return true; // No Accept header means accept anything
        }

        let Some(mt) = MediaType::parse(media_type) else {
            return false;
        };

        self.items
            .iter()
            .any(|item| item.quality > 0.0 && mt.matches(&item.media_type))
    }

    /// Check if a media type is the preferred type.
    ///
    /// Returns true if the given media type matches the highest-quality
    /// acceptable type.
    #[must_use]
    pub fn prefers(&self, media_type: &str) -> bool {
        let Some(mt) = MediaType::parse(media_type) else {
            return false;
        };

        self.items
            .first()
            .map(|item| mt.matches(&item.media_type))
            .unwrap_or(true)
    }

    /// Get the quality value for a specific media type.
    ///
    /// Returns 0.0 if not acceptable, or the quality value if acceptable.
    #[must_use]
    pub fn quality_of(&self, media_type: &str) -> f32 {
        if self.items.is_empty() {
            return 1.0; // No Accept header means q=1.0 for everything
        }

        let Some(mt) = MediaType::parse(media_type) else {
            return 0.0;
        };

        self.items
            .iter()
            .find(|item| mt.matches(&item.media_type))
            .map(|item| item.quality)
            .unwrap_or(0.0)
    }

    /// Negotiate the best media type from a list of available types.
    ///
    /// Returns the first available type that is acceptable, preferring
    /// higher quality matches.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let accept = AcceptHeader::parse("text/html, application/json;q=0.9");
    /// let available = ["application/json", "text/html", "text/plain"];
    /// let best = accept.negotiate(&available);
    /// assert_eq!(best, Some("text/html"));
    /// ```
    #[must_use]
    pub fn negotiate<'a>(&self, available: &[&'a str]) -> Option<&'a str> {
        if self.items.is_empty() {
            return available.first().copied();
        }

        // Score each available type
        let mut scored: Vec<(&str, f32, usize)> = available
            .iter()
            .enumerate()
            .filter_map(|(idx, &media_type)| {
                let q = self.quality_of(media_type);
                if q > 0.0 {
                    Some((media_type, q, idx))
                } else {
                    None
                }
            })
            .collect();

        // Sort by quality descending, then by position in available list
        scored.sort_by(|a, b| {
            let q_cmp = b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal);
            if q_cmp != std::cmp::Ordering::Equal {
                q_cmp
            } else {
                a.2.cmp(&b.2)
            }
        });

        scored.first().map(|(mt, _, _)| *mt)
    }

    /// Check if the Accept header is empty (accepts anything).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl Default for AcceptHeader {
    fn default() -> Self {
        Self::any()
    }
}

impl FromRequest for AcceptHeader {
    type Error = std::convert::Infallible;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let header = req
            .headers()
            .get("accept")
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(Self::parse)
            .unwrap_or_else(Self::any);
        Ok(header)
    }
}

/// A single entry from an Accept-Encoding header.
#[derive(Debug, Clone)]
pub struct AcceptEncodingItem {
    /// The encoding name (e.g., "gzip", "br", "deflate", "identity").
    pub encoding: String,
    /// Quality value (0.0 to 1.0, default 1.0).
    pub quality: f32,
}

impl AcceptEncodingItem {
    /// Parse a single Accept-Encoding item.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let mut quality = 1.0f32;

        let (encoding, _) = if let Some(q_pos) = s.to_ascii_lowercase().find(";q=") {
            let q_str = &s[q_pos + 3..];
            if let Ok(q) = q_str.trim().parse::<f32>() {
                quality = q.clamp(0.0, 1.0);
            }
            (s[..q_pos].trim().to_ascii_lowercase(), quality)
        } else {
            (s.to_ascii_lowercase(), quality)
        };

        if encoding.is_empty() {
            return None;
        }

        Some(Self { encoding, quality })
    }
}

/// Parsed Accept-Encoding header.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::AcceptEncodingHeader;
///
/// async fn handler(encoding: AcceptEncodingHeader) -> impl IntoResponse {
///     if encoding.accepts("br") {
///         // Use Brotli compression
///     } else if encoding.accepts("gzip") {
///         // Use gzip compression
///     }
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct AcceptEncodingHeader {
    /// Encodings sorted by quality (highest first).
    pub items: Vec<AcceptEncodingItem>,
}

impl AcceptEncodingHeader {
    /// Parse an Accept-Encoding header value.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let mut items: Vec<AcceptEncodingItem> =
            s.split(',').filter_map(AcceptEncodingItem::parse).collect();

        items.sort_by(|a, b| {
            b.quality
                .partial_cmp(&a.quality)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Self { items }
    }

    /// Check if an encoding is acceptable.
    #[must_use]
    pub fn accepts(&self, encoding: &str) -> bool {
        let encoding = encoding.to_ascii_lowercase();
        self.items
            .iter()
            .any(|item| item.quality > 0.0 && (item.encoding == encoding || item.encoding == "*"))
    }

    /// Get the preferred encoding from a list of available encodings.
    #[must_use]
    pub fn negotiate<'a>(&self, available: &[&'a str]) -> Option<&'a str> {
        if self.items.is_empty() {
            return available.first().copied();
        }

        let mut best: Option<(&str, f32)> = None;

        for &encoding in available {
            let enc_lower = encoding.to_ascii_lowercase();
            for item in &self.items {
                if item.quality > 0.0 && (item.encoding == enc_lower || item.encoding == "*") {
                    match best {
                        None => best = Some((encoding, item.quality)),
                        Some((_, q)) if item.quality > q => best = Some((encoding, item.quality)),
                        _ => {}
                    }
                    break;
                }
            }
        }

        best.map(|(e, _)| e)
    }
}

impl FromRequest for AcceptEncodingHeader {
    type Error = std::convert::Infallible;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let header = req
            .headers()
            .get("accept-encoding")
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(Self::parse)
            .unwrap_or_default();
        Ok(header)
    }
}

/// A single entry from an Accept-Language header.
#[derive(Debug, Clone)]
pub struct AcceptLanguageItem {
    /// The language tag (e.g., "en", "en-US", "fr-FR").
    pub language: String,
    /// Quality value (0.0 to 1.0, default 1.0).
    pub quality: f32,
}

impl AcceptLanguageItem {
    /// Parse a single Accept-Language item.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let mut quality = 1.0f32;

        let (language, _) = if let Some(q_pos) = s.to_ascii_lowercase().find(";q=") {
            let q_str = &s[q_pos + 3..];
            if let Ok(q) = q_str.trim().parse::<f32>() {
                quality = q.clamp(0.0, 1.0);
            }
            (s[..q_pos].trim().to_string(), quality)
        } else {
            (s.to_string(), quality)
        };

        if language.is_empty() {
            return None;
        }

        Some(Self { language, quality })
    }
}

/// Parsed Accept-Language header.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::AcceptLanguageHeader;
///
/// async fn handler(lang: AcceptLanguageHeader) -> impl IntoResponse {
///     let locale = lang.negotiate(&["en", "fr", "de"]).unwrap_or("en");
///     // Use locale for response
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct AcceptLanguageHeader {
    /// Languages sorted by quality (highest first).
    pub items: Vec<AcceptLanguageItem>,
}

impl AcceptLanguageHeader {
    /// Parse an Accept-Language header value.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let mut items: Vec<AcceptLanguageItem> =
            s.split(',').filter_map(AcceptLanguageItem::parse).collect();

        items.sort_by(|a, b| {
            b.quality
                .partial_cmp(&a.quality)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Self { items }
    }

    /// Check if a language is acceptable.
    #[must_use]
    pub fn accepts(&self, language: &str) -> bool {
        let lang_lower = language.to_ascii_lowercase();
        self.items.iter().any(|item| {
            if item.quality <= 0.0 {
                return false;
            }
            let item_lower = item.language.to_ascii_lowercase();
            // Exact match or prefix match (e.g., "en" matches "en-US")
            item_lower == lang_lower
                || item_lower == "*"
                || lang_lower.starts_with(&format!("{}-", item_lower))
                || item_lower.starts_with(&format!("{}-", lang_lower))
        })
    }

    /// Get the preferred language from a list of available languages.
    #[must_use]
    pub fn negotiate<'a>(&self, available: &[&'a str]) -> Option<&'a str> {
        if self.items.is_empty() {
            return available.first().copied();
        }

        let mut best: Option<(&str, f32, bool)> = None; // (lang, quality, exact_match)

        for &lang in available {
            let lang_lower = lang.to_ascii_lowercase();
            for item in &self.items {
                if item.quality <= 0.0 {
                    continue;
                }
                let item_lower = item.language.to_ascii_lowercase();

                let (matches, exact) = if item_lower == lang_lower {
                    (true, true)
                } else if item_lower == "*" {
                    (true, false)
                } else if lang_lower.starts_with(&format!("{}-", item_lower)) {
                    (true, false)
                } else if item_lower.starts_with(&format!("{}-", lang_lower)) {
                    (true, false)
                } else {
                    (false, false)
                };

                if matches {
                    match best {
                        None => best = Some((lang, item.quality, exact)),
                        Some((_, q, e))
                            if item.quality > q || (item.quality == q && exact && !e) =>
                        {
                            best = Some((lang, item.quality, exact));
                        }
                        _ => {}
                    }
                    break;
                }
            }
        }

        best.map(|(l, _, _)| l)
    }
}

impl FromRequest for AcceptLanguageHeader {
    type Error = std::convert::Infallible;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let header = req
            .headers()
            .get("accept-language")
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(Self::parse)
            .unwrap_or_default();
        Ok(header)
    }
}

/// Error returned when content negotiation fails.
#[derive(Debug, Clone)]
pub struct NotAcceptableError {
    /// The requested media types.
    pub requested: Vec<String>,
    /// The available media types.
    pub available: Vec<String>,
}

impl NotAcceptableError {
    /// Create a new NotAcceptableError.
    #[must_use]
    pub fn new(requested: Vec<String>, available: Vec<String>) -> Self {
        Self {
            requested,
            available,
        }
    }
}

impl fmt::Display for NotAcceptableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Not Acceptable: requested [{}], available [{}]",
            self.requested.join(", "),
            self.available.join(", ")
        )
    }
}

impl std::error::Error for NotAcceptableError {}

impl IntoResponse for NotAcceptableError {
    fn into_response(self) -> Response {
        Response::with_status(crate::response::StatusCode::NOT_ACCEPTABLE)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(
                serde_json::json!({
                    "error": "Not Acceptable",
                    "message": self.to_string(),
                    "requested": self.requested,
                    "available": self.available,
                })
                .to_string()
                .into_bytes(),
            ))
    }
}

/// Helper to build Vary header values for content negotiation.
#[derive(Debug, Clone, Default)]
pub struct VaryBuilder {
    headers: Vec<String>,
}

impl VaryBuilder {
    /// Create a new Vary builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add Accept to the Vary header.
    #[must_use]
    pub fn accept(mut self) -> Self {
        if !self.headers.contains(&"Accept".to_string()) {
            self.headers.push("Accept".to_string());
        }
        self
    }

    /// Add Accept-Encoding to the Vary header.
    #[must_use]
    pub fn accept_encoding(mut self) -> Self {
        if !self.headers.contains(&"Accept-Encoding".to_string()) {
            self.headers.push("Accept-Encoding".to_string());
        }
        self
    }

    /// Add Accept-Language to the Vary header.
    #[must_use]
    pub fn accept_language(mut self) -> Self {
        if !self.headers.contains(&"Accept-Language".to_string()) {
            self.headers.push("Accept-Language".to_string());
        }
        self
    }

    /// Add a custom header to Vary.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !self.headers.contains(&name) {
            self.headers.push(name);
        }
        self
    }

    /// Build the Vary header value.
    #[must_use]
    pub fn build(&self) -> String {
        self.headers.join(", ")
    }

    /// Check if any headers have been added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }
}

#[cfg(test)]
mod content_negotiation_tests {
    use super::*;

    #[test]
    fn media_type_parse_simple() {
        let mt = MediaType::parse("text/html").unwrap();
        assert_eq!(mt.typ, "text");
        assert_eq!(mt.subtype, "html");
        assert!(mt.params.is_empty());
    }

    #[test]
    fn media_type_parse_with_params() {
        let mt = MediaType::parse("text/html; charset=utf-8").unwrap();
        assert_eq!(mt.typ, "text");
        assert_eq!(mt.subtype, "html");
        assert_eq!(mt.param("charset"), Some("utf-8"));
    }

    #[test]
    fn media_type_parse_case_insensitive() {
        let mt = MediaType::parse("TEXT/HTML").unwrap();
        assert_eq!(mt.typ, "text");
        assert_eq!(mt.subtype, "html");
    }

    #[test]
    fn media_type_matches_wildcard() {
        let html = MediaType::new("text", "html");
        let any_text = MediaType::new("text", "*");
        let any = MediaType::new("*", "*");

        assert!(html.matches(&any_text));
        assert!(html.matches(&any));
        assert!(html.matches(&html));
    }

    #[test]
    fn accept_item_parse_with_quality() {
        let item = AcceptItem::parse("text/html;q=0.9").unwrap();
        assert_eq!(item.media_type.typ, "text");
        assert_eq!(item.media_type.subtype, "html");
        assert!((item.quality - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn accept_item_parse_default_quality() {
        let item = AcceptItem::parse("application/json").unwrap();
        assert!((item.quality - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn accept_header_parse_multiple() {
        let accept = AcceptHeader::parse("text/html, application/json;q=0.9, */*;q=0.1");
        assert_eq!(accept.items.len(), 3);
        assert_eq!(accept.items[0].media_type.subtype, "html");
        assert_eq!(accept.items[1].media_type.subtype, "json");
        assert_eq!(accept.items[2].media_type.subtype, "*");
    }

    #[test]
    fn accept_header_prefers() {
        let accept = AcceptHeader::parse("text/html, application/json;q=0.9");
        assert!(accept.prefers("text/html"));
        assert!(!accept.prefers("application/json"));
    }

    #[test]
    fn accept_header_accepts() {
        let accept = AcceptHeader::parse("text/html, application/json;q=0.9");
        assert!(accept.accepts("text/html"));
        assert!(accept.accepts("application/json"));
        assert!(!accept.accepts("image/png"));
    }

    #[test]
    fn accept_header_negotiate() {
        let accept = AcceptHeader::parse("text/html, application/json;q=0.9");
        let available = ["application/json", "text/html", "text/plain"];
        assert_eq!(accept.negotiate(&available), Some("text/html"));
    }

    #[test]
    fn accept_header_negotiate_returns_best_available() {
        let accept = AcceptHeader::parse("application/xml, application/json;q=0.9");
        let available = ["application/json", "text/plain"];
        assert_eq!(accept.negotiate(&available), Some("application/json"));
    }

    #[test]
    fn accept_header_quality_of() {
        let accept = AcceptHeader::parse("text/html, application/json;q=0.9, */*;q=0.1");
        assert!((accept.quality_of("text/html") - 1.0).abs() < f32::EPSILON);
        assert!((accept.quality_of("application/json") - 0.9).abs() < f32::EPSILON);
        assert!((accept.quality_of("image/png") - 0.1).abs() < f32::EPSILON);
    }

    #[test]
    fn accept_header_empty_accepts_all() {
        let accept = AcceptHeader::parse("");
        assert!(accept.accepts("anything/here"));
        assert_eq!(accept.quality_of("text/html"), 1.0);
    }

    #[test]
    fn accept_encoding_parse() {
        let enc = AcceptEncodingHeader::parse("gzip, deflate, br;q=0.8");
        assert_eq!(enc.items.len(), 3);
        assert!(enc.accepts("gzip"));
        assert!(enc.accepts("br"));
    }

    #[test]
    fn accept_encoding_negotiate() {
        let enc = AcceptEncodingHeader::parse("gzip;q=0.9, br");
        let available = ["gzip", "br", "identity"];
        assert_eq!(enc.negotiate(&available), Some("br"));
    }

    #[test]
    fn accept_language_parse() {
        let lang = AcceptLanguageHeader::parse("en-US, en;q=0.9, fr;q=0.8");
        assert_eq!(lang.items.len(), 3);
        assert!(lang.accepts("en-US"));
        assert!(lang.accepts("en"));
        assert!(lang.accepts("fr"));
    }

    #[test]
    fn accept_language_negotiate() {
        let lang = AcceptLanguageHeader::parse("fr, en;q=0.9");
        let available = ["en", "de", "fr"];
        assert_eq!(lang.negotiate(&available), Some("fr"));
    }

    #[test]
    fn accept_language_prefix_match() {
        let lang = AcceptLanguageHeader::parse("en");
        assert!(lang.accepts("en-US"));
        assert!(lang.accepts("en-GB"));
    }

    #[test]
    fn vary_builder() {
        let vary = VaryBuilder::new().accept().accept_encoding().build();
        assert_eq!(vary, "Accept, Accept-Encoding");
    }

    #[test]
    fn vary_builder_no_duplicates() {
        let vary = VaryBuilder::new().accept().accept().build();
        assert_eq!(vary, "Accept");
    }

    #[test]
    fn not_acceptable_error_response() {
        let err = NotAcceptableError::new(
            vec!["image/png".to_string()],
            vec!["application/json".to_string(), "text/html".to_string()],
        );
        let response = err.into_response();
        assert_eq!(
            response.status(),
            crate::response::StatusCode::NOT_ACCEPTABLE
        );
    }
}

#[cfg(test)]
mod header_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    #[test]
    fn snake_to_header_case_simple() {
        assert_eq!(snake_to_header_case("authorization"), "Authorization");
        assert_eq!(snake_to_header_case("content_type"), "Content-Type");
        assert_eq!(snake_to_header_case("x_request_id"), "X-Request-Id");
        assert_eq!(snake_to_header_case("accept"), "Accept");
    }

    #[test]
    fn snake_to_header_case_edge_cases() {
        assert_eq!(snake_to_header_case(""), "");
        assert_eq!(snake_to_header_case("a"), "A");
        assert_eq!(snake_to_header_case("a_b_c"), "A-B-C");
    }

    #[test]
    fn header_deref() {
        let header = Header::new("test", "value".to_string());
        assert_eq!(*header, "value");
    }

    #[test]
    fn header_into_inner() {
        let header = Header::new("test", 42i32);
        assert_eq!(header.into_inner(), 42);
    }

    #[test]
    fn from_header_value_string() {
        let result = String::from_header_value("test value");
        assert_eq!(result.unwrap(), "test value");
    }

    #[test]
    fn from_header_value_i32() {
        assert_eq!(i32::from_header_value("42").unwrap(), 42);
        assert_eq!(i32::from_header_value("-1").unwrap(), -1);
        assert!(i32::from_header_value("abc").is_err());
    }

    #[test]
    fn from_header_value_bool() {
        assert!(bool::from_header_value("true").unwrap());
        assert!(bool::from_header_value("1").unwrap());
        assert!(bool::from_header_value("yes").unwrap());
        assert!(!bool::from_header_value("false").unwrap());
        assert!(!bool::from_header_value("0").unwrap());
        assert!(!bool::from_header_value("no").unwrap());
        assert!(bool::from_header_value("maybe").is_err());
    }

    #[test]
    fn named_header_extract_success() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        req.headers_mut()
            .insert("authorization", b"Bearer token123".to_vec());

        let result = futures_executor::block_on(
            NamedHeader::<String, Authorization>::from_request(&ctx, &mut req),
        );
        let header = result.unwrap();
        assert_eq!(header.value, "Bearer token123");
    }

    #[test]
    fn named_header_extract_i32() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        req.headers_mut().insert("x-request-id", b"12345".to_vec());

        let result = futures_executor::block_on(NamedHeader::<i32, XRequestId>::from_request(
            &ctx, &mut req,
        ));
        let header = result.unwrap();
        assert_eq!(header.value, 12345);
    }

    #[test]
    fn named_header_missing() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        // Don't insert the header

        let result = futures_executor::block_on(
            NamedHeader::<String, Authorization>::from_request(&ctx, &mut req),
        );
        assert!(matches!(
            result,
            Err(HeaderExtractError::MissingHeader { .. })
        ));
    }

    #[test]
    fn named_header_parse_error() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        req.headers_mut()
            .insert("x-request-id", b"not-a-number".to_vec());

        let result = futures_executor::block_on(NamedHeader::<i32, XRequestId>::from_request(
            &ctx, &mut req,
        ));
        assert!(matches!(result, Err(HeaderExtractError::ParseError { .. })));
    }

    #[test]
    fn header_error_display() {
        let err = HeaderExtractError::MissingHeader {
            name: "Authorization".to_string(),
        };
        assert!(err.to_string().contains("Authorization"));

        let err = HeaderExtractError::ParseError {
            name: "X-Count".to_string(),
            value: "abc".to_string(),
            expected: "i32",
            message: "invalid digit".to_string(),
        };
        assert!(err.to_string().contains("X-Count"));
        assert!(err.to_string().contains("abc"));
    }

    #[test]
    fn optional_header_some() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        req.headers_mut()
            .insert("authorization", b"Bearer token".to_vec());

        let result = futures_executor::block_on(
            Option::<NamedHeader<String, Authorization>>::from_request(&ctx, &mut req),
        );
        let opt = result.unwrap();
        assert!(opt.is_some());
        assert_eq!(opt.unwrap().value, "Bearer token");
    }

    #[test]
    fn optional_header_none() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        // Don't insert the header

        let result = futures_executor::block_on(
            Option::<NamedHeader<String, Authorization>>::from_request(&ctx, &mut req),
        );
        let opt = result.unwrap();
        assert!(opt.is_none());
    }
}

#[cfg(test)]
mod oauth2_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::IntoResponse;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    #[test]
    fn oauth2_extract_valid_bearer_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer mytoken123".to_vec());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let bearer = result.unwrap();
        assert_eq!(bearer.token(), "mytoken123");
        assert_eq!(&*bearer, "mytoken123"); // Test Deref
    }

    #[test]
    fn oauth2_extract_bearer_lowercase() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"bearer lowercase_token".to_vec());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let bearer = result.unwrap();
        assert_eq!(bearer.token(), "lowercase_token");
    }

    #[test]
    fn oauth2_missing_header() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // No authorization header

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err.kind, OAuth2BearerErrorKind::MissingHeader);
    }

    #[test]
    fn oauth2_wrong_scheme() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Basic dXNlcjpwYXNz".to_vec());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err.kind, OAuth2BearerErrorKind::InvalidScheme);
    }

    #[test]
    fn oauth2_empty_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer ".to_vec());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err.kind, OAuth2BearerErrorKind::EmptyToken);
    }

    #[test]
    fn oauth2_whitespace_only_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer    ".to_vec());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err.kind, OAuth2BearerErrorKind::EmptyToken);
    }

    #[test]
    fn oauth2_token_with_spaces_trimmed() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer  spaced_token  ".to_vec());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let bearer = result.unwrap();
        assert_eq!(bearer.token(), "spaced_token");
    }

    #[test]
    fn oauth2_optional_extraction_some() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/maybe-protected");
        req.headers_mut()
            .insert("authorization", b"Bearer optional_token".to_vec());

        let result = futures_executor::block_on(Option::<OAuth2PasswordBearer>::from_request(
            &ctx, &mut req,
        ));
        let opt = result.unwrap();
        assert!(opt.is_some());
        assert_eq!(opt.unwrap().token(), "optional_token");
    }

    #[test]
    fn oauth2_optional_extraction_none() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/maybe-protected");
        // No authorization header

        let result = futures_executor::block_on(Option::<OAuth2PasswordBearer>::from_request(
            &ctx, &mut req,
        ));
        let opt = result.unwrap();
        assert!(opt.is_none());
    }

    #[test]
    fn oauth2_error_response_401() {
        let err = OAuth2BearerError::missing_header();
        let response = err.into_response();
        assert_eq!(response.status().as_u16(), 401);
    }

    #[test]
    fn oauth2_error_response_has_www_authenticate() {
        let err = OAuth2BearerError::missing_header();
        let response = err.into_response();

        let www_auth = response
            .headers()
            .iter()
            .find(|(name, _)| name == "www-authenticate")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(www_auth, Some("Bearer".to_string()));
    }

    #[test]
    fn oauth2_error_display() {
        assert!(
            OAuth2BearerError::missing_header()
                .to_string()
                .contains("Missing")
        );
        assert!(
            OAuth2BearerError::invalid_scheme()
                .to_string()
                .contains("Bearer")
        );
        assert!(
            OAuth2BearerError::empty_token()
                .to_string()
                .contains("empty")
        );
    }

    #[test]
    fn oauth2_config_builder() {
        let config = OAuth2PasswordBearerConfig::new("/auth/token")
            .with_refresh_url("/auth/refresh")
            .with_scope("read", "Read access")
            .with_scope("write", "Write access")
            .with_scheme_name("MyOAuth2")
            .with_description("Custom OAuth2 scheme")
            .with_auto_error(false);

        assert_eq!(config.token_url, "/auth/token");
        assert_eq!(config.refresh_url, Some("/auth/refresh".to_string()));
        assert_eq!(config.scopes.len(), 2);
        assert_eq!(config.scopes.get("read"), Some(&"Read access".to_string()));
        assert_eq!(config.scheme_name, Some("MyOAuth2".to_string()));
        assert!(!config.auto_error);
    }

    #[test]
    fn oauth2_password_bearer_accessors() {
        let bearer = OAuth2PasswordBearer::new("test_token");
        assert_eq!(bearer.token(), "test_token");
        assert_eq!(bearer.into_token(), "test_token");
    }

    // ============================================================================
    // Additional Security Tests for fastapi_rust-3mg
    // ============================================================================

    #[test]
    fn oauth2_error_response_json_body_format() {
        let err = OAuth2BearerError::missing_header();
        let response = err.into_response();

        // Verify the body is valid JSON with "detail" field
        let body = match response.body_ref() {
            crate::response::ResponseBody::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            _ => panic!("Expected Bytes body"),
        };

        let json: serde_json::Value =
            serde_json::from_str(&body).expect("Body should be valid JSON");
        assert!(
            json.get("detail").is_some(),
            "Response should have 'detail' field"
        );
        assert_eq!(json["detail"], "Not authenticated");
    }

    #[test]
    fn oauth2_error_invalid_scheme_json_body() {
        let err = OAuth2BearerError::invalid_scheme();
        let response = err.into_response();

        let body = match response.body_ref() {
            crate::response::ResponseBody::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            _ => panic!("Expected Bytes body"),
        };

        let json: serde_json::Value =
            serde_json::from_str(&body).expect("Body should be valid JSON");
        assert_eq!(json["detail"], "Invalid authentication credentials");
    }

    #[test]
    fn oauth2_error_empty_token_json_body() {
        let err = OAuth2BearerError::empty_token();
        let response = err.into_response();

        let body = match response.body_ref() {
            crate::response::ResponseBody::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            _ => panic!("Expected Bytes body"),
        };

        let json: serde_json::Value =
            serde_json::from_str(&body).expect("Body should be valid JSON");
        assert_eq!(json["detail"], "Invalid authentication credentials");
    }

    #[test]
    fn oauth2_error_response_content_type_json() {
        let err = OAuth2BearerError::missing_header();
        let response = err.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(content_type, Some("application/json".to_string()));
    }

    #[test]
    fn oauth2_extract_token_with_special_characters() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // JWT-like token with special characters
        req.headers_mut()
            .insert("authorization", b"Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U".to_vec());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let bearer = result.unwrap();
        assert!(bearer.token().contains("eyJ"));
        assert!(bearer.token().contains("."));
    }

    #[test]
    fn oauth2_extract_token_with_unicode() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // Token with unicode characters (unusual but should work)
        req.headers_mut().insert(
            "authorization",
            "Bearer tökën_with_ünïcödë".as_bytes().to_vec(),
        );

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let bearer = result.unwrap();
        assert_eq!(bearer.token(), "tökën_with_ünïcödë");
    }

    #[test]
    fn oauth2_invalid_utf8_in_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // Invalid UTF-8 sequence
        req.headers_mut().insert(
            "authorization",
            vec![66, 101, 97, 114, 101, 114, 32, 0xFF, 0xFE],
        );

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        // Should return InvalidScheme because it can't be parsed as valid UTF-8
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind,
            OAuth2BearerErrorKind::InvalidScheme
        );
    }

    #[test]
    fn oauth2_only_bearer_prefix_no_space() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // "Bearer" without space or token - should be invalid scheme
        req.headers_mut()
            .insert("authorization", b"Bearertoken".to_vec());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err.kind, OAuth2BearerErrorKind::InvalidScheme);
    }

    #[test]
    fn oauth2_mixed_case_bearer() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // "BEARER" all caps - should fail (only "Bearer" and "bearer" supported)
        req.headers_mut()
            .insert("authorization", b"BEARER uppercase_token".to_vec());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        // Currently the implementation only supports "Bearer " and "bearer " prefixes
        let err = result.unwrap_err();
        assert_eq!(err.kind, OAuth2BearerErrorKind::InvalidScheme);
    }

    #[test]
    fn oauth2_extract_very_long_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // Very long token (4KB)
        let long_token = "x".repeat(4096);
        req.headers_mut()
            .insert("authorization", format!("Bearer {long_token}").into_bytes());

        let result = futures_executor::block_on(OAuth2PasswordBearer::from_request(&ctx, &mut req));
        let bearer = result.unwrap();
        assert_eq!(bearer.token().len(), 4096);
    }

    #[test]
    fn oauth2_config_default_values() {
        let config = OAuth2PasswordBearerConfig::default();

        assert_eq!(config.token_url, "/token");
        assert!(config.refresh_url.is_none());
        assert!(config.scopes.is_empty());
        assert!(config.scheme_name.is_none());
        assert!(config.description.is_none());
        assert!(config.auto_error); // Default should be true
    }

    #[test]
    fn oauth2_error_kind_equality() {
        // Verify error kinds implement PartialEq correctly
        assert_eq!(
            OAuth2BearerErrorKind::MissingHeader,
            OAuth2BearerErrorKind::MissingHeader
        );
        assert_eq!(
            OAuth2BearerErrorKind::InvalidScheme,
            OAuth2BearerErrorKind::InvalidScheme
        );
        assert_eq!(
            OAuth2BearerErrorKind::EmptyToken,
            OAuth2BearerErrorKind::EmptyToken
        );
        assert_ne!(
            OAuth2BearerErrorKind::MissingHeader,
            OAuth2BearerErrorKind::InvalidScheme
        );
    }

    #[test]
    fn oauth2_error_debug_format() {
        // Verify error types implement Debug
        let err = OAuth2BearerError::missing_header();
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("MissingHeader"));
    }

    #[test]
    fn oauth2_bearer_clone() {
        let bearer = OAuth2PasswordBearer::new("cloneable_token");
        let cloned = bearer.clone();
        assert_eq!(bearer.token(), cloned.token());
    }

    #[test]
    fn oauth2_config_clone() {
        let config =
            OAuth2PasswordBearerConfig::new("/auth/token").with_scope("admin", "Admin access");
        let cloned = config.clone();
        assert_eq!(config.token_url, cloned.token_url);
        assert_eq!(config.scopes.len(), cloned.scopes.len());
    }

    #[test]
    fn oauth2_all_error_responses_are_401() {
        // All OAuth2 bearer errors should result in 401 Unauthorized
        let errors = [
            OAuth2BearerError::missing_header(),
            OAuth2BearerError::invalid_scheme(),
            OAuth2BearerError::empty_token(),
        ];

        for err in errors {
            let response = err.into_response();
            assert_eq!(
                response.status().as_u16(),
                401,
                "All OAuth2 errors should be 401"
            );
        }
    }

    #[test]
    fn oauth2_all_error_responses_have_www_authenticate() {
        // All OAuth2 bearer errors should include WWW-Authenticate header
        let errors = [
            OAuth2BearerError::missing_header(),
            OAuth2BearerError::invalid_scheme(),
            OAuth2BearerError::empty_token(),
        ];

        for err in errors {
            let response = err.into_response();
            let has_www_auth = response
                .headers()
                .iter()
                .any(|(name, value)| name == "www-authenticate" && value == b"Bearer");
            assert!(
                has_www_auth,
                "All OAuth2 errors should have WWW-Authenticate: Bearer"
            );
        }
    }
}

#[cfg(test)]
mod bearer_token_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::IntoResponse;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    #[test]
    fn bearer_token_extract_valid_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer mytoken123".to_vec());

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let token = result.unwrap();
        assert_eq!(token.token(), "mytoken123");
        assert_eq!(&*token, "mytoken123"); // Test Deref
        assert_eq!(token.as_ref(), "mytoken123"); // Test AsRef
    }

    #[test]
    fn bearer_token_extract_lowercase_bearer() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"bearer lowercase_token".to_vec());

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let token = result.unwrap();
        assert_eq!(token.token(), "lowercase_token");
    }

    #[test]
    fn bearer_token_missing_header() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // No authorization header

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BearerTokenError::MissingHeader);
    }

    #[test]
    fn bearer_token_wrong_scheme() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Basic dXNlcjpwYXNz".to_vec());

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BearerTokenError::InvalidScheme);
    }

    #[test]
    fn bearer_token_empty_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer ".to_vec());

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BearerTokenError::EmptyToken);
    }

    #[test]
    fn bearer_token_whitespace_only_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer    ".to_vec());

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BearerTokenError::EmptyToken);
    }

    #[test]
    fn bearer_token_with_spaces_trimmed() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer   spaced_token   ".to_vec());

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let token = result.unwrap();
        assert_eq!(token.token(), "spaced_token");
    }

    #[test]
    fn bearer_token_optional_some() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer optional_token".to_vec());

        let result =
            futures_executor::block_on(Option::<BearerToken>::from_request(&ctx, &mut req));
        let maybe_token = result.unwrap();
        assert!(maybe_token.is_some());
        assert_eq!(maybe_token.unwrap().token(), "optional_token");
    }

    #[test]
    fn bearer_token_optional_none() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // No authorization header

        let result =
            futures_executor::block_on(Option::<BearerToken>::from_request(&ctx, &mut req));
        let maybe_token = result.unwrap();
        assert!(maybe_token.is_none());
    }

    #[test]
    fn bearer_token_error_response_401() {
        let err = BearerTokenError::missing_header();
        let response = err.into_response();
        assert_eq!(response.status().as_u16(), 401);
    }

    #[test]
    fn bearer_token_error_has_www_authenticate() {
        let err = BearerTokenError::missing_header();
        let response = err.into_response();

        let has_www_auth = response
            .headers()
            .iter()
            .any(|(name, value)| name == "www-authenticate" && value == b"Bearer");
        assert!(has_www_auth);
    }

    #[test]
    fn bearer_token_error_display() {
        assert_eq!(
            BearerTokenError::missing_header().to_string(),
            "Missing Authorization header"
        );
        assert_eq!(
            BearerTokenError::invalid_scheme().to_string(),
            "Authorization header must use Bearer scheme"
        );
        assert_eq!(
            BearerTokenError::empty_token().to_string(),
            "Bearer token is empty"
        );
    }

    #[test]
    fn bearer_token_error_detail() {
        assert_eq!(
            BearerTokenError::MissingHeader.detail(),
            "Not authenticated"
        );
        assert_eq!(
            BearerTokenError::InvalidScheme.detail(),
            "Invalid authentication credentials"
        );
        assert_eq!(
            BearerTokenError::EmptyToken.detail(),
            "Invalid authentication credentials"
        );
    }

    #[test]
    fn bearer_token_new_and_accessors() {
        let token = BearerToken::new("test_token");
        assert_eq!(token.token(), "test_token");
        assert_eq!(token.clone().into_token(), "test_token");
    }

    #[test]
    fn bearer_token_error_response_json_body() {
        let err = BearerTokenError::missing_header();
        let response = err.into_response();

        let body_str = match response.body_ref() {
            crate::response::ResponseBody::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            _ => panic!("Expected Bytes body"),
        };
        let body: serde_json::Value = serde_json::from_str(&body_str).unwrap();

        assert_eq!(body["detail"], "Not authenticated");
    }

    #[test]
    fn bearer_token_error_content_type_json() {
        let err = BearerTokenError::missing_header();
        let response = err.into_response();

        let has_json_content_type = response
            .headers()
            .iter()
            .any(|(name, value)| name == "content-type" && value == b"application/json");
        assert!(has_json_content_type);
    }

    #[test]
    fn bearer_token_special_characters() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        let special_token = "abc123!@#$%^&*()_+-=[]{}|;':\",./<>?";
        req.headers_mut().insert(
            "authorization",
            format!("Bearer {}", special_token).into_bytes(),
        );

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let token = result.unwrap();
        assert_eq!(token.token(), special_token);
    }

    #[test]
    fn bearer_token_very_long_token() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        let long_token = "a".repeat(10000);
        req.headers_mut().insert(
            "authorization",
            format!("Bearer {}", long_token).into_bytes(),
        );

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let token = result.unwrap();
        assert_eq!(token.token(), long_token);
    }

    #[test]
    fn bearer_token_invalid_utf8() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // Invalid UTF-8 sequence
        req.headers_mut().insert(
            "authorization",
            vec![0x42, 0x65, 0x61, 0x72, 0x65, 0x72, 0x20, 0xFF, 0xFE],
        );

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BearerTokenError::InvalidScheme);
    }

    #[test]
    fn bearer_token_only_bearer_no_space() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // "Bearer" without trailing space and token
        req.headers_mut()
            .insert("authorization", b"Bearer".to_vec());

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BearerTokenError::InvalidScheme);
    }

    #[test]
    fn bearer_token_mixed_case_bearer() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // Mixed case should fail (we only support "Bearer" and "bearer")
        req.headers_mut()
            .insert("authorization", b"BEARER token".to_vec());

        let result = futures_executor::block_on(BearerToken::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BearerTokenError::InvalidScheme);
    }

    #[test]
    fn bearer_token_all_errors_are_401() {
        let errors = vec![
            BearerTokenError::missing_header(),
            BearerTokenError::invalid_scheme(),
            BearerTokenError::empty_token(),
        ];

        for err in errors {
            let response = err.into_response();
            assert_eq!(
                response.status().as_u16(),
                401,
                "All BearerToken errors should be 401"
            );
        }
    }

    #[test]
    fn bearer_token_all_errors_have_www_authenticate() {
        let errors = vec![
            BearerTokenError::missing_header(),
            BearerTokenError::invalid_scheme(),
            BearerTokenError::empty_token(),
        ];

        for err in errors {
            let response = err.into_response();
            let has_www_auth = response
                .headers()
                .iter()
                .any(|(name, value)| name == "www-authenticate" && value == b"Bearer");
            assert!(
                has_www_auth,
                "All BearerToken errors should have WWW-Authenticate: Bearer"
            );
        }
    }

    #[test]
    fn bearer_token_equality() {
        let token1 = BearerToken::new("same_token");
        let token2 = BearerToken::new("same_token");
        let token3 = BearerToken::new("different_token");

        assert_eq!(token1, token2);
        assert_ne!(token1, token3);
    }

    #[test]
    fn bearer_token_error_equality() {
        assert_eq!(
            BearerTokenError::MissingHeader,
            BearerTokenError::MissingHeader
        );
        assert_eq!(
            BearerTokenError::InvalidScheme,
            BearerTokenError::InvalidScheme
        );
        assert_eq!(BearerTokenError::EmptyToken, BearerTokenError::EmptyToken);
        assert_ne!(
            BearerTokenError::MissingHeader,
            BearerTokenError::InvalidScheme
        );
    }

    #[test]
    fn bearer_token_debug() {
        let token = BearerToken::new("debug_token");
        let debug_str = format!("{:?}", token);
        assert!(debug_str.contains("debug_token"));
    }

    #[test]
    fn bearer_token_clone() {
        let token = BearerToken::new("cloneable");
        let cloned = token.clone();
        assert_eq!(token, cloned);
    }
}

#[cfg(test)]
mod api_key_header_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::IntoResponse;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 54321)
    }

    #[test]
    fn api_key_header_extraction_default() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("x-api-key", b"test_api_key_123".to_vec());

        let result = futures_executor::block_on(ApiKeyHeader::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "test_api_key_123");
        assert_eq!(api_key.header_name(), "x-api-key");
    }

    #[test]
    fn api_key_header_missing() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // No API key header

        let result = futures_executor::block_on(ApiKeyHeader::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyHeaderError::MissingHeader { .. }));
    }

    #[test]
    fn api_key_header_empty() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut().insert("x-api-key", b"".to_vec());

        let result = futures_executor::block_on(ApiKeyHeader::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyHeaderError::EmptyKey { .. }));
    }

    #[test]
    fn api_key_header_whitespace_only() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut().insert("x-api-key", b"   ".to_vec());

        let result = futures_executor::block_on(ApiKeyHeader::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyHeaderError::EmptyKey { .. }));
    }

    #[test]
    fn api_key_header_trims_whitespace() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("x-api-key", b"  my_key_123  ".to_vec());

        let result = futures_executor::block_on(ApiKeyHeader::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "my_key_123");
    }

    #[test]
    fn api_key_header_custom_header_name() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"custom_key".to_vec());
        req.insert_extension(ApiKeyHeaderConfig::new().header_name("authorization"));

        let result = futures_executor::block_on(ApiKeyHeader::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "custom_key");
        assert_eq!(api_key.header_name(), "authorization");
    }

    #[test]
    fn api_key_header_invalid_utf8() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // Invalid UTF-8 sequence
        req.headers_mut()
            .insert("x-api-key", vec![0xFF, 0xFE, 0x00, 0x01]);

        let result = futures_executor::block_on(ApiKeyHeader::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyHeaderError::InvalidUtf8 { .. }));
    }

    #[test]
    fn api_key_header_error_response_401() {
        let err = ApiKeyHeaderError::missing_header("x-api-key");
        let response = err.into_response();
        assert_eq!(response.status().as_u16(), 401);
    }

    #[test]
    fn api_key_header_error_response_json() {
        let err = ApiKeyHeaderError::missing_header("x-api-key");
        let response = err.into_response();

        let has_json_content_type = response
            .headers()
            .iter()
            .any(|(name, value)| name == "content-type" && value == b"application/json");
        assert!(has_json_content_type);
    }

    #[test]
    fn api_key_header_secure_compare() {
        let api_key = ApiKeyHeader::new("secret_key_123");

        // Timing-safe comparison
        assert!(api_key.secure_eq("secret_key_123"));
        assert!(!api_key.secure_eq("secret_key_124"));
        assert!(!api_key.secure_eq("wrong"));

        // Bytes comparison
        assert!(api_key.secure_eq_bytes(b"secret_key_123"));
        assert!(!api_key.secure_eq_bytes(b"secret_key_124"));
    }

    #[test]
    fn api_key_header_deref_and_as_ref() {
        let api_key = ApiKeyHeader::new("deref_test");

        // Deref to &str
        let s: &str = &api_key;
        assert_eq!(s, "deref_test");

        // AsRef<str>
        let s: &str = api_key.as_ref();
        assert_eq!(s, "deref_test");
    }

    #[test]
    fn api_key_header_config_defaults() {
        let config = ApiKeyHeaderConfig::default();
        assert_eq!(config.get_header_name(), DEFAULT_API_KEY_HEADER);
    }

    #[test]
    fn api_key_header_error_display() {
        let err = ApiKeyHeaderError::missing_header("x-api-key");
        assert!(err.to_string().contains("x-api-key"));

        let err = ApiKeyHeaderError::empty_key("x-api-key");
        assert!(err.to_string().contains("Empty"));

        let err = ApiKeyHeaderError::invalid_utf8("x-api-key");
        assert!(err.to_string().contains("Invalid UTF-8"));
    }

    #[test]
    fn api_key_header_equality() {
        let key1 = ApiKeyHeader::new("same_key");
        let key2 = ApiKeyHeader::new("same_key");
        let key3 = ApiKeyHeader::new("different_key");

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }
}

#[cfg(test)]
mod api_key_query_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::IntoResponse;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 99999)
    }

    #[test]
    fn api_key_query_basic_extraction() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        req.set_query(Some("api_key=test_key_123".to_string()));

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "test_key_123");
        assert_eq!(api_key.param_name(), "api_key");
    }

    #[test]
    fn api_key_query_missing() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        // No query string

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyQueryError::MissingParam { .. }));
    }

    #[test]
    fn api_key_query_empty_query_string() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        req.set_query(Some("".to_string()));

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyQueryError::MissingParam { .. }));
    }

    #[test]
    fn api_key_query_param_missing_but_others_present() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        req.set_query(Some("other_param=value".to_string()));

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyQueryError::MissingParam { .. }));
    }

    #[test]
    fn api_key_query_empty_value() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        req.set_query(Some("api_key=".to_string()));

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyQueryError::EmptyKey { .. }));
    }

    #[test]
    fn api_key_query_whitespace_only() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        req.set_query(Some("api_key=   ".to_string()));

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyQueryError::EmptyKey { .. }));
    }

    #[test]
    fn api_key_query_trims_whitespace() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        req.set_query(Some("api_key=  my_key_123  ".to_string()));

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "my_key_123");
    }

    #[test]
    fn api_key_query_custom_param_name() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        req.set_query(Some("token=custom_key".to_string()));
        req.insert_extension(ApiKeyQueryConfig::new().param_name("token"));

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "custom_key");
        assert_eq!(api_key.param_name(), "token");
    }

    #[test]
    fn api_key_query_with_other_params() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        req.set_query(Some(
            "callback=https://example.com&api_key=webhook_key&format=json".to_string(),
        ));

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "webhook_key");
    }

    #[test]
    fn api_key_query_url_encoded_value() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/webhook");
        // URL encoded key with special chars: "key+with spaces" -> "key%2Bwith%20spaces"
        req.set_query(Some("api_key=key%2Bwith%20spaces".to_string()));

        let result = futures_executor::block_on(ApiKeyQuery::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "key+with spaces");
    }

    #[test]
    fn api_key_query_error_response_401() {
        let err = ApiKeyQueryError::missing_param("api_key");
        let response = err.into_response();
        assert_eq!(response.status().as_u16(), 401);
    }

    #[test]
    fn api_key_query_error_response_json() {
        let err = ApiKeyQueryError::missing_param("api_key");
        let response = err.into_response();

        let has_json_content_type = response
            .headers()
            .iter()
            .any(|(n, v)| n == "content-type" && v.starts_with(b"application/json"));
        assert!(has_json_content_type);
    }

    #[test]
    fn api_key_query_secure_compare() {
        let api_key = ApiKeyQuery::new("secret_key_123");

        // Timing-safe comparison
        assert!(api_key.secure_eq("secret_key_123"));
        assert!(!api_key.secure_eq("secret_key_124"));
        assert!(!api_key.secure_eq("wrong"));

        // Byte comparison
        assert!(api_key.secure_eq_bytes(b"secret_key_123"));
        assert!(!api_key.secure_eq_bytes(b"secret_key_124"));
    }

    #[test]
    fn api_key_query_deref_and_as_ref() {
        let api_key = ApiKeyQuery::new("deref_test");

        // Deref to &str
        let s: &str = &api_key;
        assert_eq!(s, "deref_test");

        // AsRef<str>
        let s: &str = api_key.as_ref();
        assert_eq!(s, "deref_test");
    }

    #[test]
    fn api_key_query_config_defaults() {
        let config = ApiKeyQueryConfig::default();
        assert_eq!(config.get_param_name(), DEFAULT_API_KEY_QUERY_PARAM);
    }

    #[test]
    fn api_key_query_error_display() {
        let err = ApiKeyQueryError::missing_param("api_key");
        assert!(err.to_string().contains("api_key"));

        let err = ApiKeyQueryError::empty_key("api_key");
        assert!(err.to_string().contains("Empty"));
    }

    #[test]
    fn api_key_query_equality() {
        let key1 = ApiKeyQuery::new("same_key");
        let key2 = ApiKeyQuery::new("same_key");
        let key3 = ApiKeyQuery::new("different_key");

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }
}

#[cfg(test)]
mod api_key_cookie_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::IntoResponse;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 77777)
    }

    #[test]
    fn api_key_cookie_basic_extraction() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("cookie", b"api_key=test_key_123".to_vec());

        let result = futures_executor::block_on(ApiKeyCookie::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "test_key_123");
        assert_eq!(api_key.cookie_name(), "api_key");
    }

    #[test]
    fn api_key_cookie_missing_header() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // No cookie header

        let result = futures_executor::block_on(ApiKeyCookie::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyCookieError::MissingCookie { .. }));
    }

    #[test]
    fn api_key_cookie_other_cookies_present() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("cookie", b"session_id=abc123; theme=dark".to_vec());
        // api_key cookie is missing

        let result = futures_executor::block_on(ApiKeyCookie::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyCookieError::MissingCookie { .. }));
    }

    #[test]
    fn api_key_cookie_empty_value() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut().insert("cookie", b"api_key=".to_vec());

        let result = futures_executor::block_on(ApiKeyCookie::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyCookieError::EmptyKey { .. }));
    }

    #[test]
    fn api_key_cookie_whitespace_only() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut().insert("cookie", b"api_key=   ".to_vec());

        let result = futures_executor::block_on(ApiKeyCookie::from_request(&ctx, &mut req));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiKeyCookieError::EmptyKey { .. }));
    }

    #[test]
    fn api_key_cookie_trims_whitespace() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("cookie", b"api_key=  my_key_123  ".to_vec());

        let result = futures_executor::block_on(ApiKeyCookie::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "my_key_123");
    }

    #[test]
    fn api_key_cookie_custom_name() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("cookie", b"auth_token=custom_key".to_vec());
        req.insert_extension(ApiKeyCookieConfig::new().cookie_name("auth_token"));

        let result = futures_executor::block_on(ApiKeyCookie::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "custom_key");
        assert_eq!(api_key.cookie_name(), "auth_token");
    }

    #[test]
    fn api_key_cookie_with_multiple_cookies() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut().insert(
            "cookie",
            b"session_id=sess123; api_key=my_api_key; theme=dark".to_vec(),
        );

        let result = futures_executor::block_on(ApiKeyCookie::from_request(&ctx, &mut req));
        let api_key = result.unwrap();
        assert_eq!(api_key.key(), "my_api_key");
    }

    #[test]
    fn api_key_cookie_error_response_401() {
        let err = ApiKeyCookieError::missing_cookie("api_key");
        let response = err.into_response();
        assert_eq!(response.status().as_u16(), 401);
    }

    #[test]
    fn api_key_cookie_error_response_json() {
        let err = ApiKeyCookieError::missing_cookie("api_key");
        let response = err.into_response();

        let has_json_content_type = response
            .headers()
            .iter()
            .any(|(n, v)| n == "content-type" && v.starts_with(b"application/json"));
        assert!(has_json_content_type);
    }

    #[test]
    fn api_key_cookie_secure_compare() {
        let api_key = ApiKeyCookie::new("secret_key_123");

        // Timing-safe comparison
        assert!(api_key.secure_eq("secret_key_123"));
        assert!(!api_key.secure_eq("secret_key_124"));
        assert!(!api_key.secure_eq("wrong"));

        // Byte comparison
        assert!(api_key.secure_eq_bytes(b"secret_key_123"));
        assert!(!api_key.secure_eq_bytes(b"secret_key_124"));
    }

    #[test]
    fn api_key_cookie_deref_and_as_ref() {
        let api_key = ApiKeyCookie::new("deref_test");

        // Deref to &str
        let s: &str = &api_key;
        assert_eq!(s, "deref_test");

        // AsRef<str>
        let s: &str = api_key.as_ref();
        assert_eq!(s, "deref_test");
    }

    #[test]
    fn api_key_cookie_config_defaults() {
        let config = ApiKeyCookieConfig::default();
        assert_eq!(config.get_cookie_name(), DEFAULT_API_KEY_COOKIE);
    }

    #[test]
    fn api_key_cookie_error_display() {
        let err = ApiKeyCookieError::missing_cookie("api_key");
        assert!(err.to_string().contains("api_key"));

        let err = ApiKeyCookieError::empty_key("api_key");
        assert!(err.to_string().contains("Empty"));
    }

    #[test]
    fn api_key_cookie_equality() {
        let key1 = ApiKeyCookie::new("same_key");
        let key2 = ApiKeyCookie::new("same_key");
        let key3 = ApiKeyCookie::new("different_key");

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }
}

#[cfg(test)]
mod basic_auth_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::IntoResponse;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    // Helper to base64 encode credentials
    fn encode_basic_auth(username: &str, password: &str) -> String {
        // Manual base64 encoding for test purposes
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let input = format!("{username}:{password}");
        let bytes = input.as_bytes();
        let mut output = String::new();

        for chunk in bytes.chunks(3) {
            let mut n: u32 = 0;
            for (i, &byte) in chunk.iter().enumerate() {
                n |= u32::from(byte) << (16 - 8 * i);
            }

            let chars = match chunk.len() {
                3 => 4,
                2 => 3,
                1 => 2,
                _ => unreachable!(),
            };

            for i in 0..chars {
                let idx = ((n >> (18 - 6 * i)) & 0x3F) as usize;
                output.push(ALPHABET[idx] as char);
            }

            // Add padding
            for _ in chars..4 {
                output.push('=');
            }
        }

        output
    }

    #[test]
    fn basic_auth_extract_valid_credentials() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        let encoded = encode_basic_auth("alice", "secret123");
        req.headers_mut()
            .insert("authorization", format!("Basic {encoded}").into_bytes());

        let result = futures_executor::block_on(BasicAuth::from_request(&ctx, &mut req));
        let auth = result.unwrap();
        assert_eq!(auth.username(), "alice");
        assert_eq!(auth.password(), "secret123");
    }

    #[test]
    fn basic_auth_extract_lowercase_basic() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        let encoded = encode_basic_auth("bob", "pass");
        req.headers_mut()
            .insert("authorization", format!("basic {encoded}").into_bytes());

        let result = futures_executor::block_on(BasicAuth::from_request(&ctx, &mut req));
        let auth = result.unwrap();
        assert_eq!(auth.username(), "bob");
        assert_eq!(auth.password(), "pass");
    }

    #[test]
    fn basic_auth_missing_header() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // No authorization header

        let result = futures_executor::block_on(BasicAuth::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BasicAuthError::MissingHeader);
    }

    #[test]
    fn basic_auth_wrong_scheme() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Bearer sometoken".to_vec());

        let result = futures_executor::block_on(BasicAuth::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BasicAuthError::InvalidScheme);
    }

    #[test]
    fn basic_auth_invalid_base64() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        req.headers_mut()
            .insert("authorization", b"Basic !!!invalid!!!".to_vec());

        let result = futures_executor::block_on(BasicAuth::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BasicAuthError::InvalidBase64);
    }

    #[test]
    fn basic_auth_missing_colon() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // Base64 of "nocolon" (no colon separator)
        req.headers_mut()
            .insert("authorization", b"Basic bm9jb2xvbg==".to_vec());

        let result = futures_executor::block_on(BasicAuth::from_request(&ctx, &mut req));
        let err = result.unwrap_err();
        assert_eq!(err, BasicAuthError::MissingColon);
    }

    #[test]
    fn basic_auth_empty_username() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        let encoded = encode_basic_auth("", "password");
        req.headers_mut()
            .insert("authorization", format!("Basic {encoded}").into_bytes());

        let result = futures_executor::block_on(BasicAuth::from_request(&ctx, &mut req));
        let auth = result.unwrap();
        assert_eq!(auth.username(), "");
        assert_eq!(auth.password(), "password");
    }

    #[test]
    fn basic_auth_empty_password() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        let encoded = encode_basic_auth("user", "");
        req.headers_mut()
            .insert("authorization", format!("Basic {encoded}").into_bytes());

        let result = futures_executor::block_on(BasicAuth::from_request(&ctx, &mut req));
        let auth = result.unwrap();
        assert_eq!(auth.username(), "user");
        assert_eq!(auth.password(), "");
    }

    #[test]
    fn basic_auth_password_with_colons() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // Password contains colons: "pass:word:with:colons"
        let encoded = encode_basic_auth("user", "pass:word:with:colons");
        req.headers_mut()
            .insert("authorization", format!("Basic {encoded}").into_bytes());

        let result = futures_executor::block_on(BasicAuth::from_request(&ctx, &mut req));
        let auth = result.unwrap();
        assert_eq!(auth.username(), "user");
        assert_eq!(auth.password(), "pass:word:with:colons");
    }

    #[test]
    fn basic_auth_optional_some() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        let encoded = encode_basic_auth("optional", "user");
        req.headers_mut()
            .insert("authorization", format!("Basic {encoded}").into_bytes());

        let result = futures_executor::block_on(Option::<BasicAuth>::from_request(&ctx, &mut req));
        let maybe_auth = result.unwrap();
        assert!(maybe_auth.is_some());
        assert_eq!(maybe_auth.unwrap().username(), "optional");
    }

    #[test]
    fn basic_auth_optional_none() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/api/protected");
        // No authorization header

        let result = futures_executor::block_on(Option::<BasicAuth>::from_request(&ctx, &mut req));
        let maybe_auth = result.unwrap();
        assert!(maybe_auth.is_none());
    }

    #[test]
    fn basic_auth_error_response_401() {
        let err = BasicAuthError::missing_header();
        let response = err.into_response();
        assert_eq!(response.status().as_u16(), 401);
    }

    #[test]
    fn basic_auth_error_has_www_authenticate() {
        let err = BasicAuthError::missing_header();
        let response = err.into_response();

        let has_www_auth = response
            .headers()
            .iter()
            .any(|(name, value)| name == "www-authenticate" && value == b"Basic");
        assert!(has_www_auth);
    }

    #[test]
    fn basic_auth_error_display() {
        assert_eq!(
            BasicAuthError::missing_header().to_string(),
            "Missing Authorization header"
        );
        assert_eq!(
            BasicAuthError::invalid_scheme().to_string(),
            "Authorization header must use Basic scheme"
        );
        assert_eq!(
            BasicAuthError::invalid_base64().to_string(),
            "Invalid base64 encoding in credentials"
        );
        assert_eq!(
            BasicAuthError::missing_colon().to_string(),
            "Credentials must contain username:password"
        );
        assert_eq!(
            BasicAuthError::invalid_utf8().to_string(),
            "Credentials contain invalid UTF-8"
        );
    }

    #[test]
    fn basic_auth_error_detail() {
        assert_eq!(BasicAuthError::MissingHeader.detail(), "Not authenticated");
        assert_eq!(
            BasicAuthError::InvalidScheme.detail(),
            "Invalid authentication credentials"
        );
        assert_eq!(
            BasicAuthError::InvalidBase64.detail(),
            "Invalid authentication credentials"
        );
        assert_eq!(
            BasicAuthError::MissingColon.detail(),
            "Invalid authentication credentials"
        );
        assert_eq!(
            BasicAuthError::InvalidUtf8.detail(),
            "Invalid authentication credentials"
        );
    }

    #[test]
    fn basic_auth_new_and_accessors() {
        let auth = BasicAuth::new("testuser", "testpass");
        assert_eq!(auth.username(), "testuser");
        assert_eq!(auth.password(), "testpass");
        let (user, pass) = auth.into_credentials();
        assert_eq!(user, "testuser");
        assert_eq!(pass, "testpass");
    }

    #[test]
    fn basic_auth_error_response_json_body() {
        let err = BasicAuthError::missing_header();
        let response = err.into_response();

        let body_str = match response.body_ref() {
            crate::response::ResponseBody::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            _ => panic!("Expected Bytes body"),
        };
        let body: serde_json::Value = serde_json::from_str(&body_str).unwrap();

        assert_eq!(body["detail"], "Not authenticated");
    }

    #[test]
    fn basic_auth_error_content_type_json() {
        let err = BasicAuthError::missing_header();
        let response = err.into_response();

        let has_json_content_type = response
            .headers()
            .iter()
            .any(|(name, value)| name == "content-type" && value == b"application/json");
        assert!(has_json_content_type);
    }

    #[test]
    fn basic_auth_all_errors_return_401() {
        let errors = [
            BasicAuthError::missing_header(),
            BasicAuthError::invalid_scheme(),
            BasicAuthError::invalid_base64(),
            BasicAuthError::missing_colon(),
            BasicAuthError::invalid_utf8(),
        ];

        for err in errors {
            let response = err.into_response();
            assert_eq!(
                response.status().as_u16(),
                401,
                "All BasicAuth errors should be 401"
            );
        }
    }

    #[test]
    fn basic_auth_all_errors_have_www_authenticate() {
        let errors = [
            BasicAuthError::missing_header(),
            BasicAuthError::invalid_scheme(),
            BasicAuthError::invalid_base64(),
            BasicAuthError::missing_colon(),
            BasicAuthError::invalid_utf8(),
        ];

        for err in errors {
            let response = err.into_response();
            let has_www_auth = response
                .headers()
                .iter()
                .any(|(name, value)| name == "www-authenticate" && value == b"Basic");
            assert!(
                has_www_auth,
                "All BasicAuth errors should have WWW-Authenticate: Basic"
            );
        }
    }

    #[test]
    fn basic_auth_eq_and_clone() {
        let auth1 = BasicAuth::new("user", "pass");
        let auth2 = BasicAuth::new("user", "pass");
        let auth3 = BasicAuth::new("other", "pass");

        assert_eq!(auth1, auth2);
        assert_ne!(auth1, auth3);

        let cloned = auth1.clone();
        assert_eq!(auth1, cloned);
    }

    #[test]
    fn basic_auth_error_eq() {
        assert_eq!(BasicAuthError::MissingHeader, BasicAuthError::MissingHeader);
        assert_eq!(BasicAuthError::InvalidScheme, BasicAuthError::InvalidScheme);
        assert_eq!(BasicAuthError::InvalidBase64, BasicAuthError::InvalidBase64);
        assert_eq!(BasicAuthError::MissingColon, BasicAuthError::MissingColon);
        assert_eq!(BasicAuthError::InvalidUtf8, BasicAuthError::InvalidUtf8);
        assert_ne!(BasicAuthError::MissingHeader, BasicAuthError::InvalidScheme);
    }

    #[test]
    fn basic_auth_debug() {
        let auth = BasicAuth::new("debug_user", "debug_pass");
        let debug_str = format!("{auth:?}");
        assert!(debug_str.contains("debug_user"));
        assert!(debug_str.contains("debug_pass"));
    }

    // Base64 decoder tests
    #[test]
    fn decode_base64_valid() {
        // "user:pass" encodes to "dXNlcjpwYXNz"
        let result = decode_base64("dXNlcjpwYXNz").unwrap();
        assert_eq!(String::from_utf8(result).unwrap(), "user:pass");
    }

    #[test]
    fn decode_base64_with_padding() {
        // "a" encodes to "YQ=="
        let result = decode_base64("YQ==").unwrap();
        assert_eq!(String::from_utf8(result).unwrap(), "a");

        // "ab" encodes to "YWI="
        let result = decode_base64("YWI=").unwrap();
        assert_eq!(String::from_utf8(result).unwrap(), "ab");
    }

    #[test]
    fn decode_base64_without_padding() {
        // Padding is optional
        let result = decode_base64("YQ").unwrap();
        assert_eq!(String::from_utf8(result).unwrap(), "a");

        let result = decode_base64("YWI").unwrap();
        assert_eq!(String::from_utf8(result).unwrap(), "ab");
    }

    #[test]
    fn decode_base64_empty() {
        let result = decode_base64("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn decode_base64_invalid_char() {
        let result = decode_base64("abc!def");
        assert!(result.is_err());
    }

    #[test]
    fn decode_base64_complex_password() {
        // Test with special characters in password
        // "admin:p@$$w0rd!123" base64 encoded
        let encoded = encode_basic_auth("admin", "p@$$w0rd!123");
        // Strip "Basic " prefix that encode_basic_auth adds
        let result = decode_base64(&encoded).unwrap();
        assert_eq!(String::from_utf8(result).unwrap(), "admin:p@$$w0rd!123");
    }
}

#[cfg(test)]
mod secure_compare_tests {
    use super::*;

    // ========================================================================
    // Basic constant_time_eq tests
    // ========================================================================

    #[test]
    fn constant_time_eq_equal_slices() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"a", b"a"));
        assert!(constant_time_eq(
            b"a_very_long_secret_token_12345",
            b"a_very_long_secret_token_12345"
        ));
    }

    #[test]
    fn constant_time_eq_different_slices() {
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"aaaaaa", b"aaaaab"));
        assert!(!constant_time_eq(b"a", b"b"));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn constant_time_eq_binary_data() {
        let a = [0u8, 1, 2, 3, 255, 254, 253];
        let b = [0u8, 1, 2, 3, 255, 254, 253];
        let c = [0u8, 1, 2, 3, 255, 254, 252];

        assert!(constant_time_eq(&a, &b));
        assert!(!constant_time_eq(&a, &c));
    }

    #[test]
    fn constant_time_eq_all_zeros() {
        let a = [0u8; 32];
        let b = [0u8; 32];
        let c = {
            let mut arr = [0u8; 32];
            arr[31] = 1;
            arr
        };

        assert!(constant_time_eq(&a, &b));
        assert!(!constant_time_eq(&a, &c));
    }

    #[test]
    fn constant_time_eq_all_ones() {
        let a = [0xFFu8; 16];
        let b = [0xFFu8; 16];
        let c = {
            let mut arr = [0xFFu8; 16];
            arr[0] = 0xFE;
            arr
        };

        assert!(constant_time_eq(&a, &b));
        assert!(!constant_time_eq(&a, &c));
    }

    // ========================================================================
    // constant_time_str_eq tests
    // ========================================================================

    #[test]
    fn constant_time_str_eq_equal() {
        assert!(constant_time_str_eq("password123", "password123"));
        assert!(constant_time_str_eq("", ""));
        assert!(constant_time_str_eq("🔐", "🔐")); // Unicode
    }

    #[test]
    fn constant_time_str_eq_different() {
        assert!(!constant_time_str_eq("password123", "password124"));
        assert!(!constant_time_str_eq("case", "CASE"));
        assert!(!constant_time_str_eq("🔐", "🔑"));
    }

    #[test]
    fn constant_time_str_eq_unicode() {
        // Multi-byte UTF-8 characters
        assert!(constant_time_str_eq("日本語", "日本語"));
        assert!(!constant_time_str_eq("日本語", "日本话"));
        assert!(!constant_time_str_eq("café", "cafe"));
    }

    // ========================================================================
    // SecureCompare trait tests for BearerToken
    // ========================================================================

    #[test]
    fn bearer_token_secure_eq() {
        let token = BearerToken::new("my_secret_token");

        assert!(token.secure_eq("my_secret_token"));
        assert!(!token.secure_eq("my_secret_Token")); // Case sensitive
        assert!(!token.secure_eq("wrong_token"));
    }

    #[test]
    fn bearer_token_secure_eq_bytes() {
        let token = BearerToken::new("api_key_123");

        assert!(token.secure_eq_bytes(b"api_key_123"));
        assert!(!token.secure_eq_bytes(b"api_key_124"));
    }

    // ========================================================================
    // SecureCompare trait tests for String/str
    // ========================================================================

    #[test]
    fn str_secure_eq() {
        let secret: &str = "hunter2";

        assert!(secret.secure_eq("hunter2"));
        assert!(!secret.secure_eq("hunter3"));
    }

    #[test]
    fn string_secure_eq() {
        let secret = String::from("password");

        assert!(secret.secure_eq("password"));
        assert!(!secret.secure_eq("passwor"));
    }

    #[test]
    fn string_secure_eq_bytes() {
        let secret = String::from("binary_safe");

        assert!(secret.secure_eq_bytes(b"binary_safe"));
        assert!(!secret.secure_eq_bytes(b"binary_Safe"));
    }

    // ========================================================================
    // SecureCompare trait tests for byte slices
    // ========================================================================

    #[test]
    fn byte_slice_secure_eq() {
        let hmac: &[u8] = &[0xDE, 0xAD, 0xBE, 0xEF];

        assert!(hmac.secure_eq_bytes(&[0xDE, 0xAD, 0xBE, 0xEF]));
        assert!(!hmac.secure_eq_bytes(&[0xDE, 0xAD, 0xBE, 0xEE]));
    }

    #[test]
    fn byte_array_secure_eq() {
        let key: [u8; 4] = [1, 2, 3, 4];

        assert!(key.secure_eq_bytes(&[1, 2, 3, 4]));
        assert!(!key.secure_eq_bytes(&[1, 2, 3, 5]));
    }

    #[test]
    fn vec_secure_eq() {
        let token: Vec<u8> = vec![0x41, 0x42, 0x43];

        assert!(token.secure_eq("ABC"));
        assert!(!token.secure_eq("ABD"));
    }

    // ========================================================================
    // Edge cases and security properties
    // ========================================================================

    #[test]
    fn secure_compare_empty_values() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_str_eq("", ""));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(!constant_time_str_eq("", "x"));
    }

    #[test]
    fn secure_compare_single_bit_difference() {
        // These differ by exactly one bit
        let a = [0b1111_1111u8];
        let b = [0b1111_1110u8];

        assert!(!constant_time_eq(&a, &b));
    }

    #[test]
    fn secure_compare_first_byte_differs() {
        // Difference at the very start
        assert!(!constant_time_eq(b"Xsecret", b"Ysecret"));
    }

    #[test]
    fn secure_compare_last_byte_differs() {
        // Difference at the very end
        assert!(!constant_time_eq(b"secretX", b"secretY"));
    }

    #[test]
    fn secure_compare_middle_byte_differs() {
        // Difference in the middle
        assert!(!constant_time_eq(b"secXet", b"secYet"));
    }

    // Test that ensures the trait can be used with the BearerToken extractor
    #[test]
    fn bearer_token_integration_with_secure_compare() {
        let token = BearerToken::new("real_api_token_xyz789");

        // Simulating token validation in a handler
        let stored_token = "real_api_token_xyz789";
        let is_valid = token.secure_eq(stored_token);
        assert!(is_valid);

        // Wrong token should fail
        let wrong_token = "fake_api_token_abc123";
        let is_invalid = !token.secure_eq(wrong_token);
        assert!(is_invalid);
    }

    #[test]
    fn deref_with_secure_compare() {
        // BearerToken derefs to &str, so we can use secure_eq on the deref result
        let token = BearerToken::new("my_token");
        let token_str: &str = &token; // Deref

        // Using SecureCompare on the &str
        assert!(token_str.secure_eq("my_token"));
    }

    // ========================================================================
    // Timing verification (best-effort, not a cryptographic proof)
    // ========================================================================
    // Note: Actual timing tests are notoriously unreliable due to CPU caching,
    // branch prediction, and OS scheduling. These tests verify the algorithm
    // correctness rather than timing properties. For true timing verification,
    // use specialized tools like dudect or benchmarking with statistical analysis.

    #[test]
    fn algorithm_processes_all_bytes() {
        // This test verifies the algorithm structure by checking that
        // all bytes are processed regardless of early differences.
        // The fold operation ensures all bytes are XORed.

        // Same length, differ at position 0
        let a = b"Xsecret_token";
        let b = b"Ysecret_token";
        assert!(!constant_time_eq(a, b));

        // Same length, differ at last position
        let c = b"secret_tokenX";
        let d = b"secret_tokenY";
        assert!(!constant_time_eq(c, d));

        // Both should take the same code path (all 13 bytes XORed)
        // We can't easily verify timing in unit tests, but we verify correctness
    }
}

#[cfg(test)]
mod pagination_tests {
    use super::*;
    use crate::request::Method;
    use crate::response::IntoResponse;

    // Helper to create a test context
    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    // ========================================================================
    // Pagination struct tests
    // ========================================================================

    #[test]
    fn pagination_default_values() {
        let p = Pagination::default();
        assert_eq!(p.page(), DEFAULT_PAGE);
        assert_eq!(p.per_page(), DEFAULT_PER_PAGE);
        assert_eq!(p.limit(), DEFAULT_PER_PAGE);
        assert_eq!(p.offset(), 0);
    }

    #[test]
    fn pagination_new() {
        let p = Pagination::new(3, 50);
        assert_eq!(p.page(), 3);
        assert_eq!(p.per_page(), 50);
        assert_eq!(p.offset(), 100); // (3-1) * 50
    }

    #[test]
    fn pagination_new_clamps_per_page() {
        // Below minimum
        let p = Pagination::new(1, 0);
        assert_eq!(p.per_page(), 1);

        // Above maximum
        let p = Pagination::new(1, 1000);
        assert_eq!(p.per_page(), MAX_PER_PAGE);
    }

    #[test]
    fn pagination_new_clamps_page() {
        // Page 0 should become 1
        let p = Pagination::new(0, 20);
        assert_eq!(p.page(), 1);
    }

    #[test]
    fn pagination_from_offset() {
        let p = Pagination::from_offset(40, 20);
        assert_eq!(p.offset(), 40); // Explicit offset preserved
        assert_eq!(p.per_page(), 20);
        assert_eq!(p.page(), 3); // 40/20 + 1
    }

    #[test]
    fn pagination_total_pages() {
        let p = Pagination::new(1, 10);
        assert_eq!(p.total_pages(0), 0);
        assert_eq!(p.total_pages(10), 1);
        assert_eq!(p.total_pages(11), 2);
        assert_eq!(p.total_pages(100), 10);
    }

    #[test]
    fn pagination_has_next_prev() {
        let p = Pagination::new(1, 10);
        assert!(!p.has_prev());
        assert!(p.has_next(100));

        let p = Pagination::new(5, 10);
        assert!(p.has_prev());
        assert!(p.has_next(100));

        let p = Pagination::new(10, 10);
        assert!(p.has_prev());
        assert!(!p.has_next(100));
    }

    // ========================================================================
    // Pagination extractor tests
    // ========================================================================

    #[test]
    fn pagination_extractor_default_params() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items");

        let p = futures_executor::block_on(Pagination::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(p.page(), DEFAULT_PAGE);
        assert_eq!(p.per_page(), DEFAULT_PER_PAGE);
    }

    #[test]
    fn pagination_extractor_page_param() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items?page=5");
        req.insert_extension(QueryParams::parse("page=5"));

        let p = futures_executor::block_on(Pagination::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(p.page(), 5);
        assert_eq!(p.per_page(), DEFAULT_PER_PAGE);
    }

    #[test]
    fn pagination_extractor_per_page_param() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items?per_page=50");
        req.insert_extension(QueryParams::parse("per_page=50"));

        let p = futures_executor::block_on(Pagination::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(p.page(), DEFAULT_PAGE);
        assert_eq!(p.per_page(), 50);
    }

    #[test]
    fn pagination_extractor_limit_alias() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items?limit=25");
        req.insert_extension(QueryParams::parse("limit=25"));

        let p = futures_executor::block_on(Pagination::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(p.per_page(), 25);
    }

    #[test]
    fn pagination_extractor_offset_param() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items?offset=40&limit=10");
        req.insert_extension(QueryParams::parse("offset=40&limit=10"));

        let p = futures_executor::block_on(Pagination::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(p.offset(), 40);
        assert_eq!(p.per_page(), 10);
    }

    #[test]
    fn pagination_extractor_clamps_max_per_page() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items?per_page=1000");
        req.insert_extension(QueryParams::parse("per_page=1000"));

        let p = futures_executor::block_on(Pagination::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(p.per_page(), MAX_PER_PAGE);
    }

    #[test]
    fn pagination_extractor_invalid_page_uses_default() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items?page=abc");
        req.insert_extension(QueryParams::parse("page=abc"));

        let p = futures_executor::block_on(Pagination::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(p.page(), DEFAULT_PAGE);
    }

    // ========================================================================
    // Page struct tests
    // ========================================================================

    #[test]
    fn page_new() {
        let items = vec!["a", "b", "c"];
        let pagination = Pagination::new(2, 10);
        let page = Page::new(items.clone(), 100, pagination, "/items".to_string());

        assert_eq!(page.items, items);
        assert_eq!(page.total, 100);
        assert_eq!(page.page, 2);
        assert_eq!(page.per_page, 10);
        assert_eq!(page.pages, 10);
    }

    #[test]
    fn page_with_values() {
        let items = vec![1, 2, 3];
        let page = Page::with_values(items.clone(), 50, 3, 10, "/users");

        assert_eq!(page.items, items);
        assert_eq!(page.total, 50);
        assert_eq!(page.page, 3);
        assert_eq!(page.per_page, 10);
        assert_eq!(page.pages, 5);
    }

    #[test]
    fn page_len_is_empty() {
        let page: Page<i32> = Page::with_values(vec![], 0, 1, 10, "/items");
        assert!(page.is_empty());
        assert_eq!(page.len(), 0);

        let page = Page::with_values(vec![1, 2, 3], 100, 1, 10, "/items");
        assert!(!page.is_empty());
        assert_eq!(page.len(), 3);
    }

    #[test]
    fn page_has_next_prev() {
        // First page
        let page = Page::with_values(vec![1, 2, 3], 100, 1, 10, "/items");
        assert!(!page.has_prev());
        assert!(page.has_next());

        // Middle page
        let page = Page::with_values(vec![1, 2, 3], 100, 5, 10, "/items");
        assert!(page.has_prev());
        assert!(page.has_next());

        // Last page
        let page = Page::with_values(vec![1, 2, 3], 100, 10, 10, "/items");
        assert!(page.has_prev());
        assert!(!page.has_next());
    }

    #[test]
    fn page_map() {
        let page = Page::with_values(vec![1, 2, 3], 100, 1, 10, "/items");
        let mapped = page.map(|n| n * 2);

        assert_eq!(mapped.items, vec![2, 4, 6]);
        assert_eq!(mapped.total, 100);
        assert_eq!(mapped.page, 1);
    }

    // ========================================================================
    // Link header tests
    // ========================================================================

    #[test]
    fn page_link_header_first_page() {
        let page = Page::with_values(vec![1, 2, 3], 100, 1, 10, "/items");
        let link = page.link_header();

        assert!(link.contains("rel=\"first\""));
        assert!(link.contains("rel=\"last\""));
        assert!(link.contains("rel=\"next\""));
        assert!(!link.contains("rel=\"prev\"")); // No prev on first page
        assert!(link.contains("page=1"));
        assert!(link.contains("page=2")); // Next page
        assert!(link.contains("page=10")); // Last page
    }

    #[test]
    fn page_link_header_middle_page() {
        let page = Page::with_values(vec![1, 2, 3], 100, 5, 10, "/items");
        let link = page.link_header();

        assert!(link.contains("rel=\"first\""));
        assert!(link.contains("rel=\"last\""));
        assert!(link.contains("rel=\"next\""));
        assert!(link.contains("rel=\"prev\""));
        assert!(link.contains("page=4")); // Prev page
        assert!(link.contains("page=6")); // Next page
    }

    #[test]
    fn page_link_header_last_page() {
        let page = Page::with_values(vec![1, 2, 3], 100, 10, 10, "/items");
        let link = page.link_header();

        assert!(link.contains("rel=\"first\""));
        assert!(link.contains("rel=\"last\""));
        assert!(!link.contains("rel=\"next\"")); // No next on last page
        assert!(link.contains("rel=\"prev\""));
        assert!(link.contains("page=9")); // Prev page
    }

    #[test]
    fn page_link_header_single_page() {
        let page = Page::with_values(vec![1, 2, 3], 3, 1, 10, "/items");
        let link = page.link_header();

        assert!(link.contains("rel=\"first\""));
        assert!(link.contains("rel=\"last\""));
        assert!(!link.contains("rel=\"next\"")); // Only one page
        assert!(!link.contains("rel=\"prev\""));
    }

    // ========================================================================
    // IntoResponse tests
    // ========================================================================

    #[test]
    fn page_into_response_status_ok() {
        let page = Page::with_values(vec![1, 2, 3], 100, 1, 10, "/items");
        let response = page.into_response();

        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn page_into_response_content_type() {
        let page = Page::with_values(vec![1, 2, 3], 100, 1, 10, "/items");
        let response = page.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type");
        assert!(content_type.is_some());
        assert_eq!(content_type.unwrap().1, b"application/json");
    }

    #[test]
    fn page_into_response_has_link_header() {
        let page = Page::with_values(vec![1, 2, 3], 100, 1, 10, "/items");
        let response = page.into_response();

        let link_header = response.headers().iter().find(|(name, _)| name == "link");
        assert!(link_header.is_some());

        let link_value = String::from_utf8_lossy(&link_header.unwrap().1);
        assert!(link_value.contains("rel=\"first\""));
    }

    #[test]
    fn page_into_response_has_pagination_headers() {
        let page = Page::with_values(vec![1, 2, 3], 100, 2, 10, "/items");
        let response = page.into_response();

        let get_header = |name: &str| {
            response
                .headers()
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| String::from_utf8_lossy(v).to_string())
        };

        assert_eq!(get_header("x-total-count"), Some("100".to_string()));
        assert_eq!(get_header("x-page"), Some("2".to_string()));
        assert_eq!(get_header("x-per-page"), Some("10".to_string()));
        assert_eq!(get_header("x-total-pages"), Some("10".to_string()));
    }

    #[test]
    fn page_into_response_json_body() {
        let page = Page::with_values(vec!["a", "b", "c"], 100, 2, 10, "/items");
        let response = page.into_response();

        let body_str = match response.body_ref() {
            crate::response::ResponseBody::Bytes(b) => String::from_utf8_lossy(b).to_string(),
            _ => panic!("Expected bytes body"),
        };

        // Parse and verify JSON structure
        let json: serde_json::Value = serde_json::from_str(&body_str).unwrap();
        assert_eq!(json["items"], serde_json::json!(["a", "b", "c"]));
        assert_eq!(json["total"], 100);
        assert_eq!(json["page"], 2);
        assert_eq!(json["per_page"], 10);
        assert_eq!(json["pages"], 10);
    }

    // ========================================================================
    // PaginationConfig tests
    // ========================================================================

    #[test]
    fn pagination_config_default() {
        let config = PaginationConfig::default();
        assert_eq!(config.default_per_page, DEFAULT_PER_PAGE);
        assert_eq!(config.max_per_page, MAX_PER_PAGE);
        assert_eq!(config.default_page, DEFAULT_PAGE);
    }

    #[test]
    fn pagination_config_builder() {
        let config = PaginationConfig::new()
            .default_per_page(50)
            .max_per_page(200)
            .default_page(1);

        assert_eq!(config.default_per_page, 50);
        assert_eq!(config.max_per_page, 200);
        assert_eq!(config.default_page, 1);
    }

    // ========================================================================
    // Integration tests
    // ========================================================================

    #[test]
    fn pagination_paginate_helper() {
        let pagination = Pagination::new(2, 10);
        let items = vec!["item1", "item2", "item3"];

        let page = pagination.paginate(items.clone(), 100, "/api/items");

        assert_eq!(page.items, items);
        assert_eq!(page.total, 100);
        assert_eq!(page.page, 2);
        assert_eq!(page.per_page, 10);
        assert_eq!(page.pages, 10);
    }

    #[test]
    fn pagination_equality() {
        let p1 = Pagination::new(2, 10);
        let p2 = Pagination::new(2, 10);
        let p3 = Pagination::new(3, 10);

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);
    }

    #[test]
    fn pagination_copy_clone() {
        let p1 = Pagination::new(2, 10);
        let p2 = p1; // Copy
        let p3 = p1; // Copy

        assert_eq!(p1, p2);
        assert_eq!(p1, p3);
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use crate::request::Method;
    use serde::Deserialize;

    // Helper to create a test context
    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    // Helper to create a request with path params
    fn request_with_params(params: Vec<(&str, &str)>) -> Request {
        let mut req = Request::new(Method::Get, "/test");
        let path_params = PathParams::from_pairs(
            params
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        req.insert_extension(path_params);
        req
    }

    #[test]
    fn path_params_get() {
        let params = PathParams::from_pairs(vec![("id".to_string(), "42".to_string())]);
        assert_eq!(params.get("id"), Some("42"));
        assert_eq!(params.get("unknown"), None);
    }

    #[test]
    fn path_params_len() {
        let params = PathParams::new();
        assert!(params.is_empty());
        assert_eq!(params.len(), 0);

        let params = PathParams::from_pairs(vec![
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "2".to_string()),
        ]);
        assert!(!params.is_empty());
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn path_extract_single_i64() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("id", "42")]);

        let result = futures_executor::block_on(Path::<i64>::from_request(&ctx, &mut req));
        let Path(id) = result.unwrap();
        assert_eq!(id, 42);
    }

    #[test]
    fn path_extract_single_string() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("name", "alice")]);

        let result = futures_executor::block_on(Path::<String>::from_request(&ctx, &mut req));
        let Path(name) = result.unwrap();
        assert_eq!(name, "alice");
    }

    #[test]
    fn path_extract_single_u32() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("count", "100")]);

        let result = futures_executor::block_on(Path::<u32>::from_request(&ctx, &mut req));
        let Path(count) = result.unwrap();
        assert_eq!(count, 100);
    }

    #[test]
    fn path_extract_tuple() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("user_id", "42"), ("post_id", "99")]);

        let result = futures_executor::block_on(Path::<(i64, i64)>::from_request(&ctx, &mut req));
        let Path((user_id, post_id)) = result.unwrap();
        assert_eq!(user_id, 42);
        assert_eq!(post_id, 99);
    }

    #[test]
    fn path_extract_tuple_mixed_types() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("name", "alice"), ("id", "123")]);

        let result =
            futures_executor::block_on(Path::<(String, i64)>::from_request(&ctx, &mut req));
        let Path((name, id)) = result.unwrap();
        assert_eq!(name, "alice");
        assert_eq!(id, 123);
    }

    #[test]
    fn path_extract_struct() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct UserPath {
            user_id: i64,
            post_id: i64,
        }

        let ctx = test_context();
        let mut req = request_with_params(vec![("user_id", "42"), ("post_id", "99")]);

        let result = futures_executor::block_on(Path::<UserPath>::from_request(&ctx, &mut req));
        let Path(path) = result.unwrap();
        assert_eq!(path.user_id, 42);
        assert_eq!(path.post_id, 99);
    }

    #[test]
    fn path_extract_missing_params() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        // No PathParams extension set

        let result = futures_executor::block_on(Path::<i64>::from_request(&ctx, &mut req));
        assert!(matches!(result, Err(PathExtractError::MissingPathParams)));
    }

    #[test]
    fn path_extract_invalid_type() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("id", "not_a_number")]);

        let result = futures_executor::block_on(Path::<i64>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(PathExtractError::InvalidValue { name, .. }) if name == "id"
        ));
    }

    #[test]
    fn path_extract_negative_for_unsigned() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("count", "-5")]);

        let result = futures_executor::block_on(Path::<u32>::from_request(&ctx, &mut req));
        assert!(matches!(result, Err(PathExtractError::InvalidValue { .. })));
    }

    #[test]
    fn path_extract_f64() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("price", "19.99")]);

        let result = futures_executor::block_on(Path::<f64>::from_request(&ctx, &mut req));
        let Path(price) = result.unwrap();
        assert!((price - 19.99).abs() < 0.001);
    }

    #[test]
    fn path_deref() {
        let path = Path(42i64);
        assert_eq!(*path, 42);
    }

    #[test]
    fn path_into_inner() {
        let path = Path("hello".to_string());
        assert_eq!(path.into_inner(), "hello");
    }

    #[test]
    fn path_error_display() {
        let err = PathExtractError::MissingPathParams;
        assert!(err.to_string().contains("not available"));

        let err = PathExtractError::MissingParam {
            name: "user_id".to_string(),
        };
        assert!(err.to_string().contains("user_id"));

        let err = PathExtractError::InvalidValue {
            name: "id".to_string(),
            value: "abc".to_string(),
            expected: "i64",
            message: "invalid digit".to_string(),
        };
        assert!(err.to_string().contains("id"));
        assert!(err.to_string().contains("abc"));
        assert!(err.to_string().contains("i64"));
    }

    #[test]
    fn path_extract_bool() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("active", "true")]);

        let result = futures_executor::block_on(Path::<bool>::from_request(&ctx, &mut req));
        let Path(active) = result.unwrap();
        assert!(active);
    }

    #[test]
    fn path_extract_char() {
        let ctx = test_context();
        let mut req = request_with_params(vec![("letter", "A")]);

        let result = futures_executor::block_on(Path::<char>::from_request(&ctx, &mut req));
        let Path(letter) = result.unwrap();
        assert_eq!(letter, 'A');
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;
    use crate::request::Method;
    use serde::Deserialize;

    // Helper to create a test context
    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    // Helper to create a request with query string
    fn request_with_query(query: &str) -> Request {
        let mut req = Request::new(Method::Get, "/test");
        req.set_query(Some(query.to_string()));
        req
    }

    #[test]
    fn query_params_parse() {
        let params = QueryParams::parse("a=1&b=2&c=3");
        assert_eq!(params.get("a"), Some("1"));
        assert_eq!(params.get("b"), Some("2"));
        assert_eq!(params.get("c"), Some("3"));
        assert_eq!(params.get("d"), None);
    }

    #[test]
    fn query_params_multi_value() {
        let params = QueryParams::parse("tag=rust&tag=web&tag=api");
        assert_eq!(params.get("tag"), Some("rust")); // First value
        assert_eq!(params.get_all("tag"), vec!["rust", "web", "api"]);
    }

    #[test]
    fn query_params_percent_decode() {
        let params = QueryParams::parse("msg=hello%20world&name=caf%C3%A9");
        assert_eq!(params.get("msg"), Some("hello world"));
        assert_eq!(params.get("name"), Some("café"));
    }

    #[test]
    fn query_params_plus_as_space() {
        let params = QueryParams::parse("msg=hello+world");
        assert_eq!(params.get("msg"), Some("hello world"));
    }

    #[test]
    fn query_params_empty_value() {
        let params = QueryParams::parse("flag&name=alice");
        assert!(params.contains("flag"));
        assert_eq!(params.get("flag"), Some(""));
        assert_eq!(params.get("name"), Some("alice"));
    }

    #[test]
    fn query_extract_struct() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct SearchParams {
            q: String,
            page: i32,
        }

        let ctx = test_context();
        let mut req = request_with_query("q=rust&page=5");

        let result =
            futures_executor::block_on(Query::<SearchParams>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.q, "rust");
        assert_eq!(params.page, 5);
    }

    #[test]
    fn query_extract_optional_field() {
        #[derive(Deserialize, Debug)]
        struct Params {
            required: String,
            optional: Option<i32>,
        }

        let ctx = test_context();

        // With optional present
        let mut req = request_with_query("required=hello&optional=42");
        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.required, "hello");
        assert_eq!(params.optional, Some(42));

        // Without optional
        let mut req = request_with_query("required=hello");
        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.required, "hello");
        assert_eq!(params.optional, None);
    }

    #[test]
    fn query_extract_multi_value() {
        #[derive(Deserialize, Debug)]
        struct Params {
            tags: Vec<String>,
        }

        let ctx = test_context();
        let mut req = request_with_query("tags=rust&tags=web&tags=api");

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.tags, vec!["rust", "web", "api"]);
    }

    #[test]
    fn query_extract_default_value() {
        #[derive(Deserialize, Debug)]
        struct Params {
            name: String,
            #[serde(default)]
            limit: i32,
        }

        let ctx = test_context();
        let mut req = request_with_query("name=test");

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.name, "test");
        assert_eq!(params.limit, 0); // Default for i32
    }

    #[test]
    fn query_extract_bool() {
        #[derive(Deserialize, Debug)]
        struct Params {
            active: bool,
            archived: bool,
        }

        let ctx = test_context();
        let mut req = request_with_query("active=true&archived=false");

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert!(params.active);
        assert!(!params.archived);
    }

    #[test]
    fn query_extract_bool_variants() {
        #[derive(Deserialize, Debug)]
        struct Params {
            a: bool,
            b: bool,
            c: bool,
        }

        let ctx = test_context();
        let mut req = request_with_query("a=1&b=yes&c=on");

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert!(params.a);
        assert!(params.b);
        assert!(params.c);
    }

    #[test]
    fn query_extract_missing_required_fails() {
        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct Params {
            required: String,
        }

        let ctx = test_context();
        let mut req = request_with_query("other=value");

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        assert!(result.is_err());
    }

    #[test]
    fn query_extract_invalid_type_fails() {
        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct Params {
            count: i32,
        }

        let ctx = test_context();
        let mut req = request_with_query("count=not_a_number");

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        assert!(result.is_err());
    }

    #[test]
    fn query_extract_empty_query() {
        #[derive(Deserialize, Debug, Default)]
        struct Params {
            #[serde(default)]
            name: String,
        }

        let ctx = test_context();
        let mut req = request_with_query("");

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.name, "");
    }

    #[test]
    fn query_extract_float() {
        #[derive(Deserialize, Debug)]
        struct Params {
            price: f64,
        }

        let ctx = test_context();
        let mut req = request_with_query("price=29.99");

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert!((params.price - 29.99).abs() < 0.001);
    }

    #[test]
    fn query_deref() {
        #[derive(Deserialize, Debug)]
        struct Params {
            name: String,
        }

        let query = Query(Params {
            name: "test".to_string(),
        });
        assert_eq!(query.name, "test");
    }

    #[test]
    fn query_into_inner() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct Params {
            value: i32,
        }

        let query = Query(Params { value: 42 });
        assert_eq!(query.into_inner(), Params { value: 42 });
    }

    #[test]
    fn query_error_display() {
        let err = QueryExtractError::MissingParam {
            name: "user_id".to_string(),
        };
        assert!(err.to_string().contains("user_id"));

        let err = QueryExtractError::InvalidValue {
            name: "count".to_string(),
            value: "abc".to_string(),
            expected: "i32",
            message: "invalid digit".to_string(),
        };
        assert!(err.to_string().contains("count"));
        assert!(err.to_string().contains("abc"));
        assert!(err.to_string().contains("i32"));
    }

    #[test]
    fn query_params_keys() {
        let params = QueryParams::parse("a=1&b=2&a=3&c=4");
        let keys: Vec<&str> = params.keys().collect();
        assert_eq!(keys, vec!["a", "b", "c"]); // Unique keys in order
    }

    #[test]
    fn query_params_len() {
        let params = QueryParams::parse("a=1&b=2&c=3");
        assert_eq!(params.len(), 3);
        assert!(!params.is_empty());

        let empty = QueryParams::new();
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
    }
}

// ============================================================================
// Optional Extraction Tests
// ============================================================================

#[cfg(test)]
mod optional_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 99999)
    }

    // --- Option<Json<T>> Tests ---

    #[test]
    fn optional_json_present_valid() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Data {
            value: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"{\"value\": 42}".to_vec()));

        let result = futures_executor::block_on(Option::<Json<Data>>::from_request(&ctx, &mut req));
        let Some(Json(data)) = result.unwrap() else {
            panic!("Expected Some");
        };
        assert_eq!(data.value, 42);
    }

    #[test]
    fn optional_json_invalid_content_type_returns_none() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            value: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"text/plain".to_vec());
        req.set_body(Body::Bytes(b"{\"value\": 42}".to_vec()));

        let result = futures_executor::block_on(Option::<Json<Data>>::from_request(&ctx, &mut req));
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn optional_json_missing_body_returns_none() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            value: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        // No body set, but content-type is present - will fail parsing

        let result = futures_executor::block_on(Option::<Json<Data>>::from_request(&ctx, &mut req));
        // Either None (if content-type check fails) or None from parse error
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn optional_json_malformed_returns_none() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            value: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"{ not valid json }".to_vec()));

        let result = futures_executor::block_on(Option::<Json<Data>>::from_request(&ctx, &mut req));
        assert!(result.unwrap().is_none());
    }

    // --- Option<Path<T>> Tests ---

    #[test]
    fn optional_path_present_valid() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/users/42");
        req.insert_extension(PathParams::from_pairs(vec![(
            "id".to_string(),
            "42".to_string(),
        )]));

        let result = futures_executor::block_on(Option::<Path<i64>>::from_request(&ctx, &mut req));
        let Some(Path(id)) = result.unwrap() else {
            panic!("Expected Some");
        };
        assert_eq!(id, 42);
    }

    #[test]
    fn optional_path_missing_params_returns_none() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/users/42");
        // No PathParams set

        let result = futures_executor::block_on(Option::<Path<i64>>::from_request(&ctx, &mut req));
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn optional_path_invalid_type_returns_none() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/users/abc");
        req.insert_extension(PathParams::from_pairs(vec![(
            "id".to_string(),
            "abc".to_string(),
        )]));

        let result = futures_executor::block_on(Option::<Path<i64>>::from_request(&ctx, &mut req));
        assert!(result.unwrap().is_none());
    }

    // --- Option<Query<T>> Tests ---

    #[test]
    fn optional_query_present_valid() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Params {
            page: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items");
        req.set_query(Some("page=5".to_string()));

        let result =
            futures_executor::block_on(Option::<Query<Params>>::from_request(&ctx, &mut req));
        let Some(Query(params)) = result.unwrap() else {
            panic!("Expected Some");
        };
        assert_eq!(params.page, 5);
    }

    #[test]
    fn optional_query_missing_returns_none() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Params {
            required: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items");
        // No query set

        let result =
            futures_executor::block_on(Option::<Query<Params>>::from_request(&ctx, &mut req));
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn optional_query_invalid_type_returns_none() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Params {
            page: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items");
        req.set_query(Some("page=abc".to_string()));

        let result =
            futures_executor::block_on(Option::<Query<Params>>::from_request(&ctx, &mut req));
        assert!(result.unwrap().is_none());
    }

    // --- Option<State<T>> Tests ---

    #[test]
    fn optional_state_present() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        let app_state = AppState::new().with(42i32);
        req.insert_extension(app_state);

        let result = futures_executor::block_on(Option::<State<i32>>::from_request(&ctx, &mut req));
        let Some(State(val)) = result.unwrap() else {
            panic!("Expected Some");
        };
        assert_eq!(val, 42);
    }

    #[test]
    fn optional_state_missing_returns_none() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        // No AppState set

        let result = futures_executor::block_on(Option::<State<i32>>::from_request(&ctx, &mut req));
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn optional_state_wrong_type_returns_none() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");
        let app_state = AppState::new().with("string".to_string()); // String, not i32
        req.insert_extension(app_state);

        let result = futures_executor::block_on(Option::<State<i32>>::from_request(&ctx, &mut req));
        assert!(result.unwrap().is_none());
    }
}

// ============================================================================
// Multiple Extractors Combination Tests
// ============================================================================

#[cfg(test)]
mod combination_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 88888)
    }

    #[test]
    fn path_and_query_together() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct QueryParams {
            limit: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/users/42");
        req.insert_extension(PathParams::from_pairs(vec![(
            "id".to_string(),
            "42".to_string(),
        )]));
        req.set_query(Some("limit=10".to_string()));

        // Extract path
        let path_result = futures_executor::block_on(Path::<i64>::from_request(&ctx, &mut req));
        let Path(user_id) = path_result.unwrap();
        assert_eq!(user_id, 42);

        // Extract query
        let query_result =
            futures_executor::block_on(Query::<QueryParams>::from_request(&ctx, &mut req));
        let Query(params) = query_result.unwrap();
        assert_eq!(params.limit, 10);
    }

    #[test]
    fn json_body_and_path() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct CreateItem {
            name: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/categories/5/items");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"{\"name\": \"Widget\"}".to_vec()));
        req.insert_extension(PathParams::from_pairs(vec![(
            "cat_id".to_string(),
            "5".to_string(),
        )]));

        // Extract path first (doesn't consume body)
        let path_result = futures_executor::block_on(Path::<i64>::from_request(&ctx, &mut req));
        let Path(cat_id) = path_result.unwrap();
        assert_eq!(cat_id, 5);

        // Extract JSON body
        let json_result =
            futures_executor::block_on(Json::<CreateItem>::from_request(&ctx, &mut req));
        let Json(item) = json_result.unwrap();
        assert_eq!(item.name, "Widget");
    }

    #[test]
    fn state_and_query() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct SearchParams {
            q: String,
        }

        #[derive(Clone, PartialEq, Debug)]
        struct Config {
            max_results: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/search");
        req.set_query(Some("q=hello".to_string()));
        let app_state = AppState::new().with(Config { max_results: 100 });
        req.insert_extension(app_state);

        // Extract state
        let state_result =
            futures_executor::block_on(State::<Config>::from_request(&ctx, &mut req));
        let State(config) = state_result.unwrap();
        assert_eq!(config.max_results, 100);

        // Extract query
        let query_result =
            futures_executor::block_on(Query::<SearchParams>::from_request(&ctx, &mut req));
        let Query(params) = query_result.unwrap();
        assert_eq!(params.q, "hello");
    }

    #[test]
    fn multiple_path_params_with_struct() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct CommentPath {
            post_id: i64,
            comment_id: i64,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/posts/123/comments/456");
        req.insert_extension(PathParams::from_pairs(vec![
            ("post_id".to_string(), "123".to_string()),
            ("comment_id".to_string(), "456".to_string()),
        ]));

        let result = futures_executor::block_on(Path::<CommentPath>::from_request(&ctx, &mut req));
        let Path(path) = result.unwrap();
        assert_eq!(path.post_id, 123);
        assert_eq!(path.comment_id, 456);
    }

    #[test]
    fn optional_mixed_with_required() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct OptionalParams {
            page: Option<i32>,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/users/42");
        req.insert_extension(PathParams::from_pairs(vec![(
            "id".to_string(),
            "42".to_string(),
        )]));

        // Required path - should succeed
        let path_result = futures_executor::block_on(Path::<i64>::from_request(&ctx, &mut req));
        let Path(id) = path_result.unwrap();
        assert_eq!(id, 42);

        // Optional query - should return default None
        let query_result =
            futures_executor::block_on(Query::<OptionalParams>::from_request(&ctx, &mut req));
        let Query(params) = query_result.unwrap();
        assert_eq!(params.page, None);
    }

    #[test]
    fn request_context_extraction() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/");

        let result = futures_executor::block_on(RequestContext::from_request(&ctx, &mut req));
        let extracted_ctx = result.unwrap();
        assert_eq!(extracted_ctx.request_id(), ctx.request_id());
    }

    #[test]
    fn triple_extraction_path_query_state() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct QueryFilter {
            status: String,
        }

        #[derive(Clone)]
        struct DbPool {
            connection_count: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/projects/99/tasks");
        req.insert_extension(PathParams::from_pairs(vec![(
            "project_id".to_string(),
            "99".to_string(),
        )]));
        req.set_query(Some("status=active".to_string()));
        let app_state = AppState::new().with(DbPool {
            connection_count: 10,
        });
        req.insert_extension(app_state);

        // Path
        let Path(project_id): Path<i32> =
            futures_executor::block_on(Path::<i32>::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(project_id, 99);

        // Query
        let Query(filter): Query<QueryFilter> =
            futures_executor::block_on(Query::<QueryFilter>::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(filter.status, "active");

        // State
        let State(pool): State<DbPool> =
            futures_executor::block_on(State::<DbPool>::from_request(&ctx, &mut req)).unwrap();
        assert_eq!(pool.connection_count, 10);
    }
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[cfg(test)]
mod edge_case_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 77777)
    }

    // --- Unicode and Special Characters ---

    #[test]
    fn json_with_unicode() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Data {
            name: String,
            emoji: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(
            r#"{"name": "日本語", "emoji": "🎉🚀"}"#.as_bytes().to_vec(),
        ));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        let Json(data) = result.unwrap();
        assert_eq!(data.name, "日本語");
        assert_eq!(data.emoji, "🎉🚀");
    }

    #[test]
    fn query_with_unicode_percent_encoded() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Search {
            q: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/search");
        // "こんにちは" (hello in Japanese), percent-encoded
        req.set_query(Some(
            "q=%E3%81%93%E3%82%93%E3%81%AB%E3%81%A1%E3%81%AF".to_string(),
        ));

        let result = futures_executor::block_on(Query::<Search>::from_request(&ctx, &mut req));
        let Query(search) = result.unwrap();
        assert_eq!(search.q, "こんにちは");
    }

    #[test]
    fn path_with_unicode() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/users/用户123");
        req.insert_extension(PathParams::from_pairs(vec![(
            "name".to_string(),
            "用户123".to_string(),
        )]));

        let result = futures_executor::block_on(Path::<String>::from_request(&ctx, &mut req));
        let Path(name) = result.unwrap();
        assert_eq!(name, "用户123");
    }

    // --- Boundary Values ---

    #[test]
    fn path_max_i64() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items/9223372036854775807");
        req.insert_extension(PathParams::from_pairs(vec![(
            "id".to_string(),
            "9223372036854775807".to_string(),
        )]));

        let result = futures_executor::block_on(Path::<i64>::from_request(&ctx, &mut req));
        let Path(id) = result.unwrap();
        assert_eq!(id, i64::MAX);
    }

    #[test]
    fn path_min_i64() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items/-9223372036854775808");
        req.insert_extension(PathParams::from_pairs(vec![(
            "id".to_string(),
            "-9223372036854775808".to_string(),
        )]));

        let result = futures_executor::block_on(Path::<i64>::from_request(&ctx, &mut req));
        let Path(id) = result.unwrap();
        assert_eq!(id, i64::MIN);
    }

    #[test]
    fn path_overflow_i64_fails() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items/9223372036854775808");
        req.insert_extension(PathParams::from_pairs(vec![(
            "id".to_string(),
            "9223372036854775808".to_string(), // i64::MAX + 1
        )]));

        let result = futures_executor::block_on(Path::<i64>::from_request(&ctx, &mut req));
        assert!(result.is_err());
    }

    #[test]
    fn query_with_empty_value() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Params {
            key: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        req.set_query(Some("key=".to_string()));

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.key, "");
    }

    #[test]
    fn query_with_only_key_no_equals() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Params {
            flag: Option<String>,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        req.set_query(Some("flag".to_string()));

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        // Key without = should have empty string value
        assert_eq!(params.flag, Some(String::new()));
    }

    #[test]
    fn json_empty_object() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Empty {}

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"{}".to_vec()));

        let result = futures_executor::block_on(Json::<Empty>::from_request(&ctx, &mut req));
        assert!(result.is_ok());
    }

    #[test]
    fn json_with_null_field() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Data {
            value: Option<i32>,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"{\"value\": null}".to_vec()));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        let Json(data) = result.unwrap();
        assert_eq!(data.value, None);
    }

    #[test]
    fn json_with_nested_objects() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Address {
            city: String,
            zip: String,
        }

        #[derive(Deserialize, PartialEq, Debug)]
        struct User {
            name: String,
            address: Address,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(
            b"{\"name\": \"Alice\", \"address\": {\"city\": \"NYC\", \"zip\": \"10001\"}}".to_vec(),
        ));

        let result = futures_executor::block_on(Json::<User>::from_request(&ctx, &mut req));
        let Json(user) = result.unwrap();
        assert_eq!(user.name, "Alice");
        assert_eq!(user.address.city, "NYC");
        assert_eq!(user.address.zip, "10001");
    }

    #[test]
    fn json_with_array() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Data {
            items: Vec<i32>,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"{\"items\": [1, 2, 3, 4, 5]}".to_vec()));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        let Json(data) = result.unwrap();
        assert_eq!(data.items, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn path_with_special_chars() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/files/my-file_v2.txt");
        req.insert_extension(PathParams::from_pairs(vec![(
            "filename".to_string(),
            "my-file_v2.txt".to_string(),
        )]));

        let result = futures_executor::block_on(Path::<String>::from_request(&ctx, &mut req));
        let Path(filename) = result.unwrap();
        assert_eq!(filename, "my-file_v2.txt");
    }

    #[test]
    fn query_with_special_chars_encoded() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Params {
            value: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        // Encoded: "hello world & more"
        req.set_query(Some("value=hello%20world%20%26%20more".to_string()));

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.value, "hello world & more");
    }

    #[test]
    fn query_multiple_values_same_key() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Params {
            tags: Vec<String>,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        req.set_query(Some("tags=a&tags=b&tags=c".to_string()));

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.tags, vec!["a", "b", "c"]);
    }

    #[test]
    fn path_empty_string() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/items//details");
        req.insert_extension(PathParams::from_pairs(vec![(
            "id".to_string(),
            String::new(),
        )]));

        let result = futures_executor::block_on(Path::<String>::from_request(&ctx, &mut req));
        let Path(id) = result.unwrap();
        assert_eq!(id, "");
    }

    #[test]
    fn json_with_escaped_quotes() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Data {
            message: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(
            b"{\"message\": \"He said \\\"hello\\\"\"}".to_vec(),
        ));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        let Json(data) = result.unwrap();
        assert_eq!(data.message, "He said \"hello\"");
    }

    #[test]
    fn query_with_plus_as_space() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Params {
            q: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/search");
        req.set_query(Some("q=hello+world".to_string()));

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        assert_eq!(params.q, "hello world");
    }
}

// ============================================================================
// Security Tests
// ============================================================================

#[cfg(test)]
mod security_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 66666)
    }

    #[test]
    fn json_payload_size_limit() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            content: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());

        // Create payload larger than DEFAULT_JSON_LIMIT (1MB)
        let large_content = "x".repeat(DEFAULT_JSON_LIMIT + 100);
        let body = format!("{{\"content\": \"{large_content}\"}}");
        req.set_body(Body::Bytes(body.into_bytes()));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(JsonExtractError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn json_deeply_nested_object() {
        use serde::Deserialize;

        // Deeply nested structure
        #[derive(Deserialize)]
        struct Level1 {
            #[allow(dead_code)]
            l2: Level2,
        }
        #[derive(Deserialize)]
        struct Level2 {
            #[allow(dead_code)]
            l3: Level3,
        }
        #[derive(Deserialize)]
        struct Level3 {
            #[allow(dead_code)]
            l4: Level4,
        }
        #[derive(Deserialize)]
        struct Level4 {
            #[allow(dead_code)]
            value: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(
            b"{\"l2\":{\"l3\":{\"l4\":{\"value\":42}}}}".to_vec(),
        ));

        let result = futures_executor::block_on(Json::<Level1>::from_request(&ctx, &mut req));
        assert!(result.is_ok());
    }

    #[test]
    fn query_injection_attempt_escaped() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Params {
            name: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        // SQL injection attempt - should be treated as literal string
        req.set_query(Some(
            "name=Robert%27%3B%20DROP%20TABLE%20users%3B--".to_string(),
        ));

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        // The value should be preserved as-is (decoded)
        assert_eq!(params.name, "Robert'; DROP TABLE users;--");
    }

    #[test]
    fn path_traversal_attempt() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/files/../../../etc/passwd");
        req.insert_extension(PathParams::from_pairs(vec![(
            "path".to_string(),
            "../../../etc/passwd".to_string(),
        )]));

        let result = futures_executor::block_on(Path::<String>::from_request(&ctx, &mut req));
        let Path(path) = result.unwrap();
        // Path is extracted as-is - application must validate
        assert_eq!(path, "../../../etc/passwd");
    }

    #[test]
    fn json_with_script_tag_xss() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Data {
            comment: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(
            b"{\"comment\": \"<script>alert('xss')</script>\"}".to_vec(),
        ));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        let Json(data) = result.unwrap();
        // XSS content is preserved as-is - application must sanitize on output
        assert_eq!(data.comment, "<script>alert('xss')</script>");
    }

    #[test]
    fn json_content_type_case_insensitive() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Data {
            value: i32,
        }

        // Test various case combinations
        for content_type in &[
            "APPLICATION/JSON",
            "Application/Json",
            "application/JSON",
            "APPLICATION/json",
        ] {
            let ctx = test_context();
            let mut req = Request::new(Method::Post, "/test");
            req.headers_mut()
                .insert("content-type", content_type.as_bytes().to_vec());
            req.set_body(Body::Bytes(b"{\"value\": 42}".to_vec()));

            let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
            assert!(result.is_ok(), "Failed for content-type: {}", content_type);
        }
    }

    #[test]
    fn json_wrong_content_type_variants() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            value: i32,
        }

        // These should all be rejected
        for content_type in &[
            "text/json",
            "text/plain",
            "application/xml",
            "application/x-json",
        ] {
            let ctx = test_context();
            let mut req = Request::new(Method::Post, "/test");
            req.headers_mut()
                .insert("content-type", content_type.as_bytes().to_vec());
            req.set_body(Body::Bytes(b"{\"value\": 42}".to_vec()));

            let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
            assert!(
                matches!(result, Err(JsonExtractError::UnsupportedMediaType { .. })),
                "Should reject content-type: {}",
                content_type
            );
        }
    }

    #[test]
    fn query_null_byte_handling() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Params {
            name: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/test");
        // Percent-encoded null byte
        req.set_query(Some("name=test%00value".to_string()));

        let result = futures_executor::block_on(Query::<Params>::from_request(&ctx, &mut req));
        let Query(params) = result.unwrap();
        // Null byte should be decoded
        assert_eq!(params.name, "test\0value");
    }

    #[test]
    fn path_with_null_byte() {
        let ctx = test_context();
        let mut req = Request::new(Method::Get, "/files/test");
        req.insert_extension(PathParams::from_pairs(vec![(
            "filename".to_string(),
            "test\0.txt".to_string(),
        )]));

        let result = futures_executor::block_on(Path::<String>::from_request(&ctx, &mut req));
        let Path(filename) = result.unwrap();
        assert_eq!(filename, "test\0.txt");
    }

    #[test]
    fn json_number_precision() {
        use serde::Deserialize;

        #[derive(Deserialize, PartialEq, Debug)]
        struct Data {
            big_int: i64,
            float_val: f64,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        // Large number that fits in i64 but not in f64 without precision loss
        req.set_body(Body::Bytes(
            b"{\"big_int\": 9007199254740993, \"float_val\": 3.141592653589793}".to_vec(),
        ));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        let Json(data) = result.unwrap();
        assert_eq!(data.big_int, 9007199254740993_i64);
        assert!((data.float_val - std::f64::consts::PI).abs() < 0.0000001);
    }

    // =========================================================================
    // Json<T> IntoResponse tests
    // =========================================================================

    #[test]
    fn json_into_response_serializes_struct() {
        use serde::Serialize;

        #[derive(Serialize)]
        struct User {
            name: String,
            age: u32,
        }

        let user = User {
            name: "Alice".to_string(),
            age: 30,
        };
        let json = Json(user);
        let response = json.into_response();

        assert_eq!(response.status().as_u16(), 200);

        // Check content-type header
        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());
        assert_eq!(content_type, Some("application/json".to_string()));

        // Check body content
        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(parsed["name"], "Alice");
            assert_eq!(parsed["age"], 30);
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn json_into_response_serializes_primitive() {
        let json = Json(42i32);
        let response = json.into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: i32 = serde_json::from_slice(bytes).unwrap();
            assert_eq!(parsed, 42);
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn json_into_response_serializes_array() {
        let json = Json(vec!["a", "b", "c"]);
        let response = json.into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: Vec<String> = serde_json::from_slice(bytes).unwrap();
            assert_eq!(parsed, vec!["a", "b", "c"]);
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn json_into_response_serializes_hashmap() {
        use std::collections::HashMap;

        let mut map = HashMap::new();
        map.insert("key1", "value1");
        map.insert("key2", "value2");

        let json = Json(map);
        let response = json.into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: HashMap<String, String> = serde_json::from_slice(bytes).unwrap();
            assert_eq!(parsed.get("key1"), Some(&"value1".to_string()));
            assert_eq!(parsed.get("key2"), Some(&"value2".to_string()));
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn json_into_response_handles_null() {
        let json = Json(Option::<String>::None);
        let response = json.into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let content = String::from_utf8_lossy(bytes);
            assert_eq!(content, "null");
        } else {
            panic!("Expected Bytes body");
        }
    }
}

// ============================================================================
// Body Size Limit Enforcement Tests (bd-dl14)
// ============================================================================

#[cfg(test)]
mod body_size_limit_tests {
    use super::*;
    use crate::request::{Body, Method};
    use crate::response::{ResponseBody, StatusCode};

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 1)
    }

    fn test_context_with_limit(limit: usize) -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::with_body_limit(cx, 1, limit)
    }

    // ---- Default limit constants ----

    #[test]
    fn default_constants_match_expected_values() {
        assert_eq!(DEFAULT_JSON_LIMIT, 1024 * 1024); // 1MB
        assert_eq!(DEFAULT_FORM_LIMIT, 1024 * 1024); // 1MB
        assert_eq!(DEFAULT_RAW_BODY_LIMIT, 2 * 1024 * 1024); // 2MB
        assert_eq!(crate::DEFAULT_MAX_BODY_SIZE, 1024 * 1024); // 1MB
    }

    // ---- JSON body limit tests ----

    #[test]
    fn json_body_under_limit_accepted() {
        use serde::Deserialize;

        #[derive(Deserialize, Debug)]
        struct Msg {
            text: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/api");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"{\"text\":\"hello\"}".to_vec()));

        let result = futures_executor::block_on(Json::<Msg>::from_request(&ctx, &mut req));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0.text, "hello");
    }

    #[test]
    fn json_body_over_default_limit_rejected() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            content: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/api");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());

        // Body exceeds the 1MB default limit
        let large = "x".repeat(crate::DEFAULT_MAX_BODY_SIZE + 1);
        let body = format!("{{\"content\":\"{}\"}}", large);
        req.set_body(Body::Bytes(body.into_bytes()));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(JsonExtractError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn json_body_exactly_at_limit_accepted() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            content: String,
        }

        // Use a custom small limit for easier testing
        let ctx = test_context_with_limit(100);
        let mut req = Request::new(Method::Post, "/api");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());

        // Build a body that is exactly 100 bytes
        // {"content":"xxx..."} where content is padded to make total = 100
        let prefix = b"{\"content\":\"";
        let suffix = b"\"}";
        let content_len = 100 - prefix.len() - suffix.len();
        let content: String = "a".repeat(content_len);
        let body = format!("{{\"content\":\"{}\"}}", content);
        assert_eq!(body.len(), 100);

        req.set_body(Body::Bytes(body.into_bytes()));
        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        assert!(result.is_ok(), "Body exactly at limit should be accepted");
    }

    #[test]
    fn json_body_one_byte_over_limit_rejected() {
        use serde::Deserialize;

        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct Data {
            content: String,
        }

        let ctx = test_context_with_limit(100);
        let mut req = Request::new(Method::Post, "/api");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());

        // 101 bytes body
        let prefix = b"{\"content\":\"";
        let suffix = b"\"}";
        let content_len = 101 - prefix.len() - suffix.len();
        let content: String = "a".repeat(content_len);
        let body = format!("{{\"content\":\"{}\"}}", content);
        assert_eq!(body.len(), 101);

        req.set_body(Body::Bytes(body.into_bytes()));
        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        match result {
            Err(JsonExtractError::PayloadTooLarge { size, limit }) => {
                assert_eq!(size, 101);
                assert_eq!(limit, 100);
            }
            other => panic!("Expected PayloadTooLarge, got {:?}", other),
        }
    }

    #[test]
    fn json_custom_body_limit_via_context() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            val: String,
        }

        // Small limit: 50 bytes
        let ctx = test_context_with_limit(50);
        let mut req = Request::new(Method::Post, "/api");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());

        // Body exceeding 50 bytes
        let padding = "x".repeat(60);
        let body = format!("{{\"val\":\"{}\"}}", padding);
        assert!(
            body.len() > 50,
            "Body is {} bytes, expected > 50",
            body.len()
        );
        req.set_body(Body::Bytes(body.into_bytes()));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(JsonExtractError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn json_large_custom_limit_accepts_big_body() {
        use serde::Deserialize;

        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct Data {
            content: String,
        }

        // 10MB limit
        let ctx = test_context_with_limit(10 * 1024 * 1024);
        let mut req = Request::new(Method::Post, "/api");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());

        // 2MB body (under the 10MB custom limit)
        let large = "x".repeat(2 * 1024 * 1024);
        let body = format!("{{\"content\":\"{}\"}}", large);
        req.set_body(Body::Bytes(body.into_bytes()));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        assert!(result.is_ok(), "Body under custom limit should be accepted");
    }

    #[test]
    fn json_empty_body_accepted() {
        use serde::Deserialize;

        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct Data {
            #[serde(default)]
            val: i32,
        }

        let ctx = test_context_with_limit(10);
        let mut req = Request::new(Method::Post, "/api");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Empty);

        // Empty body parses to empty bytes, under any limit
        // But it will fail deserialization (not a size issue)
        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        // Empty body means 0 bytes which is under limit, but deserialization fails
        match result {
            Err(JsonExtractError::DeserializeError { .. }) => {} // expected
            Err(JsonExtractError::PayloadTooLarge { .. }) => {
                panic!("Empty body should not trigger size limit")
            }
            Ok(_) => {} // Also fine if serde accepts empty
            other => panic!("Unexpected result: {:?}", other),
        }
    }

    #[test]
    fn json_payload_too_large_error_response_is_413() {
        let err = JsonExtractError::PayloadTooLarge {
            size: 2_000_000,
            limit: 1_000_000,
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(response.status().as_u16(), 413);
    }

    // ---- Form body limit tests ----

    #[test]
    fn form_body_under_limit_accepted() {
        use serde::Deserialize;

        #[derive(Deserialize, Debug)]
        struct Login {
            user: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/login");
        req.headers_mut().insert(
            "content-type",
            b"application/x-www-form-urlencoded".to_vec(),
        );
        req.set_body(Body::Bytes(b"user=alice".to_vec()));

        let result = futures_executor::block_on(Form::<Login>::from_request(&ctx, &mut req));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0.user, "alice");
    }

    #[test]
    fn form_body_over_limit_rejected() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            field: String,
        }

        let ctx = test_context(); // default 1MB limit
        let mut req = Request::new(Method::Post, "/submit");
        req.headers_mut().insert(
            "content-type",
            b"application/x-www-form-urlencoded".to_vec(),
        );

        // Build form body larger than 1MB
        let large_value = "x".repeat(crate::DEFAULT_MAX_BODY_SIZE + 1);
        let body = format!("field={}", large_value);
        req.set_body(Body::Bytes(body.into_bytes()));

        let result = futures_executor::block_on(Form::<Data>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(FormExtractError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn form_custom_limit_via_context() {
        use serde::Deserialize;

        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct Data {
            field: String,
        }

        let ctx = test_context_with_limit(20);
        let mut req = Request::new(Method::Post, "/submit");
        req.headers_mut().insert(
            "content-type",
            b"application/x-www-form-urlencoded".to_vec(),
        );

        // 30-byte body exceeds 20-byte limit
        let body = "field=abcdefghijklmnopqrstuv";
        assert!(body.len() > 20);
        req.set_body(Body::Bytes(body.as_bytes().to_vec()));

        let result = futures_executor::block_on(Form::<Data>::from_request(&ctx, &mut req));
        match result {
            Err(FormExtractError::PayloadTooLarge { size, limit }) => {
                assert_eq!(limit, 20);
                assert!(size > 20);
            }
            other => panic!("Expected PayloadTooLarge, got {:?}", other),
        }
    }

    #[test]
    fn form_payload_too_large_error_response_is_413() {
        let err = FormExtractError::PayloadTooLarge {
            size: 2_000_000,
            limit: 1_000_000,
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    // ---- Raw body (Bytes) limit tests ----

    #[test]
    fn bytes_body_under_limit_accepted() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.set_body(Body::Bytes(b"small payload".to_vec()));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_slice(), b"small payload");
    }

    #[test]
    fn bytes_body_over_default_limit_rejected() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        let large_body = vec![0u8; DEFAULT_RAW_BODY_LIMIT + 1];
        req.set_body(Body::Bytes(large_body));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        match result {
            Err(RawBodyError::PayloadTooLarge { size, limit }) => {
                assert_eq!(size, DEFAULT_RAW_BODY_LIMIT + 1);
                assert_eq!(limit, DEFAULT_RAW_BODY_LIMIT);
            }
            other => panic!("Expected PayloadTooLarge, got {:?}", other),
        }
    }

    #[test]
    fn bytes_custom_limit_via_extension() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.insert_extension(RawBodyConfig::new().limit(50));
        req.set_body(Body::Bytes(vec![0u8; 80]));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        match result {
            Err(RawBodyError::PayloadTooLarge { size, limit }) => {
                assert_eq!(size, 80);
                assert_eq!(limit, 50);
            }
            other => panic!("Expected PayloadTooLarge, got {:?}", other),
        }
    }

    #[test]
    fn bytes_custom_limit_accepts_body_under() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.insert_extension(RawBodyConfig::new().limit(200));
        req.set_body(Body::Bytes(vec![0u8; 100]));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 100);
    }

    #[test]
    fn bytes_empty_body_always_accepted() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.insert_extension(RawBodyConfig::new().limit(0));
        req.set_body(Body::Empty);

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn bytes_payload_too_large_error_response_is_413() {
        let err = RawBodyError::PayloadTooLarge {
            size: 5_000_000,
            limit: 2_000_000,
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    // ---- StringBody limit tests ----

    #[test]
    fn string_body_over_limit_rejected() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/text");
        req.insert_extension(RawBodyConfig::new().limit(10));
        req.set_body(Body::Bytes(b"this is longer than ten bytes".to_vec()));

        let result = futures_executor::block_on(StringBody::from_request(&ctx, &mut req));
        assert!(matches!(result, Err(RawBodyError::PayloadTooLarge { .. })));
    }

    #[test]
    fn string_body_under_limit_accepted() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/text");
        req.insert_extension(RawBodyConfig::new().limit(100));
        req.set_body(Body::Bytes(b"short".to_vec()));

        let result = futures_executor::block_on(StringBody::from_request(&ctx, &mut req));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "short");
    }

    // ---- RawBodyConfig tests ----

    #[test]
    fn raw_body_config_default() {
        let config = RawBodyConfig::default();
        assert_eq!(config.get_limit(), DEFAULT_RAW_BODY_LIMIT);
        assert_eq!(config.get_limit(), 2 * 1024 * 1024);
    }

    #[test]
    fn raw_body_config_builder() {
        let config = RawBodyConfig::new().limit(500);
        assert_eq!(config.get_limit(), 500);
    }

    // ---- JsonConfig tests ----

    #[test]
    fn json_config_default() {
        let config = JsonConfig::default();
        assert_eq!(config.get_limit(), DEFAULT_JSON_LIMIT);
    }

    #[test]
    fn json_config_builder() {
        let config = JsonConfig::new().limit(2048);
        assert_eq!(config.get_limit(), 2048);
    }

    // ---- FormConfig tests ----

    #[test]
    fn form_config_default() {
        let config = FormConfig::default();
        assert_eq!(config.get_limit(), DEFAULT_FORM_LIMIT);
    }

    #[test]
    fn form_config_builder() {
        let config = FormConfig::new().limit(4096);
        assert_eq!(config.get_limit(), 4096);
    }

    // ---- Cross-extractor limit behavior ----

    #[test]
    fn json_uses_context_body_limit_not_json_config() {
        // Verify Json extractor uses ctx.max_body_size(), not JsonConfig.limit
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            val: String,
        }

        // Context with 50 byte limit
        let ctx = test_context_with_limit(50);
        let mut req = Request::new(Method::Post, "/api");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());

        // Body of 60 bytes
        let body = "{\"val\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}";
        assert!(body.len() > 50);
        req.set_body(Body::Bytes(body.as_bytes().to_vec()));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        assert!(
            matches!(result, Err(JsonExtractError::PayloadTooLarge { .. })),
            "Json should use context body limit (50), not default"
        );
    }

    #[test]
    fn form_uses_context_body_limit() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            val: String,
        }

        // Context with 30 byte limit
        let ctx = test_context_with_limit(30);
        let mut req = Request::new(Method::Post, "/form");
        req.headers_mut().insert(
            "content-type",
            b"application/x-www-form-urlencoded".to_vec(),
        );

        // Body of 35 bytes
        let body = "val=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert!(body.len() > 30);
        req.set_body(Body::Bytes(body.as_bytes().to_vec()));

        let result = futures_executor::block_on(Form::<Data>::from_request(&ctx, &mut req));
        assert!(
            matches!(result, Err(FormExtractError::PayloadTooLarge { .. })),
            "Form should use context body limit"
        );
    }

    #[test]
    fn bytes_uses_extension_config_not_context_limit() {
        // Bytes uses RawBodyConfig extension, not ctx.max_body_size()
        let ctx = test_context_with_limit(10); // Small context limit
        let mut req = Request::new(Method::Post, "/upload");
        // RawBodyConfig with a large limit
        req.insert_extension(RawBodyConfig::new().limit(1000));
        req.set_body(Body::Bytes(vec![0u8; 500]));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        // Should use extension config (1000), not context limit (10)
        assert!(
            result.is_ok(),
            "Bytes should use RawBodyConfig from extension, not context limit"
        );
    }

    #[test]
    fn bytes_without_config_uses_default_raw_limit() {
        // When no RawBodyConfig extension, Bytes uses DEFAULT_RAW_BODY_LIMIT
        let ctx = test_context_with_limit(10); // Small context limit
        let mut req = Request::new(Method::Post, "/upload");
        // No extension set, body under DEFAULT_RAW_BODY_LIMIT (2MB)
        req.set_body(Body::Bytes(vec![0u8; 500]));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        // Uses DEFAULT_RAW_BODY_LIMIT (2MB), not context limit (10)
        assert!(result.is_ok());
    }

    // ---- Error detail tests ----

    #[test]
    fn json_error_response_body_contains_size_info() {
        let err = JsonExtractError::PayloadTooLarge {
            size: 2_500_000,
            limit: 1_048_576,
        };
        let response = err.into_response();
        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let body = std::str::from_utf8(bytes).unwrap();
            assert!(
                body.contains("2500000"),
                "Error should contain actual size, got: {}",
                body
            );
            assert!(
                body.contains("1048576"),
                "Error should contain limit, got: {}",
                body
            );
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn form_error_response_body_contains_size_info() {
        let err = FormExtractError::PayloadTooLarge {
            size: 3_000_000,
            limit: 1_048_576,
        };
        let response = err.into_response();
        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let body = std::str::from_utf8(bytes).unwrap();
            assert!(
                body.contains("3000000"),
                "Error should contain actual size, got: {}",
                body
            );
            assert!(
                body.contains("1048576"),
                "Error should contain limit, got: {}",
                body
            );
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn raw_body_error_response_contains_size_info() {
        let err = RawBodyError::PayloadTooLarge {
            size: 5_000_000,
            limit: 2_097_152,
        };
        let response = err.into_response();
        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let body = std::str::from_utf8(bytes).unwrap();
            assert!(
                body.contains("5000000"),
                "Error should contain actual size, got: {}",
                body
            );
            assert!(
                body.contains("2097152"),
                "Error should contain limit, got: {}",
                body
            );
        } else {
            panic!("Expected Bytes body");
        }
    }

    // ---- Streaming body rejection tests ----

    #[test]
    fn json_streaming_body_rejected() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            val: i32,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/api");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());

        let stream = asupersync::stream::iter(
            vec![Ok(b"chunk".to_vec())]
                .into_iter()
                .map(|r: Result<Vec<u8>, crate::request::RequestBodyStreamError>| r),
        );
        req.set_body(Body::streaming(stream));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(JsonExtractError::StreamingNotSupported)
        ));
    }

    #[test]
    fn form_streaming_body_rejected() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            field: String,
        }

        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/form");
        req.headers_mut().insert(
            "content-type",
            b"application/x-www-form-urlencoded".to_vec(),
        );

        let stream = asupersync::stream::iter(
            vec![Ok(b"chunk".to_vec())]
                .into_iter()
                .map(|r: Result<Vec<u8>, crate::request::RequestBodyStreamError>| r),
        );
        req.set_body(Body::streaming(stream));

        let result = futures_executor::block_on(Form::<Data>::from_request(&ctx, &mut req));
        assert!(matches!(
            result,
            Err(FormExtractError::StreamingNotSupported)
        ));
    }

    #[test]
    fn bytes_streaming_body_rejected() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");

        let stream = asupersync::stream::iter(
            vec![Ok(b"chunk".to_vec())]
                .into_iter()
                .map(|r: Result<Vec<u8>, crate::request::RequestBodyStreamError>| r),
        );
        req.set_body(Body::streaming(stream));

        let result = futures_executor::block_on(Bytes::from_request(&ctx, &mut req));
        assert!(matches!(result, Err(RawBodyError::StreamingNotSupported)));
    }
}
