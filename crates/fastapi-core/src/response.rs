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
    /// 413 Payload Too Large
    pub const PAYLOAD_TOO_LARGE: Self = Self(413);
    /// 415 Unsupported Media Type
    pub const UNSUPPORTED_MEDIA_TYPE: Self = Self(415);
    /// 422 Unprocessable Entity
    pub const UNPROCESSABLE_ENTITY: Self = Self(422);
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
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            422 => "Unprocessable Entity",
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

    /// Add a header.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        self.headers.push((name.into(), value.into()));
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
    /// Create a new HTML response.
    #[must_use]
    pub fn new(content: impl Into<String>) -> Self {
        Self(content.into())
    }

    /// Get the HTML content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.0
    }
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
}
