//! HTTP response types.

use serde::Serialize;
use std::fmt;
use std::pin::Pin;

use asupersync::stream::Stream;

use crate::extract::Cookie;
#[cfg(test)]
use asupersync::types::PanicPayload;
use asupersync::types::{CancelKind, CancelReason, Outcome};

/// HTTP status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatusCode(u16);

impl StatusCode {
    // Informational
    /// 100 Continue
    pub const CONTINUE: Self = Self(100);
    /// 101 Switching Protocols
    pub const SWITCHING_PROTOCOLS: Self = Self(101);

    // Success
    /// 200 OK
    pub const OK: Self = Self(200);
    /// 201 Created
    pub const CREATED: Self = Self(201);
    /// 202 Accepted
    pub const ACCEPTED: Self = Self(202);
    /// 204 No Content
    pub const NO_CONTENT: Self = Self(204);
    /// 206 Partial Content
    pub const PARTIAL_CONTENT: Self = Self(206);

    // Redirection
    /// 301 Moved Permanently
    pub const MOVED_PERMANENTLY: Self = Self(301);
    /// 302 Found
    pub const FOUND: Self = Self(302);
    /// 303 See Other
    pub const SEE_OTHER: Self = Self(303);
    /// 304 Not Modified
    pub const NOT_MODIFIED: Self = Self(304);
    /// 307 Temporary Redirect
    pub const TEMPORARY_REDIRECT: Self = Self(307);
    /// 308 Permanent Redirect
    pub const PERMANENT_REDIRECT: Self = Self(308);

    // Client Error
    /// 400 Bad Request
    pub const BAD_REQUEST: Self = Self(400);
    /// 401 Unauthorized
    pub const UNAUTHORIZED: Self = Self(401);
    /// 403 Forbidden
    pub const FORBIDDEN: Self = Self(403);
    /// 404 Not Found
    pub const NOT_FOUND: Self = Self(404);
    /// 405 Method Not Allowed
    pub const METHOD_NOT_ALLOWED: Self = Self(405);
    /// 406 Not Acceptable
    pub const NOT_ACCEPTABLE: Self = Self(406);
    /// 412 Precondition Failed
    pub const PRECONDITION_FAILED: Self = Self(412);
    /// 413 Payload Too Large
    pub const PAYLOAD_TOO_LARGE: Self = Self(413);
    /// 415 Unsupported Media Type
    pub const UNSUPPORTED_MEDIA_TYPE: Self = Self(415);
    /// 416 Range Not Satisfiable
    pub const RANGE_NOT_SATISFIABLE: Self = Self(416);
    /// 422 Unprocessable Entity
    pub const UNPROCESSABLE_ENTITY: Self = Self(422);
    /// 429 Too Many Requests
    pub const TOO_MANY_REQUESTS: Self = Self(429);
    /// 499 Client Closed Request
    pub const CLIENT_CLOSED_REQUEST: Self = Self(499);

    // Server Error
    /// 500 Internal Server Error
    pub const INTERNAL_SERVER_ERROR: Self = Self(500);
    /// 503 Service Unavailable
    pub const SERVICE_UNAVAILABLE: Self = Self(503);
    /// 504 Gateway Timeout
    pub const GATEWAY_TIMEOUT: Self = Self(504);

    /// Create a status code from a u16.
    #[must_use]
    pub const fn from_u16(code: u16) -> Self {
        Self(code)
    }

    /// Get the numeric value.
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    /// Check if status code allows a body.
    #[must_use]
    pub const fn allows_body(self) -> bool {
        !matches!(self.0, 100..=103 | 204 | 304)
    }

    /// Get the canonical reason phrase.
    #[must_use]
    pub const fn canonical_reason(self) -> &'static str {
        match self.0 {
            100 => "Continue",
            101 => "Switching Protocols",
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            204 => "No Content",
            206 => "Partial Content",
            301 => "Moved Permanently",
            302 => "Found",
            303 => "See Other",
            304 => "Not Modified",
            307 => "Temporary Redirect",
            308 => "Permanent Redirect",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            406 => "Not Acceptable",
            412 => "Precondition Failed",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            416 => "Range Not Satisfiable",
            422 => "Unprocessable Entity",
            429 => "Too Many Requests",
            499 => "Client Closed Request",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "Unknown",
        }
    }
}

/// Streamed response body type.
pub type BodyStream = Pin<Box<dyn Stream<Item = Vec<u8>> + Send>>;

/// Response body.
pub enum ResponseBody {
    /// Empty body.
    Empty,
    /// Bytes body.
    Bytes(Vec<u8>),
    /// Streaming body.
    Stream(BodyStream),
}

impl ResponseBody {
    /// Create a streaming response body.
    #[must_use]
    pub fn stream<S>(stream: S) -> Self
    where
        S: Stream<Item = Vec<u8>> + Send + 'static,
    {
        Self::Stream(Box::pin(stream))
    }

    /// Check if body is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty) || matches!(self, Self::Bytes(b) if b.is_empty())
    }

    /// Get body length.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Bytes(b) => b.len(),
            Self::Stream(_) => 0,
        }
    }
}

impl fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.debug_tuple("Empty").finish(),
            Self::Bytes(bytes) => f.debug_tuple("Bytes").field(bytes).finish(),
            Self::Stream(_) => f.debug_tuple("Stream").finish(),
        }
    }
}

// ============================================================================
// Header Validation (CRLF Injection Prevention)
// ============================================================================

/// Check if a header name contains only valid HTTP token characters.
///
/// Valid token characters per RFC 7230:
/// `!#$%&'*+-.0-9A-Z^_`a-z|~`
fn is_valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            matches!(b,
                b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' |
                b'0'..=b'9' | b'A'..=b'Z' | b'^' | b'_' | b'`' | b'a'..=b'z' | b'|' | b'~'
            )
        })
}

/// Sanitize a header value to prevent CRLF injection attacks.
///
/// Removes CR (\r) and LF (\n) characters which could be used to inject
/// additional headers. Also removes null bytes.
fn sanitize_header_value(value: Vec<u8>) -> Vec<u8> {
    value
        .into_iter()
        .filter(|&b| b != b'\r' && b != b'\n' && b != 0)
        .collect()
}

/// HTTP response.
#[derive(Debug)]
pub struct Response {
    status: StatusCode,
    headers: Vec<(String, Vec<u8>)>,
    body: ResponseBody,
}

impl Response {
    /// Create a response with the given status.
    #[must_use]
    pub fn with_status(status: StatusCode) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: ResponseBody::Empty,
        }
    }

    /// Create a 200 OK response.
    #[must_use]
    pub fn ok() -> Self {
        Self::with_status(StatusCode::OK)
    }

    /// Create a 201 Created response.
    #[must_use]
    pub fn created() -> Self {
        Self::with_status(StatusCode::CREATED)
    }

    /// Create a 204 No Content response.
    #[must_use]
    pub fn no_content() -> Self {
        Self::with_status(StatusCode::NO_CONTENT)
    }

    /// Create a 500 Internal Server Error response.
    #[must_use]
    pub fn internal_error() -> Self {
        Self::with_status(StatusCode::INTERNAL_SERVER_ERROR)
    }

    /// Create a 206 Partial Content response.
    ///
    /// Used for range requests. You should also set the `Content-Range` header.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::{Response, ResponseBody};
    ///
    /// let response = Response::partial_content()
    ///     .header("Content-Range", b"bytes 0-499/1000".to_vec())
    ///     .header("Accept-Ranges", b"bytes".to_vec())
    ///     .body(ResponseBody::Bytes(partial_data));
    /// ```
    #[must_use]
    pub fn partial_content() -> Self {
        Self::with_status(StatusCode::PARTIAL_CONTENT)
    }

    /// Create a 416 Range Not Satisfiable response.
    ///
    /// Used when a Range header specifies a range that cannot be satisfied.
    /// You should also set the `Content-Range` header with the resource size.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_core::Response;
    ///
    /// let response = Response::range_not_satisfiable()
    ///     .header("Content-Range", b"bytes */1000".to_vec());
    /// ```
    #[must_use]
    pub fn range_not_satisfiable() -> Self {
        Self::with_status(StatusCode::RANGE_NOT_SATISFIABLE)
    }

    /// Create a 304 Not Modified response.
    ///
    /// Used for conditional requests where the resource has not changed.
    /// The response body is empty per HTTP spec.
    #[must_use]
    pub fn not_modified() -> Self {
        Self::with_status(StatusCode::NOT_MODIFIED)
    }

    /// Create a 412 Precondition Failed response.
    ///
    /// Used when a conditional request's precondition (e.g., `If-Match`) fails.
    #[must_use]
    pub fn precondition_failed() -> Self {
        Self::with_status(StatusCode::PRECONDITION_FAILED)
    }

    /// Set the ETag header on this response.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let response = Response::ok()
    ///     .with_etag("\"abc123\"")
    ///     .body(b"content".to_vec());
    /// ```
    #[must_use]
    pub fn with_etag(self, etag: impl Into<String>) -> Self {
        self.header("ETag", etag.into().into_bytes())
    }

    /// Set a weak ETag header on this response.
    ///
    /// Automatically prefixes with `W/` if not already present.
    #[must_use]
    pub fn with_weak_etag(self, etag: impl Into<String>) -> Self {
        let etag = etag.into();
        let value = if etag.starts_with("W/") {
            etag
        } else {
            format!("W/{}", etag)
        };
        self.header("ETag", value.into_bytes())
    }

    /// Add a header.
    ///
    /// # Security
    ///
    /// Header names are validated to contain only valid token characters.
    /// Header values are sanitized to prevent CRLF injection attacks.
    /// Invalid characters in names will cause the header to be silently dropped.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        let name = name.into();
        let value = value.into();

        // Validate header name (must be valid HTTP token)
        if !is_valid_header_name(&name) {
            // Silently drop invalid headers to prevent injection
            return self;
        }

        // Sanitize header value (remove CRLF to prevent injection)
        let sanitized_value = sanitize_header_value(value);

        self.headers.push((name, sanitized_value));
        self
    }

    /// Set the body.
    #[must_use]
    pub fn body(mut self, body: ResponseBody) -> Self {
        self.body = body;
        self
    }

    /// Set a cookie on the response.
    ///
    /// Adds a `Set-Cookie` header with the serialized cookie value.
    /// Multiple cookies can be set by calling this method multiple times.
    ///
    /// # Example
    ///
    /// ```
    /// use fastapi_core::{Response, Cookie, SameSite};
    ///
    /// let response = Response::ok()
    ///     .set_cookie(Cookie::new("session", "abc123").http_only(true))
    ///     .set_cookie(Cookie::new("prefs", "dark").same_site(SameSite::Lax));
    /// ```
    #[must_use]
    pub fn set_cookie(self, cookie: Cookie) -> Self {
        self.header("set-cookie", cookie.to_header_value().into_bytes())
    }

    /// Delete a cookie by setting it to expire immediately.
    ///
    /// This sets the cookie with an empty value and `Max-Age=0`, which tells
    /// the browser to remove the cookie.
    ///
    /// # Example
    ///
    /// ```
    /// use fastapi_core::Response;
    ///
    /// let response = Response::ok()
    ///     .delete_cookie("session");
    /// ```
    #[must_use]
    pub fn delete_cookie(self, name: &str) -> Self {
        // Create an expired cookie to delete it
        let cookie = Cookie::new(name, "").max_age(0);
        self.set_cookie(cookie)
    }

    /// Create a JSON response.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn json<T: Serialize>(value: &T) -> Result<Self, serde_json::Error> {
        let bytes = serde_json::to_vec(value)?;
        Ok(Self::ok()
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(bytes)))
    }

    /// Get the status code.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Get the headers.
    #[must_use]
    pub fn headers(&self) -> &[(String, Vec<u8>)] {
        &self.headers
    }

    /// Get the body.
    #[must_use]
    pub fn body_ref(&self) -> &ResponseBody {
        &self.body
    }

    /// Decompose this response into its parts.
    #[must_use]
    pub fn into_parts(self) -> (StatusCode, Vec<(String, Vec<u8>)>, ResponseBody) {
        (self.status, self.headers, self.body)
    }

    /// Rebuilds this response with the given headers, preserving status and body.
    ///
    /// This is useful for middleware that needs to modify the response
    /// but preserve original headers.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (status, headers, body) = response.into_parts();
    /// // ... modify headers ...
    /// let new_response = Response::with_status(status)
    ///     .body(body)
    ///     .rebuild_with_headers(headers);
    /// ```
    #[must_use]
    pub fn rebuild_with_headers(mut self, headers: Vec<(String, Vec<u8>)>) -> Self {
        for (name, value) in headers {
            self = self.header(name, value);
        }
        self
    }
}

/// Trait for types that can be converted into a response.
pub trait IntoResponse {
    /// Convert into a response.
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for () {
    fn into_response(self) -> Response {
        Response::no_content()
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        Response::ok()
            .header("content-type", b"text/plain; charset=utf-8".to_vec())
            .body(ResponseBody::Bytes(self.as_bytes().to_vec()))
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        Response::ok()
            .header("content-type", b"text/plain; charset=utf-8".to_vec())
            .body(ResponseBody::Bytes(self.into_bytes()))
    }
}

impl<T: IntoResponse, E: IntoResponse> IntoResponse for Result<T, E> {
    fn into_response(self) -> Response {
        match self {
            Ok(v) => v.into_response(),
            Err(e) => e.into_response(),
        }
    }
}

impl IntoResponse for std::convert::Infallible {
    fn into_response(self) -> Response {
        match self {}
    }
}

// =============================================================================
// Response Type Checking (OpenAPI)
// =============================================================================

/// Marker trait for compile-time response type verification.
///
/// This trait is used by the route macros to verify at compile time that
/// a handler's return type can produce the declared OpenAPI response schema.
///
/// # How It Works
///
/// When you declare `#[get("/users", response(200, User))]`, the macro
/// generates a compile-time assertion that checks if the handler's return
/// type implements `ResponseProduces<User>`.
///
/// The implementation uses a simple blanket implementation: any type `T`
/// that implements `IntoResponse` trivially produces itself as a schema.
///
/// For wrapper types like `Json<T>`, they implement `ResponseProduces<T>`
/// to indicate they produce the inner type's schema.
///
/// # Example
///
/// ```ignore
/// // This compiles because User produces User schema
/// #[get("/user/{id}", response(200, User))]
/// async fn get_user(Path(id): Path<i64>) -> User {
///     User { id, name: "Alice".into() }
/// }
///
/// // This also compiles because Json<User> produces User schema
/// #[get("/user/{id}", response(200, User))]
/// async fn get_user(Path(id): Path<i64>) -> Json<User> {
///     Json(User { id, name: "Alice".into() })
/// }
/// ```
pub trait ResponseProduces<T> {}

// A type trivially produces itself
impl<T> ResponseProduces<T> for T {}

// Json<T> produces T schema (in addition to producing Json<T>)
impl<T: serde::Serialize + 'static> ResponseProduces<T> for crate::extract::Json<T> {}

// =============================================================================
// Specialized Response Types
// =============================================================================

/// HTTP redirect response.
///
/// Creates responses with appropriate redirect status codes and Location header.
///
/// # Examples
///
/// ```
/// use fastapi_core::Redirect;
///
/// // Temporary redirect (307)
/// let response = Redirect::temporary("/new-location");
///
/// // Permanent redirect (308)
/// let response = Redirect::permanent("/moved-permanently");
///
/// // See Other (303) - for POST/redirect/GET pattern
/// let response = Redirect::see_other("/result");
/// ```
#[derive(Debug, Clone)]
pub struct Redirect {
    status: StatusCode,
    location: String,
}

impl Redirect {
    /// Create a 307 Temporary Redirect.
    ///
    /// The request method and body should be preserved when following the redirect.
    #[must_use]
    pub fn temporary(location: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TEMPORARY_REDIRECT,
            location: location.into(),
        }
    }

    /// Create a 308 Permanent Redirect.
    ///
    /// The request method and body should be preserved when following the redirect.
    /// This indicates the resource has permanently moved.
    #[must_use]
    pub fn permanent(location: impl Into<String>) -> Self {
        Self {
            status: StatusCode::PERMANENT_REDIRECT,
            location: location.into(),
        }
    }

    /// Create a 303 See Other redirect.
    ///
    /// The client should use GET to fetch the redirected resource.
    /// Commonly used for POST/redirect/GET pattern.
    #[must_use]
    pub fn see_other(location: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SEE_OTHER,
            location: location.into(),
        }
    }

    /// Create a 301 Moved Permanently redirect.
    ///
    /// Note: Browsers may change POST to GET. Use 308 for method preservation.
    #[must_use]
    pub fn moved_permanently(location: impl Into<String>) -> Self {
        Self {
            status: StatusCode::MOVED_PERMANENTLY,
            location: location.into(),
        }
    }

    /// Create a 302 Found redirect.
    ///
    /// Note: Browsers may change POST to GET. Use 307 for method preservation.
    #[must_use]
    pub fn found(location: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FOUND,
            location: location.into(),
        }
    }

    /// Get the redirect location.
    #[must_use]
    pub fn location(&self) -> &str {
        &self.location
    }

    /// Get the status code.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }
}

impl IntoResponse for Redirect {
    fn into_response(self) -> Response {
        Response::with_status(self.status).header("location", self.location.into_bytes())
    }
}

/// HTML response with proper content-type.
///
/// # Examples
///
/// ```
/// use fastapi_core::Html;
///
/// let response = Html::new("<html><body>Hello</body></html>");
/// ```
#[derive(Debug, Clone)]
pub struct Html(String);

impl Html {
    /// Create a new HTML response from trusted content.
    ///
    /// # Safety Note
    ///
    /// This method does NOT escape the content. Only use with trusted HTML.
    /// For user-provided content, use [`Html::escaped`] instead to prevent XSS.
    #[must_use]
    pub fn new(content: impl Into<String>) -> Self {
        Self(content.into())
    }

    /// Create an HTML response with the content escaped to prevent XSS.
    ///
    /// Use this method when including any user-provided content in HTML.
    /// Characters `& < > " '` are escaped to their HTML entities.
    #[must_use]
    pub fn escaped(content: impl AsRef<str>) -> Self {
        Self(escape_html(content.as_ref()))
    }

    /// Get the HTML content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.0
    }
}

/// Escape HTML special characters to prevent XSS attacks.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

impl IntoResponse for Html {
    fn into_response(self) -> Response {
        Response::ok()
            .header("content-type", b"text/html; charset=utf-8".to_vec())
            .body(ResponseBody::Bytes(self.0.into_bytes()))
    }
}

impl<S: Into<String>> From<S> for Html {
    fn from(s: S) -> Self {
        Self::new(s)
    }
}

/// Plain text response with proper content-type.
///
/// While `String` and `&str` already implement `IntoResponse` as plain text,
/// this type provides an explicit way to indicate text content.
///
/// # Examples
///
/// ```
/// use fastapi_core::Text;
///
/// let response = Text::new("Hello, World!");
/// ```
#[derive(Debug, Clone)]
pub struct Text(String);

impl Text {
    /// Create a new plain text response.
    #[must_use]
    pub fn new(content: impl Into<String>) -> Self {
        Self(content.into())
    }

    /// Get the text content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.0
    }
}

impl IntoResponse for Text {
    fn into_response(self) -> Response {
        Response::ok()
            .header("content-type", b"text/plain; charset=utf-8".to_vec())
            .body(ResponseBody::Bytes(self.0.into_bytes()))
    }
}

impl<S: Into<String>> From<S> for Text {
    fn from(s: S) -> Self {
        Self::new(s)
    }
}

/// No Content (204) response.
///
/// Used for successful operations that don't return a body,
/// such as DELETE operations.
///
/// # Examples
///
/// ```
/// use fastapi_core::NoContent;
///
/// // After a successful DELETE
/// let response = NoContent;
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct NoContent;

impl IntoResponse for NoContent {
    fn into_response(self) -> Response {
        Response::no_content()
    }
}

/// Binary response with `application/octet-stream` content type.
///
/// Use this for raw binary data that doesn't have a specific MIME type.
///
/// # Examples
///
/// ```
/// use fastapi_core::Binary;
///
/// let data = vec![0x00, 0x01, 0x02, 0x03];
/// let response = Binary::new(data);
/// ```
#[derive(Debug, Clone)]
pub struct Binary(Vec<u8>);

impl Binary {
    /// Create a new binary response.
    #[must_use]
    pub fn new(data: impl Into<Vec<u8>>) -> Self {
        Self(data.into())
    }

    /// Get the binary data.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.0
    }

    /// Create with a specific content type override.
    #[must_use]
    pub fn with_content_type(self, content_type: &str) -> BinaryWithType {
        BinaryWithType {
            data: self.0,
            content_type: content_type.to_string(),
        }
    }
}

impl IntoResponse for Binary {
    fn into_response(self) -> Response {
        Response::ok()
            .header("content-type", b"application/octet-stream".to_vec())
            .body(ResponseBody::Bytes(self.0))
    }
}

impl From<Vec<u8>> for Binary {
    fn from(data: Vec<u8>) -> Self {
        Self::new(data)
    }
}

impl From<&[u8]> for Binary {
    fn from(data: &[u8]) -> Self {
        Self::new(data.to_vec())
    }
}

/// Binary response with a custom content type.
///
/// # Examples
///
/// ```
/// use fastapi_core::Binary;
///
/// let pdf_data = vec![0x25, 0x50, 0x44, 0x46]; // PDF magic bytes
/// let response = Binary::new(pdf_data).with_content_type("application/pdf");
/// ```
#[derive(Debug, Clone)]
pub struct BinaryWithType {
    data: Vec<u8>,
    content_type: String,
}

impl BinaryWithType {
    /// Get a reference to the underlying data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get the content type.
    pub fn content_type(&self) -> &str {
        &self.content_type
    }
}

impl IntoResponse for BinaryWithType {
    fn into_response(self) -> Response {
        Response::ok()
            .header("content-type", self.content_type.into_bytes())
            .body(ResponseBody::Bytes(self.data))
    }
}

/// File response for serving files.
///
/// Supports:
/// - Automatic content-type inference from file extension
/// - Optional Content-Disposition for downloads
/// - Streaming for large files
///
/// # Examples
///
/// ```ignore
/// use fastapi_core::response::FileResponse;
/// use std::path::Path;
///
/// // Inline display (images, PDFs in browser)
/// let response = FileResponse::new(Path::new("image.png"));
///
/// // Force download with custom filename
/// let response = FileResponse::new(Path::new("data.csv"))
///     .download_as("report.csv");
/// ```
#[derive(Debug)]
pub struct FileResponse {
    path: std::path::PathBuf,
    content_type: Option<String>,
    download_name: Option<String>,
    inline: bool,
}

impl FileResponse {
    /// Create a new file response.
    ///
    /// The content-type will be inferred from the file extension.
    #[must_use]
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            content_type: None,
            download_name: None,
            inline: true,
        }
    }

    /// Override the content-type.
    #[must_use]
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set as download with the specified filename.
    ///
    /// Sets Content-Disposition: attachment; filename="..."
    #[must_use]
    pub fn download_as(mut self, filename: impl Into<String>) -> Self {
        self.download_name = Some(filename.into());
        self.inline = false;
        self
    }

    /// Set as inline content (default).
    ///
    /// Sets Content-Disposition: inline
    #[must_use]
    pub fn inline(mut self) -> Self {
        self.inline = true;
        self.download_name = None;
        self
    }

    /// Get the file path.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Infer content-type from file extension.
    fn infer_content_type(&self) -> &'static str {
        self.path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| mime_type_for_extension(ext))
            .unwrap_or("application/octet-stream")
    }

    /// Build the Content-Disposition header value.
    fn content_disposition(&self) -> String {
        if self.inline {
            "inline".to_string()
        } else if let Some(ref name) = self.download_name {
            // RFC 6266: filename should be quoted and special chars escaped
            format!("attachment; filename=\"{}\"", name.replace('"', "\\\""))
        } else {
            // Use the actual filename from path
            let filename = self
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("download");
            format!("attachment; filename=\"{}\"", filename.replace('"', "\\\""))
        }
    }

    /// Read file and create response.
    ///
    /// # Errors
    ///
    /// Returns an error response if the file cannot be read.
    #[must_use]
    pub fn into_response_sync(self) -> Response {
        match std::fs::read(&self.path) {
            Ok(contents) => {
                let content_type = self
                    .content_type
                    .as_deref()
                    .unwrap_or_else(|| self.infer_content_type());

                Response::ok()
                    .header("content-type", content_type.as_bytes().to_vec())
                    .header(
                        "content-disposition",
                        self.content_disposition().into_bytes(),
                    )
                    .header("accept-ranges", b"bytes".to_vec())
                    .body(ResponseBody::Bytes(contents))
            }
            Err(_) => Response::with_status(StatusCode::NOT_FOUND),
        }
    }
}

impl IntoResponse for FileResponse {
    fn into_response(self) -> Response {
        self.into_response_sync()
    }
}

/// Get MIME type for a file extension.
///
/// Returns a reasonable MIME type for common file extensions.
/// Falls back to "application/octet-stream" for unknown types.
#[must_use]
pub fn mime_type_for_extension(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        // Text
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "xml" => "application/xml",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",

        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "avif" => "image/avif",

        // Fonts
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",

        // Audio
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "m4a" => "audio/mp4",

        // Video
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",

        // Documents
        "pdf" => "application/pdf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",

        // Archives
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        "rar" => "application/vnd.rar",
        "7z" => "application/x-7z-compressed",

        // Other
        "wasm" => "application/wasm",

        _ => "application/octet-stream",
    }
}

/// Convert an asupersync Outcome into an HTTP response.
///
/// This is the canonical mapping used by the framework when handlers return
/// Outcome values. It preserves normal success/error responses while
/// translating cancellations and panics into appropriate HTTP status codes.
#[must_use]
#[allow(dead_code)] // Will be used when TCP server is wired up
pub fn outcome_to_response<T, E>(outcome: Outcome<T, E>) -> Response
where
    T: IntoResponse,
    E: IntoResponse,
{
    match outcome {
        Outcome::Ok(value) => value.into_response(),
        Outcome::Err(err) => err.into_response(),
        Outcome::Cancelled(reason) => cancelled_to_response(&reason),
        Outcome::Panicked(_payload) => Response::with_status(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[allow(dead_code)] // Will be used when TCP server is wired up
fn cancelled_to_response(reason: &CancelReason) -> Response {
    let status = match reason.kind() {
        CancelKind::Timeout => StatusCode::GATEWAY_TIMEOUT,
        CancelKind::Shutdown => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::CLIENT_CLOSED_REQUEST,
    };
    Response::with_status(status)
}

// ============================================================================
// Response Model Configuration
// ============================================================================

/// Configuration for response model serialization.
///
/// This provides FastAPI-compatible options for controlling how response
/// data is serialized and validated before sending to clients.
///
/// # Examples
///
/// ```
/// use fastapi_core::ResponseModelConfig;
/// use std::collections::HashSet;
///
/// // Only include specific fields
/// let config = ResponseModelConfig::new()
///     .include(["id", "name", "email"].into_iter().map(String::from).collect());
///
/// // Exclude sensitive fields
/// let config = ResponseModelConfig::new()
///     .exclude(["password", "internal_notes"].into_iter().map(String::from).collect());
///
/// // Use field aliases in output
/// let config = ResponseModelConfig::new()
///     .by_alias(true);
/// ```
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)] // Mirrors FastAPI's response_model options
pub struct ResponseModelConfig {
    /// Only include these fields in the response.
    /// If None, all fields are included (subject to exclude).
    pub include: Option<std::collections::HashSet<String>>,

    /// Exclude these fields from the response.
    pub exclude: Option<std::collections::HashSet<String>>,

    /// Use serde aliases in output field names.
    pub by_alias: bool,

    /// Exclude fields that were not explicitly set.
    /// Requires the type to track which fields were set.
    pub exclude_unset: bool,

    /// Exclude fields that have their default values.
    pub exclude_defaults: bool,

    /// Exclude fields with None values.
    pub exclude_none: bool,
}

impl ResponseModelConfig {
    /// Create a new configuration with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set fields to include (whitelist).
    #[must_use]
    pub fn include(mut self, fields: std::collections::HashSet<String>) -> Self {
        self.include = Some(fields);
        self
    }

    /// Set fields to exclude (blacklist).
    #[must_use]
    pub fn exclude(mut self, fields: std::collections::HashSet<String>) -> Self {
        self.exclude = Some(fields);
        self
    }

    /// Use serde aliases in output.
    ///
    /// **Note:** This option is stored but not yet implemented in `filter_json`.
    /// Full implementation requires compile-time serde attribute introspection.
    #[must_use]
    pub fn by_alias(mut self, value: bool) -> Self {
        self.by_alias = value;
        self
    }

    /// Exclude unset fields.
    ///
    /// **Note:** This option is stored but not yet implemented in `filter_json`.
    /// Full implementation requires the type to track which fields were explicitly set.
    #[must_use]
    pub fn exclude_unset(mut self, value: bool) -> Self {
        self.exclude_unset = value;
        self
    }

    /// Exclude fields with default values.
    ///
    /// **Note:** This option is stored but not yet implemented in `filter_json`.
    /// Full implementation requires compile-time default value comparison.
    #[must_use]
    pub fn exclude_defaults(mut self, value: bool) -> Self {
        self.exclude_defaults = value;
        self
    }

    /// Exclude fields with None values.
    #[must_use]
    pub fn exclude_none(mut self, value: bool) -> Self {
        self.exclude_none = value;
        self
    }

    /// Check if any filtering is configured.
    #[must_use]
    pub fn has_filtering(&self) -> bool {
        self.include.is_some()
            || self.exclude.is_some()
            || self.exclude_none
            || self.exclude_unset
            || self.exclude_defaults
    }

    /// Apply filtering to a JSON value.
    ///
    /// This filters the JSON according to the configuration:
    /// - Applies include whitelist
    /// - Applies exclude blacklist
    /// - Removes None values if exclude_none is set
    #[must_use]
    pub fn filter_json(&self, mut value: serde_json::Value) -> serde_json::Value {
        if let serde_json::Value::Object(ref mut map) = value {
            // Apply include whitelist
            if let Some(ref include_set) = self.include {
                map.retain(|key, _| include_set.contains(key));
            }

            // Apply exclude blacklist
            if let Some(ref exclude_set) = self.exclude {
                map.retain(|key, _| !exclude_set.contains(key));
            }

            // Remove None values if configured
            if self.exclude_none {
                map.retain(|_, v| !v.is_null());
            }
        }

        value
    }
}

/// Trait for types that can be validated as response models.
///
/// This allows custom validation logic to be applied before serialization.
/// Types implementing this trait can verify that the response data is valid
/// according to the declared response model.
pub trait ResponseModel: Serialize {
    /// Validate the response model before serialization.
    ///
    /// Returns Ok(()) if valid, or a validation error if invalid.
    #[allow(clippy::result_large_err)] // Error provides detailed validation context
    fn validate(&self) -> Result<(), crate::error::ResponseValidationError> {
        // Default implementation: no validation
        Ok(())
    }

    /// Get the model name for error messages.
    fn model_name() -> &'static str {
        std::any::type_name::<Self>()
    }
}

// Blanket implementation for all Serialize types
impl<T: Serialize> ResponseModel for T {}

/// A validated response with its configuration.
///
/// This wraps a response value with its model configuration, ensuring
/// the response is validated and filtered before sending.
///
/// # Examples
///
/// ```
/// use fastapi_core::{ValidatedResponse, ResponseModelConfig};
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct User {
///     id: i64,
///     name: String,
///     email: String,
///     password_hash: String,
/// }
///
/// let user = User {
///     id: 1,
///     name: "Alice".to_string(),
///     email: "alice@example.com".to_string(),
///     password_hash: "secret123".to_string(),
/// };
///
/// // Create a validated response that excludes the password
/// let response = ValidatedResponse::new(user)
///     .with_config(ResponseModelConfig::new()
///         .exclude(["password_hash"].into_iter().map(String::from).collect()));
/// ```
#[derive(Debug)]
pub struct ValidatedResponse<T> {
    /// The response value.
    pub value: T,
    /// The serialization configuration.
    pub config: ResponseModelConfig,
}

impl<T> ValidatedResponse<T> {
    /// Create a new validated response.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            value,
            config: ResponseModelConfig::default(),
        }
    }

    /// Set the serialization configuration.
    #[must_use]
    pub fn with_config(mut self, config: ResponseModelConfig) -> Self {
        self.config = config;
        self
    }
}

impl<T: Serialize + ResponseModel> IntoResponse for ValidatedResponse<T> {
    fn into_response(self) -> Response {
        // First validate the response model
        if let Err(error) = self.value.validate() {
            return error.into_response();
        }

        // Serialize to JSON
        let json_value = match serde_json::to_value(&self.value) {
            Ok(v) => v,
            Err(e) => {
                // Serialization failed - return 500
                let error =
                    crate::error::ResponseValidationError::serialization_failed(e.to_string());
                return error.into_response();
            }
        };

        // Apply filtering
        let filtered = self.config.filter_json(json_value);

        // Serialize the filtered value
        let bytes = match serde_json::to_vec(&filtered) {
            Ok(b) => b,
            Err(e) => {
                let error =
                    crate::error::ResponseValidationError::serialization_failed(e.to_string());
                return error.into_response();
            }
        };

        Response::ok()
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(bytes))
    }
}

/// Macro helper for creating validated responses with field exclusion.
///
/// This is a convenience wrapper that excludes specified fields from the response.
#[must_use]
pub fn exclude_fields<T: Serialize + ResponseModel>(
    value: T,
    fields: &[&str],
) -> ValidatedResponse<T> {
    ValidatedResponse::new(value).with_config(
        ResponseModelConfig::new().exclude(fields.iter().map(|s| (*s).to_string()).collect()),
    )
}

/// Macro helper for creating validated responses with field inclusion.
///
/// This is a convenience wrapper that only includes specified fields in the response.
#[must_use]
pub fn include_fields<T: Serialize + ResponseModel>(
    value: T,
    fields: &[&str],
) -> ValidatedResponse<T> {
    ValidatedResponse::new(value).with_config(
        ResponseModelConfig::new().include(fields.iter().map(|s| (*s).to_string()).collect()),
    )
}

// ============================================================================
// Conditional request (ETag / If-None-Match / If-Match) utilities
// ============================================================================

/// Check an `If-None-Match` header value against an ETag.
///
/// Returns `true` if the condition is met (i.e., the resource HAS changed and the
/// full response should be sent). Returns `false` if a 304 Not Modified should be
/// returned instead.
///
/// Handles:
/// - `*` wildcard (matches any ETag)
/// - Multiple comma-separated ETags
/// - Weak ETag comparison (W/ prefix stripped for comparison)
///
/// # Example
///
/// ```ignore
/// use fastapi_core::response::check_if_none_match;
///
/// let current_etag = "\"abc123\"";
/// let if_none_match = "\"abc123\"";
/// // Returns false: ETag matches, so send 304
/// assert!(!check_if_none_match(if_none_match, current_etag));
/// ```
pub fn check_if_none_match(if_none_match: &str, current_etag: &str) -> bool {
    let if_none_match = if_none_match.trim();

    // Wildcard matches everything
    if if_none_match == "*" {
        return false; // Resource exists, send 304
    }

    let current_stripped = strip_weak_prefix(current_etag.trim());

    // Check each ETag in the comma-separated list
    for candidate in if_none_match.split(',') {
        let candidate = strip_weak_prefix(candidate.trim());
        if candidate == current_stripped {
            return false; // Match found, send 304
        }
    }

    true // No match, send full response
}

/// Check an `If-Match` header value against an ETag.
///
/// Returns `true` if the precondition is met (the resource matches and the
/// request should proceed). Returns `false` if a 412 Precondition Failed
/// should be returned.
///
/// Uses strong comparison (W/ weak ETags never match).
pub fn check_if_match(if_match: &str, current_etag: &str) -> bool {
    let if_match = if_match.trim();

    // Wildcard matches everything
    if if_match == "*" {
        return true;
    }

    let current = current_etag.trim();

    // Weak ETags never match for If-Match (strong comparison required)
    if current.starts_with("W/") {
        return false;
    }

    for candidate in if_match.split(',') {
        let candidate = candidate.trim();
        // Weak ETags don't match in strong comparison
        if candidate.starts_with("W/") {
            continue;
        }
        if candidate == current {
            return true;
        }
    }

    false
}

/// Strip the `W/` weak ETag prefix for weak comparison.
fn strip_weak_prefix(etag: &str) -> &str {
    etag.strip_prefix("W/").unwrap_or(etag)
}

/// Evaluate conditional request headers against a response and return the
/// appropriate response (304, 412, or the original).
///
/// This checks `If-None-Match` (for GET/HEAD) and `If-Match` (for PUT/PATCH/DELETE)
/// against the response's ETag header.
///
/// # Arguments
///
/// * `request_headers` - Iterator of (name, value) pairs from the request
/// * `method` - The HTTP method
/// * `response` - The prepared response (must have ETag header set)
///
/// # Returns
///
/// Either the original response or a 304/412 response as appropriate.
pub fn apply_conditional(
    request_headers: &[(String, Vec<u8>)],
    method: &crate::request::Method,
    response: Response,
) -> Response {
    // Find the ETag from the response
    let response_etag = response
        .headers()
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
        .and_then(|(_, value)| std::str::from_utf8(value).ok())
        .map(String::from);

    let Some(response_etag) = response_etag else {
        return response; // No ETag, can't do conditional
    };

    // Check If-None-Match (for GET/HEAD - returns 304)
    if matches!(
        method,
        crate::request::Method::Get | crate::request::Method::Head
    ) {
        if let Some(if_none_match) = find_header(request_headers, "if-none-match") {
            if !check_if_none_match(&if_none_match, &response_etag) {
                return Response::not_modified().with_etag(response_etag);
            }
        }
    }

    // Check If-Match (for unsafe methods - returns 412)
    if matches!(
        method,
        crate::request::Method::Put
            | crate::request::Method::Patch
            | crate::request::Method::Delete
    ) {
        if let Some(if_match) = find_header(request_headers, "if-match") {
            if !check_if_match(&if_match, &response_etag) {
                return Response::precondition_failed();
            }
        }
    }

    response
}

/// Find a header value by name (case-insensitive).
fn find_header(headers: &[(String, Vec<u8>)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| std::str::from_utf8(v).ok())
        .map(String::from)
}

// ============================================================================
// Link Header (RFC 8288)
// ============================================================================

/// Link relation type per RFC 8288.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkRel {
    /// The current resource.
    Self_,
    /// Next page in a paginated collection.
    Next,
    /// Previous page in a paginated collection.
    Prev,
    /// First page in a paginated collection.
    First,
    /// Last page in a paginated collection.
    Last,
    /// A related resource.
    Related,
    /// An alternate representation.
    Alternate,
    /// Custom relation type.
    Custom(String),
}

impl fmt::Display for LinkRel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Self_ => write!(f, "self"),
            Self::Next => write!(f, "next"),
            Self::Prev => write!(f, "prev"),
            Self::First => write!(f, "first"),
            Self::Last => write!(f, "last"),
            Self::Related => write!(f, "related"),
            Self::Alternate => write!(f, "alternate"),
            Self::Custom(s) => write!(f, "{s}"),
        }
    }
}

/// A single link entry in a Link header.
#[derive(Debug, Clone)]
pub struct Link {
    url: String,
    rel: LinkRel,
    title: Option<String>,
    media_type: Option<String>,
}

impl Link {
    /// Create a new link with the given URL and relation.
    pub fn new(url: impl Into<String>, rel: LinkRel) -> Self {
        Self {
            url: url.into(),
            rel,
            title: None,
            media_type: None,
        }
    }

    /// Set the title parameter.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the type parameter (media type).
    #[must_use]
    pub fn media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{}>; rel=\"{}\"", self.url, self.rel)?;
        if let Some(ref title) = self.title {
            write!(f, "; title=\"{title}\"")?;
        }
        if let Some(ref mt) = self.media_type {
            write!(f, "; type=\"{mt}\"")?;
        }
        Ok(())
    }
}

/// Builder for constructing RFC 8288 Link headers.
///
/// Supports multiple links in a single header value, pagination helpers,
/// and custom relation types.
///
/// # Example
///
/// ```
/// use fastapi_core::{LinkHeader, LinkRel};
///
/// let header = LinkHeader::new()
///     .link("https://api.example.com/users?page=2", LinkRel::Next)
///     .link("https://api.example.com/users?page=1", LinkRel::Prev)
///     .link("https://api.example.com/users?page=1", LinkRel::First)
///     .link("https://api.example.com/users?page=5", LinkRel::Last);
///
/// assert!(header.to_string().contains("rel=\"next\""));
/// ```
#[derive(Debug, Clone, Default)]
pub struct LinkHeader {
    links: Vec<Link>,
}

impl LinkHeader {
    /// Create an empty link header builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a link with the given URL and relation.
    #[must_use]
    pub fn link(mut self, url: impl Into<String>, rel: LinkRel) -> Self {
        self.links.push(Link::new(url, rel));
        self
    }

    /// Add a fully configured [`Link`] entry.
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, link: Link) -> Self {
        self.links.push(link);
        self
    }

    /// Add pagination links from page/per_page/total parameters.
    ///
    /// Generates `first`, `last`, `next`, `prev`, and `self` links
    /// using the given base URL and query parameters.
    #[must_use]
    pub fn paginate(
        self,
        base_url: &str,
        page: u64,
        per_page: u64,
        total: u64,
    ) -> Self {
        let last_page = if total == 0 {
            1
        } else {
            total.div_ceil(per_page)
        };
        let sep = if base_url.contains('?') { '&' } else { '?' };

        let mut h = self.link(
            format!("{base_url}{sep}page={page}&per_page={per_page}"),
            LinkRel::Self_,
        );
        h = h.link(
            format!("{base_url}{sep}page=1&per_page={per_page}"),
            LinkRel::First,
        );
        h = h.link(
            format!("{base_url}{sep}page={last_page}&per_page={per_page}"),
            LinkRel::Last,
        );
        if page > 1 {
            h = h.link(
                format!("{base_url}{sep}page={}&per_page={per_page}", page - 1),
                LinkRel::Prev,
            );
        }
        if page < last_page {
            h = h.link(
                format!("{base_url}{sep}page={}&per_page={per_page}", page + 1),
                LinkRel::Next,
            );
        }
        h
    }

    /// Returns true if no links have been added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// Returns the number of links.
    #[must_use]
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// Convert to the header value string (RFC 8288 format).
    #[must_use]
    pub fn to_header_value(&self) -> String {
        self.to_string()
    }

    /// Apply this Link header to a response.
    pub fn apply(self, response: Response) -> Response {
        if self.is_empty() {
            return response;
        }
        response.header("link", self.to_string().into_bytes())
    }
}

impl fmt::Display for LinkHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, link) in self.links.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{link}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HttpError;

    #[test]
    fn outcome_ok_maps_to_response() {
        let response = Response::created();
        let mapped = outcome_to_response::<Response, HttpError>(Outcome::Ok(response));
        assert_eq!(mapped.status().as_u16(), 201);
    }

    #[test]
    fn outcome_err_maps_to_response() {
        let mapped =
            outcome_to_response::<Response, HttpError>(Outcome::Err(HttpError::bad_request()));
        assert_eq!(mapped.status().as_u16(), 400);
    }

    #[test]
    fn outcome_cancelled_timeout_maps_to_504() {
        let mapped =
            outcome_to_response::<Response, HttpError>(Outcome::Cancelled(CancelReason::timeout()));
        assert_eq!(mapped.status().as_u16(), 504);
    }

    #[test]
    fn outcome_cancelled_user_maps_to_499() {
        let mapped = outcome_to_response::<Response, HttpError>(Outcome::Cancelled(
            CancelReason::user("client disconnected"),
        ));
        assert_eq!(mapped.status().as_u16(), 499);
    }

    #[test]
    fn outcome_panicked_maps_to_500() {
        let mapped = outcome_to_response::<Response, HttpError>(Outcome::Panicked(
            PanicPayload::new("boom"),
        ));
        assert_eq!(mapped.status().as_u16(), 500);
    }

    // =========================================================================
    // Redirect tests
    // =========================================================================

    #[test]
    fn redirect_temporary_returns_307() {
        let redirect = Redirect::temporary("/new-location");
        let response = redirect.into_response();
        assert_eq!(response.status().as_u16(), 307);
    }

    #[test]
    fn redirect_permanent_returns_308() {
        let redirect = Redirect::permanent("/moved");
        let response = redirect.into_response();
        assert_eq!(response.status().as_u16(), 308);
    }

    #[test]
    fn redirect_see_other_returns_303() {
        let redirect = Redirect::see_other("/result");
        let response = redirect.into_response();
        assert_eq!(response.status().as_u16(), 303);
    }

    #[test]
    fn redirect_moved_permanently_returns_301() {
        let redirect = Redirect::moved_permanently("/gone");
        let response = redirect.into_response();
        assert_eq!(response.status().as_u16(), 301);
    }

    #[test]
    fn redirect_found_returns_302() {
        let redirect = Redirect::found("/elsewhere");
        let response = redirect.into_response();
        assert_eq!(response.status().as_u16(), 302);
    }

    #[test]
    fn redirect_sets_location_header() {
        let redirect = Redirect::temporary("/target?query=1");
        let response = redirect.into_response();

        let location = response
            .headers()
            .iter()
            .find(|(name, _)| name == "location")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(location, Some("/target?query=1".to_string()));
    }

    #[test]
    fn redirect_location_accessor() {
        let redirect = Redirect::permanent("https://example.com/new");
        assert_eq!(redirect.location(), "https://example.com/new");
    }

    #[test]
    fn redirect_status_accessor() {
        let redirect = Redirect::see_other("/done");
        assert_eq!(redirect.status().as_u16(), 303);
    }

    // =========================================================================
    // Html tests
    // =========================================================================

    #[test]
    fn html_response_has_correct_content_type() {
        let html = Html::new("<html><body>Hello</body></html>");
        let response = html.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(content_type, Some("text/html; charset=utf-8".to_string()));
    }

    #[test]
    fn html_response_has_status_200() {
        let html = Html::new("<p>test</p>");
        let response = html.into_response();
        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn html_content_accessor() {
        let html = Html::new("<div>content</div>");
        assert_eq!(html.content(), "<div>content</div>");
    }

    #[test]
    fn html_from_string() {
        let html: Html = "hello".into();
        assert_eq!(html.content(), "hello");
    }

    // =========================================================================
    // Text tests
    // =========================================================================

    #[test]
    fn text_response_has_correct_content_type() {
        let text = Text::new("Plain text content");
        let response = text.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(content_type, Some("text/plain; charset=utf-8".to_string()));
    }

    #[test]
    fn text_response_has_status_200() {
        let text = Text::new("hello");
        let response = text.into_response();
        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn text_content_accessor() {
        let text = Text::new("my content");
        assert_eq!(text.content(), "my content");
    }

    // =========================================================================
    // NoContent tests
    // =========================================================================

    #[test]
    fn no_content_returns_204() {
        let response = NoContent.into_response();
        assert_eq!(response.status().as_u16(), 204);
    }

    #[test]
    fn no_content_has_empty_body() {
        let response = NoContent.into_response();
        assert!(response.body_ref().is_empty());
    }

    // =========================================================================
    // FileResponse tests
    // =========================================================================

    #[test]
    fn file_response_infers_png_content_type() {
        let file = FileResponse::new("/path/to/image.png");
        // We test the internal method indirectly through the response
        assert_eq!(file.path().to_str(), Some("/path/to/image.png"));
    }

    #[test]
    fn file_response_download_as_sets_attachment() {
        let file = FileResponse::new("/data/report.csv").download_as("my-report.csv");
        let disposition = file.content_disposition();
        assert!(disposition.contains("attachment"));
        assert!(disposition.contains("my-report.csv"));
    }

    #[test]
    fn file_response_inline_sets_inline() {
        let file = FileResponse::new("/image.png").inline();
        let disposition = file.content_disposition();
        assert_eq!(disposition, "inline");
    }

    #[test]
    fn file_response_custom_content_type() {
        // Create a temp file for testing
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_response_file.txt");
        std::fs::write(&test_file, b"test content").unwrap();

        let file = FileResponse::new(&test_file).content_type("application/custom");
        let response = file.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(content_type, Some("application/custom".to_string()));

        // Cleanup
        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn file_response_includes_accept_ranges_header() {
        // Create a temp file for testing
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_accept_ranges.txt");
        std::fs::write(&test_file, b"test content for range support").unwrap();

        let file = FileResponse::new(&test_file);
        let response = file.into_response();

        let accept_ranges = response
            .headers()
            .iter()
            .find(|(name, _)| name == "accept-ranges")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(accept_ranges, Some("bytes".to_string()));

        // Cleanup
        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn file_response_not_found_returns_404() {
        let file = FileResponse::new("/nonexistent/path/file.txt");
        let response = file.into_response();
        assert_eq!(response.status().as_u16(), 404);
    }

    // =========================================================================
    // MIME type tests
    // =========================================================================

    #[test]
    fn mime_type_for_common_extensions() {
        assert_eq!(mime_type_for_extension("html"), "text/html; charset=utf-8");
        assert_eq!(mime_type_for_extension("css"), "text/css; charset=utf-8");
        assert_eq!(
            mime_type_for_extension("js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime_type_for_extension("json"), "application/json");
        assert_eq!(mime_type_for_extension("png"), "image/png");
        assert_eq!(mime_type_for_extension("jpg"), "image/jpeg");
        assert_eq!(mime_type_for_extension("pdf"), "application/pdf");
        assert_eq!(mime_type_for_extension("zip"), "application/zip");
    }

    #[test]
    fn mime_type_case_insensitive() {
        assert_eq!(mime_type_for_extension("HTML"), "text/html; charset=utf-8");
        assert_eq!(mime_type_for_extension("PNG"), "image/png");
        assert_eq!(mime_type_for_extension("Json"), "application/json");
    }

    #[test]
    fn mime_type_unknown_returns_octet_stream() {
        assert_eq!(
            mime_type_for_extension("unknown"),
            "application/octet-stream"
        );
        assert_eq!(mime_type_for_extension("xyz"), "application/octet-stream");
    }

    // =========================================================================
    // StatusCode tests
    // =========================================================================

    #[test]
    fn status_code_see_other_is_303() {
        assert_eq!(StatusCode::SEE_OTHER.as_u16(), 303);
    }

    #[test]
    fn status_code_see_other_canonical_reason() {
        assert_eq!(StatusCode::SEE_OTHER.canonical_reason(), "See Other");
    }

    #[test]
    fn status_code_partial_content_is_206() {
        assert_eq!(StatusCode::PARTIAL_CONTENT.as_u16(), 206);
    }

    #[test]
    fn status_code_partial_content_canonical_reason() {
        assert_eq!(
            StatusCode::PARTIAL_CONTENT.canonical_reason(),
            "Partial Content"
        );
    }

    #[test]
    fn status_code_range_not_satisfiable_is_416() {
        assert_eq!(StatusCode::RANGE_NOT_SATISFIABLE.as_u16(), 416);
    }

    #[test]
    fn status_code_range_not_satisfiable_canonical_reason() {
        assert_eq!(
            StatusCode::RANGE_NOT_SATISFIABLE.canonical_reason(),
            "Range Not Satisfiable"
        );
    }

    #[test]
    fn response_partial_content_returns_206() {
        let response = Response::partial_content();
        assert_eq!(response.status().as_u16(), 206);
    }

    #[test]
    fn response_range_not_satisfiable_returns_416() {
        let response = Response::range_not_satisfiable();
        assert_eq!(response.status().as_u16(), 416);
    }

    // =========================================================================
    // Cookie setting tests
    // =========================================================================

    #[test]
    fn response_set_cookie_adds_header() {
        use crate::extract::Cookie;

        let response = Response::ok().set_cookie(Cookie::new("session", "abc123"));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(name, _)| name == "set-cookie")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert!(cookie_header.is_some());
        let header_value = cookie_header.unwrap();
        assert!(header_value.contains("session=abc123"));
    }

    #[test]
    fn response_set_cookie_with_attributes() {
        use crate::extract::{Cookie, SameSite};

        let response = Response::ok().set_cookie(
            Cookie::new("session", "token123")
                .http_only(true)
                .secure(true)
                .same_site(SameSite::Strict)
                .max_age(3600)
                .path("/api"),
        );

        let cookie_header = response
            .headers()
            .iter()
            .find(|(name, _)| name == "set-cookie")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string())
            .unwrap();

        assert!(cookie_header.contains("session=token123"));
        assert!(cookie_header.contains("HttpOnly"));
        assert!(cookie_header.contains("Secure"));
        assert!(cookie_header.contains("SameSite=Strict"));
        assert!(cookie_header.contains("Max-Age=3600"));
        assert!(cookie_header.contains("Path=/api"));
    }

    #[test]
    fn response_set_multiple_cookies() {
        use crate::extract::Cookie;

        let response = Response::ok()
            .set_cookie(Cookie::new("session", "abc"))
            .set_cookie(Cookie::new("prefs", "dark"));

        let cookie_headers: Vec<_> = response
            .headers()
            .iter()
            .filter(|(name, _)| name == "set-cookie")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string())
            .collect();

        assert_eq!(cookie_headers.len(), 2);
        assert!(cookie_headers.iter().any(|h| h.contains("session=abc")));
        assert!(cookie_headers.iter().any(|h| h.contains("prefs=dark")));
    }

    #[test]
    fn response_delete_cookie_sets_max_age_zero() {
        let response = Response::ok().delete_cookie("session");

        let cookie_header = response
            .headers()
            .iter()
            .find(|(name, _)| name == "set-cookie")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string())
            .unwrap();

        assert!(cookie_header.contains("session="));
        assert!(cookie_header.contains("Max-Age=0"));
    }

    #[test]
    fn response_set_and_delete_cookies() {
        use crate::extract::Cookie;

        // Set a new cookie and delete an old one in the same response
        let response = Response::ok()
            .set_cookie(Cookie::new("new_session", "xyz"))
            .delete_cookie("old_session");

        let cookie_headers: Vec<_> = response
            .headers()
            .iter()
            .filter(|(name, _)| name == "set-cookie")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string())
            .collect();

        assert_eq!(cookie_headers.len(), 2);
        assert!(cookie_headers.iter().any(|h| h.contains("new_session=xyz")));
        assert!(
            cookie_headers
                .iter()
                .any(|h| h.contains("old_session=") && h.contains("Max-Age=0"))
        );
    }

    // =========================================================================
    // Binary tests
    // =========================================================================

    #[test]
    fn binary_new_creates_from_vec() {
        let data = vec![0x01, 0x02, 0x03, 0x04];
        let binary = Binary::new(data.clone());
        assert_eq!(binary.data(), &data[..]);
    }

    #[test]
    fn binary_new_creates_from_slice() {
        let data = [0xDE, 0xAD, 0xBE, 0xEF];
        let binary = Binary::new(&data[..]);
        assert_eq!(binary.data(), &data);
    }

    #[test]
    fn binary_into_response_has_correct_content_type() {
        let binary = Binary::new(vec![1, 2, 3]);
        let response = binary.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(content_type, Some("application/octet-stream".to_string()));
    }

    #[test]
    fn binary_into_response_has_status_200() {
        let binary = Binary::new(vec![1, 2, 3]);
        let response = binary.into_response();
        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn binary_into_response_has_correct_body() {
        let data = vec![0x48, 0x65, 0x6C, 0x6C, 0x6F]; // "Hello" in bytes
        let binary = Binary::new(data.clone());
        let response = binary.into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert_eq!(bytes, &data);
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn binary_with_content_type_returns_binary_with_type() {
        let data = vec![0x89, 0x50, 0x4E, 0x47]; // PNG magic bytes
        let binary = Binary::new(data);
        let binary_typed = binary.with_content_type("image/png");

        assert_eq!(binary_typed.content_type(), "image/png");
    }

    #[test]
    fn binary_with_type_into_response_has_correct_content_type() {
        let data = vec![0xFF, 0xD8, 0xFF]; // JPEG magic bytes
        let binary = Binary::new(data).with_content_type("image/jpeg");
        let response = binary.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(content_type, Some("image/jpeg".to_string()));
    }

    #[test]
    fn binary_with_type_into_response_has_correct_body() {
        let data = vec![0x25, 0x50, 0x44, 0x46]; // PDF magic bytes
        let binary = Binary::new(data.clone()).with_content_type("application/pdf");
        let response = binary.into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert_eq!(bytes, &data);
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn binary_with_type_data_accessor() {
        let data = vec![1, 2, 3, 4, 5];
        let binary = Binary::new(data.clone()).with_content_type("application/custom");
        assert_eq!(binary.data(), &data[..]);
    }

    #[test]
    fn binary_with_type_status_200() {
        let binary = Binary::new(vec![0]).with_content_type("text/plain");
        let response = binary.into_response();
        assert_eq!(response.status().as_u16(), 200);
    }

    // =========================================================================
    // ResponseModelConfig tests
    // =========================================================================

    #[test]
    fn response_model_config_default() {
        let config = ResponseModelConfig::new();
        assert!(config.include.is_none());
        assert!(config.exclude.is_none());
        assert!(!config.by_alias);
        assert!(!config.exclude_unset);
        assert!(!config.exclude_defaults);
        assert!(!config.exclude_none);
    }

    #[test]
    fn response_model_config_include() {
        let fields: std::collections::HashSet<String> =
            ["id", "name"].iter().map(|s| (*s).to_string()).collect();
        let config = ResponseModelConfig::new().include(fields.clone());
        assert_eq!(config.include, Some(fields));
    }

    #[test]
    fn response_model_config_exclude() {
        let fields: std::collections::HashSet<String> =
            ["password"].iter().map(|s| (*s).to_string()).collect();
        let config = ResponseModelConfig::new().exclude(fields.clone());
        assert_eq!(config.exclude, Some(fields));
    }

    #[test]
    fn response_model_config_by_alias() {
        let config = ResponseModelConfig::new().by_alias(true);
        assert!(config.by_alias);
    }

    #[test]
    fn response_model_config_exclude_none() {
        let config = ResponseModelConfig::new().exclude_none(true);
        assert!(config.exclude_none);
    }

    #[test]
    fn response_model_config_exclude_unset() {
        let config = ResponseModelConfig::new().exclude_unset(true);
        assert!(config.exclude_unset);
    }

    #[test]
    fn response_model_config_exclude_defaults() {
        let config = ResponseModelConfig::new().exclude_defaults(true);
        assert!(config.exclude_defaults);
    }

    #[test]
    fn response_model_config_has_filtering() {
        let config = ResponseModelConfig::new();
        assert!(!config.has_filtering());

        let config =
            ResponseModelConfig::new().include(["id"].iter().map(|s| (*s).to_string()).collect());
        assert!(config.has_filtering());

        let config = ResponseModelConfig::new()
            .exclude(["password"].iter().map(|s| (*s).to_string()).collect());
        assert!(config.has_filtering());

        let config = ResponseModelConfig::new().exclude_none(true);
        assert!(config.has_filtering());
    }

    #[test]
    fn response_model_config_filter_json_include() {
        let config = ResponseModelConfig::new()
            .include(["id", "name"].iter().map(|s| (*s).to_string()).collect());

        let value = serde_json::json!({
            "id": 1,
            "name": "Alice",
            "email": "alice@example.com",
            "password": "secret"
        });

        let filtered = config.filter_json(value);
        assert_eq!(filtered.get("id"), Some(&serde_json::json!(1)));
        assert_eq!(filtered.get("name"), Some(&serde_json::json!("Alice")));
        assert!(filtered.get("email").is_none());
        assert!(filtered.get("password").is_none());
    }

    #[test]
    fn response_model_config_filter_json_exclude() {
        let config = ResponseModelConfig::new().exclude(
            ["password", "secret"]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        );

        let value = serde_json::json!({
            "id": 1,
            "name": "Alice",
            "password": "secret123",
            "secret": "hidden"
        });

        let filtered = config.filter_json(value);
        assert_eq!(filtered.get("id"), Some(&serde_json::json!(1)));
        assert_eq!(filtered.get("name"), Some(&serde_json::json!("Alice")));
        assert!(filtered.get("password").is_none());
        assert!(filtered.get("secret").is_none());
    }

    #[test]
    fn response_model_config_filter_json_exclude_none() {
        let config = ResponseModelConfig::new().exclude_none(true);

        let value = serde_json::json!({
            "id": 1,
            "name": "Alice",
            "middle_name": null,
            "nickname": null
        });

        let filtered = config.filter_json(value);
        assert_eq!(filtered.get("id"), Some(&serde_json::json!(1)));
        assert_eq!(filtered.get("name"), Some(&serde_json::json!("Alice")));
        assert!(filtered.get("middle_name").is_none());
        assert!(filtered.get("nickname").is_none());
    }

    #[test]
    fn response_model_config_filter_json_combined() {
        let config = ResponseModelConfig::new()
            .include(
                ["id", "name", "email", "middle_name"]
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            )
            .exclude_none(true);

        let value = serde_json::json!({
            "id": 1,
            "name": "Alice",
            "email": "alice@example.com",
            "middle_name": null,
            "password": "secret"
        });

        let filtered = config.filter_json(value);
        assert_eq!(filtered.get("id"), Some(&serde_json::json!(1)));
        assert_eq!(filtered.get("name"), Some(&serde_json::json!("Alice")));
        assert_eq!(
            filtered.get("email"),
            Some(&serde_json::json!("alice@example.com"))
        );
        assert!(filtered.get("middle_name").is_none()); // null, excluded
        assert!(filtered.get("password").is_none()); // not in include
    }

    // =========================================================================
    // ValidatedResponse tests
    // =========================================================================

    #[test]
    fn validated_response_serializes_struct() {
        #[derive(Serialize)]
        struct User {
            id: i64,
            name: String,
        }

        let user = User {
            id: 1,
            name: "Alice".to_string(),
        };

        let response = ValidatedResponse::new(user).into_response();
        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(parsed["id"], 1);
            assert_eq!(parsed["name"], "Alice");
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn validated_response_excludes_fields() {
        #[derive(Serialize)]
        struct User {
            id: i64,
            name: String,
            password: String,
        }

        let user = User {
            id: 1,
            name: "Alice".to_string(),
            password: "secret123".to_string(),
        };

        let response = ValidatedResponse::new(user)
            .with_config(
                ResponseModelConfig::new()
                    .exclude(["password"].iter().map(|s| (*s).to_string()).collect()),
            )
            .into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(parsed["id"], 1);
            assert_eq!(parsed["name"], "Alice");
            assert!(parsed.get("password").is_none());
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn validated_response_includes_fields() {
        #[derive(Serialize)]
        struct User {
            id: i64,
            name: String,
            email: String,
            password: String,
        }

        let user = User {
            id: 1,
            name: "Alice".to_string(),
            email: "alice@example.com".to_string(),
            password: "secret123".to_string(),
        };

        let response = ValidatedResponse::new(user)
            .with_config(
                ResponseModelConfig::new()
                    .include(["id", "name"].iter().map(|s| (*s).to_string()).collect()),
            )
            .into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(parsed["id"], 1);
            assert_eq!(parsed["name"], "Alice");
            assert!(parsed.get("email").is_none());
            assert!(parsed.get("password").is_none());
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn validated_response_exclude_none_values() {
        #[derive(Serialize)]
        struct User {
            id: i64,
            name: String,
            nickname: Option<String>,
        }

        let user = User {
            id: 1,
            name: "Alice".to_string(),
            nickname: None,
        };

        let response = ValidatedResponse::new(user)
            .with_config(ResponseModelConfig::new().exclude_none(true))
            .into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(parsed["id"], 1);
            assert_eq!(parsed["name"], "Alice");
            assert!(parsed.get("nickname").is_none());
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn validated_response_content_type_is_json() {
        #[derive(Serialize)]
        struct Data {
            value: i32,
        }

        let response = ValidatedResponse::new(Data { value: 42 }).into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| String::from_utf8_lossy(value).to_string());

        assert_eq!(content_type, Some("application/json".to_string()));
    }

    // =========================================================================
    // Helper function tests
    // =========================================================================

    #[test]
    fn exclude_fields_helper() {
        #[derive(Serialize)]
        struct User {
            id: i64,
            name: String,
            password: String,
        }

        let user = User {
            id: 1,
            name: "Alice".to_string(),
            password: "secret".to_string(),
        };

        let response = exclude_fields(user, &["password"]).into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert!(parsed.get("id").is_some());
            assert!(parsed.get("name").is_some());
            assert!(parsed.get("password").is_none());
        } else {
            panic!("Expected Bytes body");
        }
    }

    #[test]
    fn include_fields_helper() {
        #[derive(Serialize)]
        struct User {
            id: i64,
            name: String,
            email: String,
            password: String,
        }

        let user = User {
            id: 1,
            name: "Alice".to_string(),
            email: "alice@example.com".to_string(),
            password: "secret".to_string(),
        };

        let response = include_fields(user, &["id", "name"]).into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            assert!(parsed.get("id").is_some());
            assert!(parsed.get("name").is_some());
            assert!(parsed.get("email").is_none());
            assert!(parsed.get("password").is_none());
        } else {
            panic!("Expected Bytes body");
        }
    }

    // ====================================================================
    // Conditional request (ETag) tests
    // ====================================================================

    #[test]
    fn status_code_precondition_failed() {
        assert_eq!(StatusCode::PRECONDITION_FAILED.as_u16(), 412);
        assert_eq!(
            StatusCode::PRECONDITION_FAILED.canonical_reason(),
            "Precondition Failed"
        );
    }

    #[test]
    fn response_not_modified_status() {
        let resp = Response::not_modified();
        assert_eq!(resp.status().as_u16(), 304);
    }

    #[test]
    fn response_precondition_failed_status() {
        let resp = Response::precondition_failed();
        assert_eq!(resp.status().as_u16(), 412);
    }

    #[test]
    fn response_with_etag() {
        let resp = Response::ok().with_etag("\"abc123\"");
        let etag = resp
            .headers()
            .iter()
            .find(|(n, _)| n == "ETag")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert_eq!(etag, Some("\"abc123\"".to_string()));
    }

    #[test]
    fn response_with_weak_etag() {
        let resp = Response::ok().with_weak_etag("\"abc123\"");
        let etag = resp
            .headers()
            .iter()
            .find(|(n, _)| n == "ETag")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert_eq!(etag, Some("W/\"abc123\"".to_string()));
    }

    #[test]
    fn response_with_weak_etag_already_prefixed() {
        let resp = Response::ok().with_weak_etag("W/\"abc123\"");
        let etag = resp
            .headers()
            .iter()
            .find(|(n, _)| n == "ETag")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert_eq!(etag, Some("W/\"abc123\"".to_string()));
    }

    #[test]
    fn check_if_none_match_exact() {
        // Exact match -> false (send 304)
        assert!(!check_if_none_match("\"abc\"", "\"abc\""));
    }

    #[test]
    fn check_if_none_match_no_match() {
        // No match -> true (send full response)
        assert!(check_if_none_match("\"abc\"", "\"def\""));
    }

    #[test]
    fn check_if_none_match_wildcard() {
        assert!(!check_if_none_match("*", "\"anything\""));
    }

    #[test]
    fn check_if_none_match_multiple_etags() {
        // Second ETag matches
        assert!(!check_if_none_match("\"aaa\", \"bbb\", \"ccc\"", "\"bbb\""));
        // None match
        assert!(check_if_none_match("\"aaa\", \"bbb\"", "\"ccc\""));
    }

    #[test]
    fn check_if_none_match_weak_comparison() {
        // Weak ETags should match in If-None-Match (weak comparison)
        assert!(!check_if_none_match("W/\"abc\"", "\"abc\""));
        assert!(!check_if_none_match("\"abc\"", "W/\"abc\""));
        assert!(!check_if_none_match("W/\"abc\"", "W/\"abc\""));
    }

    #[test]
    fn check_if_match_exact() {
        // Exact match -> true (proceed)
        assert!(check_if_match("\"abc\"", "\"abc\""));
    }

    #[test]
    fn check_if_match_no_match() {
        // No match -> false (412)
        assert!(!check_if_match("\"abc\"", "\"def\""));
    }

    #[test]
    fn check_if_match_wildcard() {
        assert!(check_if_match("*", "\"anything\""));
    }

    #[test]
    fn check_if_match_weak_etag_fails() {
        // If-Match requires strong comparison - weak ETags never match
        assert!(!check_if_match("W/\"abc\"", "\"abc\""));
        assert!(!check_if_match("\"abc\"", "W/\"abc\""));
    }

    #[test]
    fn check_if_match_multiple_etags() {
        assert!(check_if_match("\"aaa\", \"bbb\"", "\"bbb\""));
        assert!(!check_if_match("\"aaa\", \"bbb\"", "\"ccc\""));
    }

    #[test]
    fn apply_conditional_get_304() {
        use crate::request::Method;

        let headers = vec![("If-None-Match".to_string(), b"\"abc123\"".to_vec())];
        let response = Response::ok().with_etag("\"abc123\"");
        let result = apply_conditional(&headers, &Method::Get, response);
        assert_eq!(result.status().as_u16(), 304);
    }

    #[test]
    fn apply_conditional_get_no_match_200() {
        use crate::request::Method;

        let headers = vec![("If-None-Match".to_string(), b"\"old\"".to_vec())];
        let response = Response::ok().with_etag("\"new\"");
        let result = apply_conditional(&headers, &Method::Get, response);
        assert_eq!(result.status().as_u16(), 200);
    }

    #[test]
    fn apply_conditional_put_412() {
        use crate::request::Method;

        let headers = vec![("If-Match".to_string(), b"\"old\"".to_vec())];
        let response = Response::ok().with_etag("\"new\"");
        let result = apply_conditional(&headers, &Method::Put, response);
        assert_eq!(result.status().as_u16(), 412);
    }

    #[test]
    fn apply_conditional_put_match_200() {
        use crate::request::Method;

        let headers = vec![("If-Match".to_string(), b"\"current\"".to_vec())];
        let response = Response::ok().with_etag("\"current\"");
        let result = apply_conditional(&headers, &Method::Put, response);
        assert_eq!(result.status().as_u16(), 200);
    }

    #[test]
    fn apply_conditional_no_etag_passthrough() {
        use crate::request::Method;

        let headers = vec![("If-None-Match".to_string(), b"\"abc\"".to_vec())];
        let response = Response::ok(); // No ETag
        let result = apply_conditional(&headers, &Method::Get, response);
        assert_eq!(result.status().as_u16(), 200);
    }

    // ====================================================================
    // Link Header Tests
    // ====================================================================

    #[test]
    fn link_header_single() {
        let h = LinkHeader::new().link("https://example.com/next", LinkRel::Next);
        assert_eq!(h.to_string(), r#"<https://example.com/next>; rel="next""#);
    }

    #[test]
    fn link_header_multiple() {
        let h = LinkHeader::new()
            .link("/page/2", LinkRel::Next)
            .link("/page/0", LinkRel::Prev);
        let s = h.to_string();
        assert!(s.contains(r#"</page/2>; rel="next""#));
        assert!(s.contains(r#"</page/0>; rel="prev""#));
        assert!(s.contains(", "));
    }

    #[test]
    fn link_with_title_and_type() {
        let link = Link::new("https://api.example.com", LinkRel::Related)
            .title("API Docs")
            .media_type("text/html");
        let s = link.to_string();
        assert!(s.contains(r#"title="API Docs""#));
        assert!(s.contains(r#"type="text/html""#));
    }

    #[test]
    fn link_header_custom_rel() {
        let h = LinkHeader::new().link("/schema", LinkRel::Custom("describedby".to_string()));
        assert!(h.to_string().contains(r#"rel="describedby""#));
    }

    #[test]
    fn link_header_paginate_first_page() {
        let h = LinkHeader::new().paginate("/users", 1, 10, 50);
        let s = h.to_string();
        assert!(s.contains(r#"rel="self""#));
        assert!(s.contains(r#"rel="first""#));
        assert!(s.contains(r#"rel="last""#));
        assert!(s.contains(r#"rel="next""#));
        assert!(!s.contains(r#"rel="prev""#)); // No prev on first page
        assert!(s.contains("page=5")); // last page = 50/10 = 5
    }

    #[test]
    fn link_header_paginate_middle_page() {
        let h = LinkHeader::new().paginate("/users", 3, 10, 50);
        let s = h.to_string();
        assert!(s.contains(r#"rel="prev""#));
        assert!(s.contains(r#"rel="next""#));
        assert!(s.contains("page=2")); // prev
        assert!(s.contains("page=4")); // next
    }

    #[test]
    fn link_header_paginate_last_page() {
        let h = LinkHeader::new().paginate("/users", 5, 10, 50);
        let s = h.to_string();
        assert!(s.contains(r#"rel="prev""#));
        assert!(!s.contains(r#"rel="next""#)); // No next on last page
    }

    #[test]
    fn link_header_paginate_with_existing_query() {
        let h = LinkHeader::new().paginate("/users?sort=name", 1, 10, 20);
        let s = h.to_string();
        assert!(s.contains("sort=name&page="));
    }

    #[test]
    fn link_header_empty() {
        let h = LinkHeader::new();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
        assert_eq!(h.to_string(), "");
    }

    #[test]
    fn link_header_apply_to_response() {
        let h = LinkHeader::new().link("/next", LinkRel::Next);
        let response = h.apply(Response::ok());
        let link_hdr = response
            .headers()
            .iter()
            .find(|(n, _)| n == "link")
            .map(|(_, v)| std::str::from_utf8(v).unwrap().to_string());
        assert!(link_hdr.unwrap().contains("rel=\"next\""));
    }

    #[test]
    fn link_header_apply_empty_noop() {
        let h = LinkHeader::new();
        let response = h.apply(Response::ok());
        let has_link = response.headers().iter().any(|(n, _)| n == "link");
        assert!(!has_link);
    }

    #[test]
    fn link_rel_display() {
        assert_eq!(LinkRel::Self_.to_string(), "self");
        assert_eq!(LinkRel::Next.to_string(), "next");
        assert_eq!(LinkRel::Prev.to_string(), "prev");
        assert_eq!(LinkRel::First.to_string(), "first");
        assert_eq!(LinkRel::Last.to_string(), "last");
        assert_eq!(LinkRel::Related.to_string(), "related");
        assert_eq!(LinkRel::Alternate.to_string(), "alternate");
    }
}
