//! Request extraction traits and extractors.
//!
//! This module provides the [`FromRequest`] trait and common extractors
//! like [`Json`] and [`Path`] for parsing request data.

use crate::context::RequestContext;
use crate::error::{HttpError, ValidationError, ValidationErrors};
use crate::multipart;
use crate::request::{Body, Request, RequestBodyStreamError};
use crate::response::IntoResponse;
use serde::de::{
    self, DeserializeOwned, Deserializer, IntoDeserializer, MapAccess, SeqAccess, Visitor,
};
use std::fmt;
use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::task::Context;

async fn collect_body_limited(
    ctx: &RequestContext,
    body: Body,
    limit: usize,
) -> Result<Vec<u8>, RequestBodyStreamError> {
    match body {
        Body::Empty => Ok(Vec::new()),
        Body::Bytes(b) => {
            if b.len() > limit {
                Err(RequestBodyStreamError::TooLarge {
                    received: b.len(),
                    max: limit,
                })
            } else {
                Ok(b)
            }
        }
        Body::Stream {
            stream,
            content_length,
        } => {
            let mut stream = stream.into_inner().unwrap_or_else(|e| e.into_inner());
            if let Some(n) = content_length {
                if n > limit {
                    return Err(RequestBodyStreamError::TooLarge {
                        received: n,
                        max: limit,
                    });
                }
            }

            // Poll the stream without requiring Unpin.
            let mut out = Vec::with_capacity(content_length.unwrap_or(0).min(limit));
            let mut seen = 0usize;
            loop {
                let next =
                    std::future::poll_fn(|cx: &mut Context<'_>| stream.as_mut().poll_next(cx))
                        .await;
                let Some(chunk) = next else {
                    break;
                };
                let chunk = chunk?;
                seen = seen.saturating_add(chunk.len());
                if seen > limit {
                    return Err(RequestBodyStreamError::TooLarge {
                        received: seen,
                        max: limit,
                    });
                }
                out.extend_from_slice(&chunk);
                let _ = ctx.checkpoint();
            }
            Ok(out)
        }
    }
}

async fn parse_multipart_limited(
    ctx: &RequestContext,
    body: Body,
    limit: usize,
    parser: &multipart::MultipartParser,
) -> Result<Vec<multipart::Part>, MultipartExtractError> {
    fn map_parser_error(err: multipart::MultipartError) -> MultipartExtractError {
        match err {
            multipart::MultipartError::FileTooLarge { size, max }
            | multipart::MultipartError::TotalTooLarge { size, max } => {
                MultipartExtractError::PayloadTooLarge { size, limit: max }
            }
            multipart::MultipartError::Io { detail } => {
                MultipartExtractError::ReadError { message: detail }
            }
            other => MultipartExtractError::BadRequest {
                message: other.to_string(),
            },
        }
    }

    fn map_stream_error(err: RequestBodyStreamError) -> MultipartExtractError {
        match err {
            RequestBodyStreamError::TooLarge { received, max } => {
                MultipartExtractError::PayloadTooLarge {
                    size: received,
                    limit: max,
                }
            }
            RequestBodyStreamError::ConnectionClosed => MultipartExtractError::BadRequest {
                message: RequestBodyStreamError::ConnectionClosed.to_string(),
            },
            RequestBodyStreamError::Io(message) => MultipartExtractError::ReadError { message },
        }
    }

    match body {
        Body::Empty => parser.parse(&[]).map_err(map_parser_error),
        Body::Bytes(bytes) => {
            if bytes.len() > limit {
                return Err(MultipartExtractError::PayloadTooLarge {
                    size: bytes.len(),
                    limit,
                });
            }
            parser.parse(&bytes).map_err(map_parser_error)
        }
        Body::Stream {
            stream,
            content_length,
        } => {
            let mut stream = stream.into_inner().unwrap_or_else(|e| e.into_inner());

            if let Some(n) = content_length {
                if n > limit {
                    return Err(MultipartExtractError::PayloadTooLarge { size: n, limit });
                }
            }

            let mut state = multipart::MultipartStreamState::default();
            let mut buffer = Vec::new();
            let mut parts = Vec::new();
            let mut seen = 0usize;

            loop {
                let next =
                    std::future::poll_fn(|cx: &mut Context<'_>| stream.as_mut().poll_next(cx))
                        .await;
                let Some(chunk) = next else {
                    break;
                };
                let chunk = chunk.map_err(map_stream_error)?;

                seen = seen.saturating_add(chunk.len());
                if seen > limit {
                    return Err(MultipartExtractError::PayloadTooLarge { size: seen, limit });
                }

                buffer.extend_from_slice(&chunk);
                let mut newly_parsed = parser
                    .parse_incremental(&mut buffer, &mut state, false)
                    .map_err(map_parser_error)?;
                parts.append(&mut newly_parsed);
                let _ = ctx.checkpoint();
            }

            let mut tail = parser
                .parse_incremental(&mut buffer, &mut state, true)
                .map_err(map_parser_error)?;
            parts.append(&mut tail);

            if !state.is_done() {
                return Err(MultipartExtractError::BadRequest {
                    message: RequestBodyStreamError::ConnectionClosed.to_string(),
                });
            }

            Ok(parts)
        }
    }
}

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

// ============================================================================
// Multipart Form Extractor
// ============================================================================

/// Error when multipart extraction fails.
#[derive(Debug)]
pub enum MultipartExtractError {
    /// Wrong or missing content type.
    UnsupportedMediaType { actual: Option<String> },
    /// Missing/invalid boundary or invalid multipart format.
    BadRequest { message: String },
    /// Payload too large.
    PayloadTooLarge { size: usize, limit: usize },
    /// Body stream read error.
    ReadError { message: String },
}

impl fmt::Display for MultipartExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMediaType { actual } => {
                if let Some(ct) = actual {
                    write!(f, "Expected Content-Type: multipart/form-data, got: {ct}")
                } else {
                    write!(
                        f,
                        "Missing Content-Type header, expected multipart/form-data"
                    )
                }
            }
            Self::BadRequest { message } => write!(f, "{message}"),
            Self::PayloadTooLarge { size, limit } => write!(
                f,
                "Request body too large: {size} bytes exceeds {limit} byte limit"
            ),
            Self::ReadError { message } => write!(f, "Failed to read request body: {message}"),
        }
    }
}

impl std::error::Error for MultipartExtractError {}

impl IntoResponse for MultipartExtractError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let (status, detail) = match self {
            Self::UnsupportedMediaType { actual: _ } => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, self.to_string())
            }
            Self::BadRequest { message } => (StatusCode::BAD_REQUEST, message),
            Self::PayloadTooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, self.to_string()),
            Self::ReadError { .. } => (StatusCode::BAD_REQUEST, self.to_string()),
        };

        let body = serde_json::json!({ "detail": detail });
        Response::with_status(status)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for multipart::MultipartForm {
    type Error = MultipartExtractError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let _ = ctx.checkpoint();

        let content_type = req
            .headers()
            .get("content-type")
            .and_then(|v| std::str::from_utf8(v).ok());
        let Some(ct) = content_type else {
            return Err(MultipartExtractError::UnsupportedMediaType { actual: None });
        };

        let ct = ct.trim();
        let main = ct.split(';').next().unwrap_or("").trim();
        if !main.eq_ignore_ascii_case("multipart/form-data") {
            return Err(MultipartExtractError::UnsupportedMediaType {
                actual: Some(ct.to_string()),
            });
        }

        let boundary =
            multipart::parse_boundary(ct).map_err(|e| MultipartExtractError::BadRequest {
                message: e.to_string(),
            })?;

        let multipart_config = multipart::MultipartConfig::default();
        let limit = multipart_config.get_max_total_size();
        let spool_threshold = multipart_config.get_spool_threshold();
        let parser = multipart::MultipartParser::new(&boundary, multipart_config);
        let parts = parse_multipart_limited(ctx, req.take_body(), limit, &parser).await?;

        Ok(multipart::MultipartForm::from_parts_with_spool_threshold(
            parts,
            spool_threshold,
        ))
    }
}

#[cfg(test)]
mod multipart_extractor_tests {
    use super::*;
    use crate::request::Method;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    #[test]
    fn multipart_extract_success() {
        let ctx = test_context();
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"field1\"\r\n",
            "\r\n",
            "value1\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "Hello\r\n",
            "------boundary--\r\n"
        );

        let mut req = Request::new(Method::Post, "/upload");
        req.headers_mut().insert(
            "content-type",
            format!("multipart/form-data; boundary={boundary}").into_bytes(),
        );
        req.set_body(Body::Bytes(body.as_bytes().to_vec()));

        let form =
            futures_executor::block_on(multipart::MultipartForm::from_request(&ctx, &mut req))
                .expect("multipart parse");
        assert_eq!(form.get_field("field1"), Some("value1"));
        let file = form.get_file("file").expect("file");
        assert_eq!(file.filename, "test.txt");
        assert_eq!(file.content_type, "text/plain");
        assert_eq!(file.bytes().expect("read upload bytes"), b"Hello".to_vec());
    }

    #[test]
    fn multipart_extract_wrong_content_type() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        req.set_body(Body::Bytes(b"{}".to_vec()));

        let err =
            futures_executor::block_on(multipart::MultipartForm::from_request(&ctx, &mut req))
                .unwrap_err();
        assert!(matches!(
            err,
            MultipartExtractError::UnsupportedMediaType { actual: Some(_) }
        ));
    }

    #[test]
    fn multipart_extract_missing_boundary_is_bad_request() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/upload");
        req.headers_mut()
            .insert("content-type", b"multipart/form-data".to_vec());
        req.set_body(Body::Bytes(b"".to_vec()));

        let err =
            futures_executor::block_on(multipart::MultipartForm::from_request(&ctx, &mut req))
                .unwrap_err();
        assert!(matches!(err, MultipartExtractError::BadRequest { .. }));
    }

    #[test]
    fn multipart_extract_streaming_body() {
        use asupersync::stream;

        let ctx = test_context();
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"field1\"\r\n",
            "\r\n",
            "value1\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "Hello stream\r\n",
            "------boundary--\r\n"
        )
        .as_bytes()
        .to_vec();

        let chunks: Vec<Result<Vec<u8>, RequestBodyStreamError>> =
            body.chunks(7).map(|chunk| Ok(chunk.to_vec())).collect();
        let stream = stream::iter(chunks);

        let mut req = Request::new(Method::Post, "/upload");
        req.headers_mut().insert(
            "content-type",
            format!("multipart/form-data; boundary={boundary}").into_bytes(),
        );
        req.set_body(Body::streaming(stream));

        let form =
            futures_executor::block_on(multipart::MultipartForm::from_request(&ctx, &mut req))
                .expect("multipart parse");
        assert_eq!(form.get_field("field1"), Some("value1"));
        let file = form.get_file("file").expect("file");
        assert_eq!(
            file.bytes().expect("read upload bytes"),
            b"Hello stream".to_vec()
        );
    }

    #[test]
    fn multipart_extract_file_too_large_maps_to_payload_too_large() {
        let ctx = test_context();
        let boundary = "----boundary";
        let oversized = vec![b'a'; multipart::DEFAULT_MAX_FILE_SIZE + 1];

        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"big.bin\"\r\n",
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(&oversized);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let mut req = Request::new(Method::Post, "/upload");
        req.headers_mut().insert(
            "content-type",
            format!("multipart/form-data; boundary={boundary}").into_bytes(),
        );
        req.set_body(Body::Bytes(body));

        let err =
            futures_executor::block_on(multipart::MultipartForm::from_request(&ctx, &mut req))
                .unwrap_err();
        assert!(matches!(err, MultipartExtractError::PayloadTooLarge { .. }));
        if let MultipartExtractError::PayloadTooLarge { size, limit } = err {
            assert!(size > limit);
            assert_eq!(limit, multipart::DEFAULT_MAX_FILE_SIZE);
        }
    }
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
    /// Failed to read the request body (stream error).
    ReadError {
        /// The body read error message.
        message: String,
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
            Self::ReadError { message } => write!(f, "Failed to read request body: {message}"),
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
            Self::ReadError { message } => HttpError::bad_request()
                .with_detail(format!("Failed to read request body: {message}"))
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
        }
    }
}

impl<T: DeserializeOwned> FromRequest for Json<T> {
    type Error = JsonExtractError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Check cancellation at start
        let _ = ctx.checkpoint();

        // Validate Content-Type
        let content_type = req
            .headers()
            .get("content-type")
            .and_then(|v| std::str::from_utf8(v).ok());

        let is_json = content_type.is_some_and(|ct| {
            let ct_lower = ct.to_ascii_lowercase();
            // Check for exact "application/json" possibly followed by parameters (";")
            // Reject near-miss types like "application/jsonl" or "application/json-seq"
            let base_type = ct_lower.split(';').next().unwrap_or("").trim();
            base_type == "application/json"
                || (base_type.starts_with("application/") && base_type.ends_with("+json"))
        });

        if !is_json {
            return Err(JsonExtractError::UnsupportedMediaType {
                actual: content_type.map(String::from),
            });
        }

        // Get body bytes
        let body = req.take_body();
        let limit = DEFAULT_JSON_LIMIT;
        let bytes = collect_body_limited(ctx, body, limit)
            .await
            .map_err(|e| match e {
                RequestBodyStreamError::TooLarge { received, .. } => {
                    JsonExtractError::PayloadTooLarge {
                        size: received,
                        limit,
                    }
                }
                other => JsonExtractError::ReadError {
                    message: other.to_string(),
                },
            })?;

        // Check cancellation before deserialization
        let _ = ctx.checkpoint();

        // Deserialize JSON
        serde_json::from_slice(&bytes)
            .map(Json)
            .map_err(|e| JsonExtractError::DeserializeError {
                message: e.to_string(),
                line: Some(e.line()),
                column: Some(e.column()),
            })
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
/// `/users/42/posts/99`, this contains:
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
// Pagination
// ============================================================================

/// Default page number used when `page` is not provided.
pub const DEFAULT_PAGE: u64 = 1;
/// Default items-per-page used when `per_page` is not provided.
pub const DEFAULT_PER_PAGE: u64 = 20;
/// Maximum allowed `per_page` value.
pub const MAX_PER_PAGE: u64 = 100;

/// Pagination extractor configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaginationConfig {
    default_page: u64,
    default_per_page: u64,
    max_per_page: u64,
}

impl Default for PaginationConfig {
    fn default() -> Self {
        Self {
            default_page: DEFAULT_PAGE,
            default_per_page: DEFAULT_PER_PAGE,
            max_per_page: MAX_PER_PAGE,
        }
    }
}

impl PaginationConfig {
    /// Create a config with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the default page.
    #[must_use]
    pub fn default_page(mut self, page: u64) -> Self {
        self.default_page = page;
        self
    }

    /// Set the default per-page.
    #[must_use]
    pub fn default_per_page(mut self, per_page: u64) -> Self {
        self.default_per_page = per_page;
        self
    }

    /// Set the maximum per-page.
    #[must_use]
    pub fn max_per_page(mut self, max: u64) -> Self {
        self.max_per_page = max;
        self
    }
}

#[derive(serde::Deserialize)]
struct PaginationParams {
    page: Option<u64>,
    per_page: Option<u64>,
}

/// Pagination extractor: reads `?page=` and `?per_page=` from the query string.
///
/// Defaults:
/// - `page`: 1
/// - `per_page`: 20
/// - `per_page` max: 100
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    page: u64,
    per_page: u64,
}

impl Pagination {
    #[must_use]
    pub fn page(&self) -> u64 {
        self.page
    }

    #[must_use]
    pub fn per_page(&self) -> u64 {
        self.per_page
    }

    /// Alias for `per_page()` for common DB APIs.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.per_page
    }

    /// Zero-based offset (`(page - 1) * per_page`).
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.page.saturating_sub(1).saturating_mul(self.per_page)
    }
}

impl FromRequest for Pagination {
    type Error = QueryExtractError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Allow users/middleware to install an override per-request.
        let config = req
            .get_extension::<PaginationConfig>()
            .copied()
            .unwrap_or_default();

        let Query(params) = Query::<PaginationParams>::from_request(ctx, req).await?;

        let page = params.page.unwrap_or(config.default_page);
        if page == 0 {
            return Err(QueryExtractError::InvalidValue {
                name: "page".to_string(),
                value: "0".to_string(),
                expected: "u64",
                message: "must be >= 1".to_string(),
            });
        }

        let per_page = params.per_page.unwrap_or(config.default_per_page);
        if per_page == 0 {
            return Err(QueryExtractError::InvalidValue {
                name: "per_page".to_string(),
                value: "0".to_string(),
                expected: "u64",
                message: "must be >= 1".to_string(),
            });
        }
        if per_page > config.max_per_page {
            return Err(QueryExtractError::InvalidValue {
                name: "per_page".to_string(),
                value: per_page.to_string(),
                expected: "u64",
                message: format!("must be <= {}", config.max_per_page),
            });
        }

        Ok(Self { page, per_page })
    }
}

/// Generic paginated response payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u64,
    pub per_page: u64,
    pub total_pages: u64,
}

impl<T> Page<T> {
    #[must_use]
    pub fn new(items: Vec<T>, total: u64, page: u64, per_page: u64) -> Self {
        let total_pages = if per_page == 0 {
            0
        } else {
            total.div_ceil(per_page)
        };
        Self {
            items,
            total,
            page,
            per_page,
            total_pages,
        }
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

        let auth_str =
            std::str::from_utf8(auth_header).map_err(|_| OAuth2BearerError::invalid_scheme())?;

        let mut parts = auth_str.split_whitespace();
        let scheme = parts.next().ok_or_else(OAuth2BearerError::invalid_scheme)?;
        if !scheme.eq_ignore_ascii_case("bearer") {
            return Err(OAuth2BearerError::invalid_scheme());
        }

        let token = parts.next().unwrap_or("");
        if token.is_empty() {
            return Err(OAuth2BearerError::empty_token());
        }

        // Reject excessively long tokens to prevent memory/DoS issues
        const MAX_TOKEN_LEN: usize = 8192;
        if token.len() > MAX_TOKEN_LEN {
            return Err(OAuth2BearerError::empty_token());
        }

        Ok(OAuth2PasswordBearer::new(token))
    }
}

// ============================================================================
// HTTP Basic Auth Extractor
// ============================================================================

/// HTTP Basic Authentication credentials extractor.
///
/// Extracts username and password from the `Authorization` header using
/// the Basic authentication scheme (RFC 7617). The header value is expected
/// to be `Basic <base64(username:password)>`.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::BasicAuth;
///
/// async fn protected_route(auth: BasicAuth) -> impl IntoResponse {
///     format!("Hello, {}!", auth.username)
/// }
/// ```
///
/// # Error Responses
///
/// Returns **401 Unauthorized** with `WWW-Authenticate: Basic` header when:
/// - Authorization header is missing
/// - Header doesn't use Basic scheme
/// - Base64 decoding fails
/// - Decoded value doesn't contain a colon separator
///
/// # Optional Extraction
///
/// Use `Option<BasicAuth>` to make authentication optional:
///
/// ```ignore
/// async fn maybe_protected(auth: Option<BasicAuth>) -> impl IntoResponse {
///     match auth {
///         Some(creds) => format!("Hello, {}!", creds.username),
///         None => "Hello, anonymous!".to_string(),
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct BasicAuth {
    /// The extracted username.
    pub username: String,
    /// The extracted password (may be empty).
    pub password: String,
}

impl BasicAuth {
    /// Create new BasicAuth credentials.
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

    /// Decode a base64-encoded credentials string.
    fn decode_credentials(encoded: &str) -> Option<(String, String)> {
        // Base64 decode
        let decoded_bytes = base64_decode(encoded)?;
        let decoded = std::str::from_utf8(&decoded_bytes).ok()?;

        // Split on first colon (password may contain colons)
        let colon_pos = decoded.find(':')?;
        let username = decoded[..colon_pos].to_string();
        let password = decoded[colon_pos + 1..].to_string();

        Some((username, password))
    }
}

/// Simple base64 decoder (standard alphabet).
#[allow(clippy::cast_sign_loss)] // value is validated >= 0 before cast
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const DECODE_TABLE: [i8; 256] = {
        let mut table = [-1i8; 256];
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0;
        while i < 64 {
            table[alphabet[i] as usize] = i as i8;
            i += 1;
        }
        table
    };

    let input = input.trim_end_matches('=');
    let bytes = input.as_bytes();
    let output_len = bytes.len() * 3 / 4;
    let mut output = Vec::with_capacity(output_len);

    let mut buffer = 0u32;
    let mut bits_collected = 0;

    for &byte in bytes {
        let value = DECODE_TABLE[byte as usize];
        if value < 0 {
            return None; // Invalid character
        }
        buffer = (buffer << 6) | (value as u32);
        bits_collected += 6;

        if bits_collected >= 8 {
            bits_collected -= 8;
            output.push((buffer >> bits_collected) as u8);
        }
    }

    Some(output)
}

/// Error when HTTP Basic Auth extraction fails.
#[derive(Debug, Clone)]
pub struct BasicAuthError {
    /// The kind of error that occurred.
    pub kind: BasicAuthErrorKind,
}

/// The specific kind of Basic Auth error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasicAuthErrorKind {
    /// Authorization header is missing.
    MissingHeader,
    /// Authorization header doesn't use Basic scheme.
    InvalidScheme,
    /// Base64 decoding failed.
    InvalidEncoding,
    /// Decoded string doesn't contain colon separator.
    InvalidFormat,
}

impl BasicAuthError {
    /// Create a missing header error.
    #[must_use]
    pub fn missing_header() -> Self {
        Self {
            kind: BasicAuthErrorKind::MissingHeader,
        }
    }

    /// Create an invalid scheme error.
    #[must_use]
    pub fn invalid_scheme() -> Self {
        Self {
            kind: BasicAuthErrorKind::InvalidScheme,
        }
    }

    /// Create an invalid encoding error.
    #[must_use]
    pub fn invalid_encoding() -> Self {
        Self {
            kind: BasicAuthErrorKind::InvalidEncoding,
        }
    }

    /// Create an invalid format error.
    #[must_use]
    pub fn invalid_format() -> Self {
        Self {
            kind: BasicAuthErrorKind::InvalidFormat,
        }
    }
}

impl fmt::Display for BasicAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            BasicAuthErrorKind::MissingHeader => {
                write!(f, "Missing Authorization header")
            }
            BasicAuthErrorKind::InvalidScheme => {
                write!(f, "Authorization header must use Basic scheme")
            }
            BasicAuthErrorKind::InvalidEncoding => {
                write!(f, "Invalid base64 encoding in Authorization header")
            }
            BasicAuthErrorKind::InvalidFormat => {
                write!(f, "Invalid format in Basic auth credentials")
            }
        }
    }
}

impl IntoResponse for BasicAuthError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let message = match self.kind {
            BasicAuthErrorKind::MissingHeader => "Not authenticated",
            BasicAuthErrorKind::InvalidScheme => "Invalid authentication credentials",
            BasicAuthErrorKind::InvalidEncoding => "Invalid authentication credentials",
            BasicAuthErrorKind::InvalidFormat => "Invalid authentication credentials",
        };

        let body = serde_json::json!({
            "detail": message
        });

        Response::with_status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", b"Basic realm=\"api\"".to_vec())
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for BasicAuth {
    type Error = BasicAuthError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Get the Authorization header
        let auth_header = req
            .headers()
            .get("authorization")
            .ok_or_else(BasicAuthError::missing_header)?;

        // Convert to string
        let auth_str =
            std::str::from_utf8(auth_header).map_err(|_| BasicAuthError::invalid_encoding())?;

        let mut parts = auth_str.split_whitespace();
        let scheme = parts.next().ok_or_else(BasicAuthError::invalid_scheme)?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return Err(BasicAuthError::invalid_scheme());
        }

        let encoded = parts.next().unwrap_or("");
        if encoded.is_empty() {
            return Err(BasicAuthError::invalid_format());
        }

        // Reject excessively long credentials to prevent memory/DoS issues
        const MAX_ENCODED_LEN: usize = 8192;
        if encoded.len() > MAX_ENCODED_LEN {
            return Err(BasicAuthError::invalid_format());
        }

        // Decode and parse credentials
        let (username, password) = BasicAuth::decode_credentials(encoded.trim())
            .ok_or_else(BasicAuthError::invalid_format)?;

        Ok(BasicAuth::new(username, password))
    }
}

// ============================================================================
// Bearer Token Extractor
// ============================================================================

/// Bearer token extractor for `Authorization: Bearer <token>`.
///
/// This is a lightweight alternative to the OAuth2-specific extractor when you
/// just want a token string and will validate it yourself (e.g. JWT).
#[derive(Debug, Clone)]
pub struct BearerToken {
    token: String,
}

impl BearerToken {
    /// Create a new bearer token wrapper.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
        }
    }

    /// Access the token string (without the `Bearer ` prefix).
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Error when bearer token extraction fails.
#[derive(Debug, Clone)]
pub struct BearerTokenError {
    /// The kind of error that occurred.
    pub kind: BearerTokenErrorKind,
}

/// The specific kind of bearer token error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BearerTokenErrorKind {
    /// Authorization header is missing.
    MissingHeader,
    /// Authorization header isn't valid UTF-8.
    InvalidUtf8,
    /// Authorization header doesn't use the Bearer scheme.
    InvalidScheme,
    /// Authorization header has `Bearer` but no token value.
    EmptyToken,
}

impl BearerTokenError {
    #[must_use]
    pub fn missing_header() -> Self {
        Self {
            kind: BearerTokenErrorKind::MissingHeader,
        }
    }

    #[must_use]
    pub fn invalid_utf8() -> Self {
        Self {
            kind: BearerTokenErrorKind::InvalidUtf8,
        }
    }

    #[must_use]
    pub fn invalid_scheme() -> Self {
        Self {
            kind: BearerTokenErrorKind::InvalidScheme,
        }
    }

    #[must_use]
    pub fn empty_token() -> Self {
        Self {
            kind: BearerTokenErrorKind::EmptyToken,
        }
    }
}

impl fmt::Display for BearerTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            BearerTokenErrorKind::MissingHeader => write!(f, "Missing Authorization header"),
            BearerTokenErrorKind::InvalidUtf8 => write!(f, "Invalid Authorization header encoding"),
            BearerTokenErrorKind::InvalidScheme => {
                write!(f, "Authorization header must use Bearer scheme")
            }
            BearerTokenErrorKind::EmptyToken => write!(f, "Bearer token is empty"),
        }
    }
}

impl IntoResponse for BearerTokenError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        // Match FastAPI's typical shape for auth failures: 401 + WWW-Authenticate.
        let detail = match self.kind {
            BearerTokenErrorKind::MissingHeader => "Not authenticated",
            BearerTokenErrorKind::InvalidUtf8 => "Invalid authentication credentials",
            BearerTokenErrorKind::InvalidScheme => "Invalid authentication credentials",
            BearerTokenErrorKind::EmptyToken => "Invalid authentication credentials",
        };

        let body = serde_json::json!({ "detail": detail });
        Response::with_status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", b"Bearer".to_vec())
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for BearerToken {
    type Error = BearerTokenError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let auth_header = req
            .headers()
            .get("authorization")
            .ok_or_else(BearerTokenError::missing_header)?;

        let auth_str =
            std::str::from_utf8(auth_header).map_err(|_| BearerTokenError::invalid_utf8())?;

        let mut parts = auth_str.split_whitespace();
        let scheme = parts.next().ok_or_else(BearerTokenError::invalid_scheme)?;
        if !scheme.eq_ignore_ascii_case("bearer") {
            return Err(BearerTokenError::invalid_scheme());
        }

        let token = parts.next().unwrap_or("").trim();
        if token.is_empty() {
            return Err(BearerTokenError::empty_token());
        }

        // Reject excessively long tokens to prevent memory/DoS issues
        const MAX_TOKEN_LEN: usize = 8192;
        if token.len() > MAX_TOKEN_LEN {
            return Err(BearerTokenError::empty_token());
        }

        Ok(BearerToken::new(token.to_string()))
    }
}

// ============================================================================
// API Key Extractor
// ============================================================================

/// API key extraction location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyLocation {
    /// Extract from HTTP header.
    Header,
    /// Extract from query string.
    Query,
    /// Extract from cookie.
    Cookie,
}

/// API key authentication extractor.
///
/// Extracts an API key from a configurable location (header, query, or cookie).
/// The default location is the `X-API-Key` header.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::ApiKey;
///
/// async fn protected(key: ApiKey) -> impl IntoResponse {
///     format!("API key: {}", key.key)
/// }
/// ```
///
/// # Custom Configuration
///
/// Use `ApiKeyConfig` to customize extraction:
///
/// ```ignore
/// // From query parameter
/// ApiKeyConfig::query("api_key")
///
/// // From custom header
/// ApiKeyConfig::header("Authorization")
///
/// // From cookie
/// ApiKeyConfig::cookie("session_key")
/// ```
#[derive(Debug, Clone)]
pub struct ApiKey {
    /// The extracted API key value.
    pub key: String,
}

impl ApiKey {
    /// Create a new API key.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into() }
    }

    /// Get the key value.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Consume self and return the key.
    #[must_use]
    pub fn into_key(self) -> String {
        self.key
    }
}

impl Deref for ApiKey {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.key
    }
}

/// Configuration for API key extraction.
#[derive(Debug, Clone)]
pub struct ApiKeyConfig {
    /// Name of the header/query param/cookie.
    pub name: String,
    /// Where to look for the API key.
    pub location: ApiKeyLocation,
    /// Description for OpenAPI documentation.
    pub description: Option<String>,
}

impl Default for ApiKeyConfig {
    fn default() -> Self {
        Self {
            name: "X-API-Key".to_string(),
            location: ApiKeyLocation::Header,
            description: None,
        }
    }
}

impl ApiKeyConfig {
    /// Create a config for header-based API key.
    #[must_use]
    pub fn header(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            location: ApiKeyLocation::Header,
            description: None,
        }
    }

    /// Create a config for query-based API key.
    #[must_use]
    pub fn query(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            location: ApiKeyLocation::Query,
            description: None,
        }
    }

    /// Create a config for cookie-based API key.
    #[must_use]
    pub fn cookie(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            location: ApiKeyLocation::Cookie,
            description: None,
        }
    }

    /// Set description for OpenAPI.
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Error when API key extraction fails.
#[derive(Debug, Clone)]
pub struct ApiKeyError {
    /// The kind of error.
    pub kind: ApiKeyErrorKind,
    /// Location where the key was expected.
    pub location: ApiKeyLocation,
    /// Name of the expected key.
    pub name: String,
}

/// The specific kind of API key error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyErrorKind {
    /// API key is missing.
    Missing,
    /// API key is empty.
    Empty,
}

impl ApiKeyError {
    /// Create a missing key error.
    #[must_use]
    pub fn missing(location: ApiKeyLocation, name: impl Into<String>) -> Self {
        Self {
            kind: ApiKeyErrorKind::Missing,
            location,
            name: name.into(),
        }
    }

    /// Create an empty key error.
    #[must_use]
    pub fn empty(location: ApiKeyLocation, name: impl Into<String>) -> Self {
        Self {
            kind: ApiKeyErrorKind::Empty,
            location,
            name: name.into(),
        }
    }
}

impl fmt::Display for ApiKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let location_name = match self.location {
            ApiKeyLocation::Header => "header",
            ApiKeyLocation::Query => "query parameter",
            ApiKeyLocation::Cookie => "cookie",
        };
        match self.kind {
            ApiKeyErrorKind::Missing => {
                write!(f, "Missing API key in {} '{}'", location_name, self.name)
            }
            ApiKeyErrorKind::Empty => {
                write!(f, "Empty API key in {} '{}'", location_name, self.name)
            }
        }
    }
}

impl IntoResponse for ApiKeyError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let body = serde_json::json!({
            "detail": "Not authenticated"
        });

        Response::with_status(StatusCode::UNAUTHORIZED)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

impl FromRequest for ApiKey {
    type Error = ApiKeyError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Default: extract from X-API-Key header
        let name = "X-API-Key";
        let location = ApiKeyLocation::Header;

        let key = req
            .headers()
            .get("x-api-key")
            .and_then(|v| std::str::from_utf8(v).ok())
            .map(|s| s.trim().to_string())
            .ok_or_else(|| ApiKeyError::missing(location, name))?;

        if key.is_empty() {
            return Err(ApiKeyError::empty(location, name));
        }

        Ok(ApiKey::new(key))
    }
}

// ============================================================================
// Cookie Extractor
// ============================================================================

/// Trait for cookie name markers (similar to HeaderName).
///
/// Implement this trait to create a type-safe cookie extractor.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{CookieName, Cookie};
///
/// struct SessionId;
/// impl CookieName for SessionId {
///     const NAME: &'static str = "session_id";
/// }
///
/// async fn get_session(Cookie(session): Cookie<String, SessionId>) -> impl IntoResponse {
///     format!("Session: {}", session)
/// }
/// ```
pub trait CookieName {
    /// The cookie name as it appears in the Cookie header.
    const NAME: &'static str;
}

/// Cookie value extractor.
///
/// Extracts a single cookie value from the `Cookie` request header using
/// a type-safe name marker.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::{Cookie, CookieName};
///
/// struct SessionId;
/// impl CookieName for SessionId {
///     const NAME: &'static str = "session_id";
/// }
///
/// async fn get_session(Cookie(session): Cookie<String, SessionId>) -> impl IntoResponse {
///     format!("Session: {}", session)
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Cookie<T, N> {
    /// The extracted cookie value.
    pub value: T,
    _marker: std::marker::PhantomData<N>,
}

impl<T, N> Cookie<T, N> {
    /// Create a new cookie wrapper.
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

impl<T, N> Deref for Cookie<T, N> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T, N> DerefMut for Cookie<T, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

/// Error when cookie extraction fails.
#[derive(Debug, Clone)]
pub struct CookieExtractError {
    /// The cookie name that was expected.
    pub name: String,
    /// The kind of error.
    pub kind: CookieExtractErrorKind,
}

/// The specific kind of cookie extraction error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieExtractErrorKind {
    /// No Cookie header present.
    NoCookieHeader,
    /// Cookie with given name not found.
    NotFound,
    /// Cookie value is empty.
    Empty,
    /// Failed to parse cookie value.
    ParseError,
}

impl CookieExtractError {
    /// Create a no cookie header error.
    #[must_use]
    pub fn no_header(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: CookieExtractErrorKind::NoCookieHeader,
        }
    }

    /// Create a not found error.
    #[must_use]
    pub fn not_found(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: CookieExtractErrorKind::NotFound,
        }
    }

    /// Create an empty value error.
    #[must_use]
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: CookieExtractErrorKind::Empty,
        }
    }

    /// Create a parse error.
    #[must_use]
    pub fn parse_error(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: CookieExtractErrorKind::ParseError,
        }
    }
}

impl fmt::Display for CookieExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            CookieExtractErrorKind::NoCookieHeader => {
                write!(f, "No Cookie header in request")
            }
            CookieExtractErrorKind::NotFound => {
                write!(f, "Cookie '{}' not found", self.name)
            }
            CookieExtractErrorKind::Empty => {
                write!(f, "Cookie '{}' is empty", self.name)
            }
            CookieExtractErrorKind::ParseError => {
                write!(f, "Failed to parse cookie '{}'", self.name)
            }
        }
    }
}

impl IntoResponse for CookieExtractError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let body = serde_json::json!({
            "detail": [{
                "type": "missing",
                "loc": ["cookie", &self.name],
                "msg": format!("Cookie '{}' is required", self.name),
            }]
        });

        Response::with_status(StatusCode::UNPROCESSABLE_ENTITY)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

/// Parse cookies from the Cookie header.
fn parse_cookies(header: &str) -> impl Iterator<Item = (&str, &str)> {
    header.split(';').filter_map(|cookie| {
        let cookie = cookie.trim();
        let eq_pos = cookie.find('=')?;
        let name = cookie[..eq_pos].trim();
        let value = cookie[eq_pos + 1..].trim();
        Some((name, value))
    })
}

impl<T, N> FromRequest for Cookie<T, N>
where
    T: FromHeaderValue + Send + Sync + 'static,
    N: CookieName + Send + Sync + 'static,
{
    type Error = CookieExtractError;

    async fn from_request(_ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        let cookie_name = N::NAME;

        // Get Cookie header
        let cookie_header = req
            .headers()
            .get("cookie")
            .ok_or_else(|| CookieExtractError::no_header(cookie_name))?;

        let cookie_str = std::str::from_utf8(cookie_header)
            .map_err(|_| CookieExtractError::not_found(cookie_name))?;

        // Parse cookies and find the one we want
        for (name, value) in parse_cookies(cookie_str) {
            if name == cookie_name {
                if value.is_empty() {
                    return Err(CookieExtractError::empty(cookie_name));
                }
                let parsed = T::from_header_value(value)
                    .map_err(|_| CookieExtractError::parse_error(cookie_name))?;
                return Ok(Cookie::new(parsed));
            }
        }

        Err(CookieExtractError::not_found(cookie_name))
    }
}

// Common cookie name markers
/// Session ID cookie marker.
pub struct SessionId;
impl CookieName for SessionId {
    const NAME: &'static str = "session_id";
}

/// CSRF token cookie marker.
pub struct CsrfToken;
impl CookieName for CsrfToken {
    const NAME: &'static str = "csrf_token";
}

/// CSRF token cookie marker (compat alias used by some middleware/tests).
pub struct CsrfTokenCookie;
impl CookieName for CsrfTokenCookie {
    const NAME: &'static str = "csrf_token";
}

// ============================================================================
// Form Data Extractor
// ============================================================================

/// URL-encoded form data extractor.
///
/// Extracts `application/x-www-form-urlencoded` form data from the request body
/// and deserializes it into the target type using serde.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::Form;
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct LoginForm {
///     username: String,
///     password: String,
/// }
///
/// async fn login(Form(form): Form<LoginForm>) -> impl IntoResponse {
///     format!("Login attempt for: {}", form.username)
/// }
/// ```
///
/// # Content-Type
///
/// This extractor expects the `Content-Type` header to be
/// `application/x-www-form-urlencoded`. Other content types will result
/// in an error.
///
/// # Error Responses
///
/// Returns **422 Unprocessable Entity** when:
/// - Content-Type is not `application/x-www-form-urlencoded`
/// - Body cannot be read
/// - Deserialization fails
#[derive(Debug, Clone)]
pub struct Form<T>(pub T);

impl<T> Form<T> {
    /// Create a new Form wrapper.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Unwrap the inner value.
    #[must_use]
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

/// Error when form extraction fails.
#[derive(Debug)]
pub struct FormExtractError {
    /// The kind of error.
    pub kind: FormExtractErrorKind,
}

/// The specific kind of form extraction error.
#[derive(Debug)]
pub enum FormExtractErrorKind {
    /// Wrong content type.
    WrongContentType {
        /// The actual content type received.
        actual: Option<String>,
    },
    /// Failed to read body.
    ReadError(String),
    /// Body exceeds configured limit.
    PayloadTooLarge { size: usize, limit: usize },
    /// Failed to deserialize.
    DeserializeError(String),
}

impl FormExtractError {
    /// Create a wrong content type error.
    #[must_use]
    pub fn wrong_content_type(actual: Option<String>) -> Self {
        Self {
            kind: FormExtractErrorKind::WrongContentType { actual },
        }
    }

    /// Create a read error.
    #[must_use]
    pub fn read_error(msg: impl Into<String>) -> Self {
        Self {
            kind: FormExtractErrorKind::ReadError(msg.into()),
        }
    }

    /// Create a payload-too-large error.
    #[must_use]
    pub fn payload_too_large(size: usize, limit: usize) -> Self {
        Self {
            kind: FormExtractErrorKind::PayloadTooLarge { size, limit },
        }
    }

    /// Create a deserialization error.
    #[must_use]
    pub fn deserialize_error(msg: impl Into<String>) -> Self {
        Self {
            kind: FormExtractErrorKind::DeserializeError(msg.into()),
        }
    }
}

impl fmt::Display for FormExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            FormExtractErrorKind::WrongContentType { actual } => {
                if let Some(ct) = actual {
                    write!(
                        f,
                        "Expected content-type 'application/x-www-form-urlencoded', got '{}'",
                        ct
                    )
                } else {
                    write!(
                        f,
                        "Expected content-type 'application/x-www-form-urlencoded', none provided"
                    )
                }
            }
            FormExtractErrorKind::ReadError(msg) => {
                write!(f, "Failed to read form body: {}", msg)
            }
            FormExtractErrorKind::PayloadTooLarge { size, limit } => {
                write!(
                    f,
                    "Request body too large: {size} bytes exceeds {limit} byte limit"
                )
            }
            FormExtractErrorKind::DeserializeError(msg) => {
                write!(f, "Failed to deserialize form data: {}", msg)
            }
        }
    }
}

impl IntoResponse for FormExtractError {
    fn into_response(self) -> crate::response::Response {
        use crate::response::{Response, ResponseBody, StatusCode};

        let (status, detail) = match &self.kind {
            FormExtractErrorKind::WrongContentType { .. } => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, self.to_string())
            }
            FormExtractErrorKind::ReadError(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            FormExtractErrorKind::PayloadTooLarge { .. } => {
                (StatusCode::PAYLOAD_TOO_LARGE, self.to_string())
            }
            FormExtractErrorKind::DeserializeError(msg) => {
                (StatusCode::UNPROCESSABLE_ENTITY, msg.clone())
            }
        };

        let body = serde_json::json!({
            "detail": detail
        });

        Response::with_status(status)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes()))
    }
}

/// Parse URL-encoded form data into key-value pairs.
fn parse_urlencoded(data: &str) -> impl Iterator<Item = (String, String)> + '_ {
    data.split('&').filter_map(|pair| {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?;
        let value = parts.next().unwrap_or("");

        // URL decode both key and value
        let key = url_decode(key);
        let value = url_decode(value);

        Some((key, value))
    })
}

/// URL-decode a string (percent-decoding).
///
/// Uses byte-level decoding to correctly handle multi-byte UTF-8 sequences
/// (e.g., `%C3%A9` → `é`). Falls back to the replacement character for
/// invalid UTF-8 after decoding.
fn url_decode(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.as_bytes().iter().copied();

    while let Some(b) = chars.next() {
        if b == b'%' {
            // Try to read two hex digits
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(h), Some(l)) = (hi, lo) {
                let hex_str = [h, l];
                if let Ok(decoded) =
                    u8::from_str_radix(std::str::from_utf8(&hex_str).unwrap_or(""), 16)
                {
                    bytes.push(decoded);
                    continue;
                }
                // Invalid hex — keep raw bytes
                bytes.push(b'%');
                bytes.push(h);
                bytes.push(l);
            } else {
                // Truncated escape — keep raw bytes
                bytes.push(b'%');
                if let Some(h) = hi {
                    bytes.push(h);
                }
            }
        } else if b == b'+' {
            // Plus signs decode to spaces in form data
            bytes.push(b' ');
        } else {
            bytes.push(b);
        }
    }

    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

impl<T> FromRequest for Form<T>
where
    T: serde::de::DeserializeOwned + Send + Sync + 'static,
{
    type Error = FormExtractError;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // Check content type
        let content_type = req
            .headers()
            .get("content-type")
            .and_then(|v| std::str::from_utf8(v).ok());

        let is_form = content_type
            .map(|ct| {
                ct.starts_with("application/x-www-form-urlencoded")
                    || ct.starts_with("application/x-www-form-urlencoded;")
            })
            .unwrap_or(false);

        if !is_form {
            return Err(FormExtractError::wrong_content_type(
                content_type.map(String::from),
            ));
        }

        // Read body
        let limit = DEFAULT_JSON_LIMIT;
        let body = collect_body_limited(ctx, req.take_body(), limit)
            .await
            .map_err(|e| match e {
                RequestBodyStreamError::TooLarge { received, .. } => {
                    FormExtractError::payload_too_large(received, limit)
                }
                other => FormExtractError::read_error(other.to_string()),
            })?;
        let body_str = std::str::from_utf8(&body)
            .map_err(|e| FormExtractError::read_error(format!("Invalid UTF-8: {}", e)))?;

        // Parse form data into a map for serde
        let pairs: Vec<(String, String)> = parse_urlencoded(body_str).collect();

        // Use serde_json as an intermediary (form data → JSON object → T)
        // This handles nested structures and arrays properly
        let mut map = serde_json::Map::new();
        for (key, value) in pairs {
            // Handle array notation (key[] or key[0])
            if key.ends_with("[]") {
                let base_key = &key[..key.len() - 2];
                let entry = map
                    .entry(base_key.to_string())
                    .or_insert_with(|| serde_json::Value::Array(Vec::new()));
                if let serde_json::Value::Array(arr) = entry {
                    arr.push(serde_json::Value::String(value));
                }
            } else {
                map.insert(key, serde_json::Value::String(value));
            }
        }

        let json_value = serde_json::Value::Object(map);
        let result: T = serde_json::from_value(json_value)
            .map_err(|e| FormExtractError::deserialize_error(e.to_string()))?;

        Ok(Form(result))
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
// Validated Extractor Wrapper
// ============================================================================

/// Validated extractor wrapper.
///
/// Wraps any extractor and runs validation after extraction.
/// Use this with types that implement the [`Validate`] trait.
///
/// # Error Responses
///
/// - If extraction fails, returns the inner extractor's error response
/// - If validation fails, returns **422 Unprocessable Entity** with validation errors
///
/// # Example
///
/// ```ignore
/// use fastapi_core::extract::{Json, Valid};
/// use fastapi_core::validation::Validate;
/// use fastapi_macros::Validate;
/// use serde::Deserialize;
///
/// #[derive(Deserialize, Validate)]
/// struct CreateUser {
///     #[validate(email)]
///     email: String,
///     #[validate(length(min = 3, max = 50))]
///     username: String,
/// }
///
/// async fn create_user(Valid(Json(user)): Valid<Json<CreateUser>>) -> impl IntoResponse {
///     format!("Created user: {}", user.username)
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Valid<T>(pub T);

impl<T> Valid<T> {
    /// Unwrap the inner value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for Valid<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Valid<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Error returned when validated extraction fails.
#[derive(Debug)]
pub enum ValidExtractError<E> {
    /// Inner extraction failed.
    Extract(E),
    /// Validation failed.
    Validation(Box<ValidationErrors>),
}

impl<E: std::fmt::Display> std::fmt::Display for ValidExtractError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Extract(e) => write!(f, "Extraction failed: {e}"),
            Self::Validation(e) => write!(f, "{e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ValidExtractError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Extract(e) => Some(e),
            Self::Validation(e) => Some(&**e),
        }
    }
}

impl<E: IntoResponse> IntoResponse for ValidExtractError<E> {
    fn into_response(self) -> crate::response::Response {
        match self {
            Self::Extract(e) => e.into_response(),
            Self::Validation(e) => (*e).into_response(),
        }
    }
}

/// Trait for types that can be validated after extraction.
///
/// This is a re-export convenience trait. For types implementing
/// validation via the derive macro, use `#[derive(Validate)]`.
pub use crate::validation::Validate;

impl<T> FromRequest for Valid<T>
where
    T: FromRequest,
    T::Error: IntoResponse,
    <T as Deref>::Target: Validate,
    T: Deref,
{
    type Error = ValidExtractError<T::Error>;

    async fn from_request(ctx: &RequestContext, req: &mut Request) -> Result<Self, Self::Error> {
        // First, extract the inner value
        let inner = T::from_request(ctx, req)
            .await
            .map_err(ValidExtractError::Extract)?;

        // Then validate it
        inner.validate().map_err(ValidExtractError::Validation)?;

        Ok(Valid(inner))
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
    fn json_content_type_rejects_near_miss_types() {
        use serde::Deserialize;

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            value: i32,
        }

        // These look like JSON but are different formats — must be rejected
        for content_type in &[
            "application/jsonl",
            "application/json-seq",
            "application/json-patch",
            "application/jsonlines",
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

        // But application/json with parameters is still valid
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut().insert(
            "content-type",
            b"application/json; charset=utf-8".to_vec(),
        );
        req.set_body(Body::Bytes(b"{\"value\": 42}".to_vec()));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        assert!(
            result.is_ok(),
            "application/json with charset parameter should be accepted"
        );

        // Vendor JSON types should also be accepted
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.headers_mut().insert(
            "content-type",
            b"application/vnd.api+json".to_vec(),
        );
        req.set_body(Body::Bytes(b"{\"value\": 42}".to_vec()));

        let result = futures_executor::block_on(Json::<Data>::from_request(&ctx, &mut req));
        assert!(
            result.is_ok(),
            "application/vnd.api+json should be accepted"
        );
    }

    #[test]
    fn form_url_decode_handles_multibyte_utf8() {
        // %C3%A9 is the UTF-8 encoding of "é"
        let decoded = url_decode("caf%C3%A9");
        assert_eq!(decoded, "café");

        // Japanese: %E6%97%A5%E6%9C%AC = "日本"
        let decoded = url_decode("%E6%97%A5%E6%9C%AC");
        assert_eq!(decoded, "日本");

        // Plus sign and mixed
        let decoded = url_decode("hello+w%C3%B6rld");
        assert_eq!(decoded, "hello wörld");

        // ASCII only
        let decoded = url_decode("hello+world");
        assert_eq!(decoded, "hello world");
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
}

#[cfg(test)]
mod valid_tests {
    use super::*;
    use crate::error::ValidationErrors;
    use crate::request::Method;
    use crate::validation::Validate;

    fn test_context() -> RequestContext {
        let cx = asupersync::Cx::for_testing();
        RequestContext::new(cx, 12345)
    }

    // Implement Validate for String (for testing purposes)
    impl Validate for String {
        fn validate(&self) -> Result<(), Box<ValidationErrors>> {
            if self.is_empty() {
                let mut errors = ValidationErrors::new();
                errors.push(crate::error::ValidationError::new(
                    crate::error::error_types::STRING_TOO_SHORT,
                    crate::error::loc::body(),
                ));
                Err(Box::new(errors))
            } else if self.len() > 100 {
                let mut errors = ValidationErrors::new();
                errors.push(crate::error::ValidationError::new(
                    crate::error::error_types::STRING_TOO_LONG,
                    crate::error::loc::body(),
                ));
                Err(Box::new(errors))
            } else {
                Ok(())
            }
        }
    }

    // Mock extractor for testing
    struct MockExtractor(String);

    impl Deref for MockExtractor {
        type Target = String;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl FromRequest for MockExtractor {
        type Error = HttpError;

        async fn from_request(
            _ctx: &RequestContext,
            req: &mut Request,
        ) -> Result<Self, Self::Error> {
            let body = req.take_body();
            let bytes = body.into_bytes();
            let s = String::from_utf8(bytes).map_err(|_| HttpError::bad_request())?;
            Ok(MockExtractor(s))
        }
    }

    #[test]
    fn valid_deref() {
        let valid = Valid(42i32);
        assert_eq!(*valid, 42);
    }

    #[test]
    fn valid_into_inner() {
        let valid = Valid("hello".to_string());
        assert_eq!(valid.into_inner(), "hello");
    }

    #[test]
    fn valid_extract_and_validate_success() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.set_body(Body::Bytes(b"valid string".to_vec()));

        let result =
            futures_executor::block_on(Valid::<MockExtractor>::from_request(&ctx, &mut req));
        assert!(result.is_ok());
        let Valid(MockExtractor(inner)) = result.unwrap();
        assert_eq!(inner, "valid string");
    }

    #[test]
    fn valid_extract_validation_fails_empty() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        req.set_body(Body::Bytes(b"".to_vec()));

        let result =
            futures_executor::block_on(Valid::<MockExtractor>::from_request(&ctx, &mut req));
        assert!(matches!(result, Err(ValidExtractError::Validation(_))));
    }

    #[test]
    fn valid_extract_validation_fails_too_long() {
        let ctx = test_context();
        let mut req = Request::new(Method::Post, "/test");
        // Create a string longer than 100 characters
        let long_string = "a".repeat(101);
        req.set_body(Body::Bytes(long_string.into_bytes()));

        let result =
            futures_executor::block_on(Valid::<MockExtractor>::from_request(&ctx, &mut req));
        assert!(matches!(result, Err(ValidExtractError::Validation(_))));
    }

    #[test]
    fn valid_extract_error_display() {
        let extract_err: ValidExtractError<HttpError> =
            ValidExtractError::Extract(HttpError::bad_request());
        let display = format!("{}", extract_err);
        assert!(display.contains("Extraction failed"));

        let validation_err: ValidExtractError<HttpError> =
            ValidExtractError::Validation(Box::new(ValidationErrors::new()));
        let display = format!("{}", validation_err);
        assert!(display.contains("validation error"));
    }
}
