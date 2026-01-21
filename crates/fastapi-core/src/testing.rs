//! Test utilities for fastapi applications.
//!
//! This module provides a [`TestClient`] for in-process testing of handlers
//! without network overhead. It integrates with asupersync's capability model
//! and supports deterministic testing via the Lab runtime.
//!
//! # Features
//!
//! - **In-process testing**: No network I/O, fast execution
//! - **HTTP-like API**: Familiar `client.get("/path")` interface
//! - **Request builder**: Fluent API for headers, body, cookies
//! - **Response assertions**: Convenient assertion helpers
//! - **Cookie jar**: Automatic session management across requests
//! - **Lab integration**: Deterministic testing with asupersync
//!
//! # Example
//!
//! ```ignore
//! use fastapi_core::testing::TestClient;
//! use fastapi_core::middleware::Handler;
//!
//! async fn hello_handler(ctx: &RequestContext, req: &mut Request) -> Response {
//!     Response::ok().body(ResponseBody::Bytes(b"Hello, World!".to_vec()))
//! }
//!
//! #[test]
//! fn test_hello() {
//!     let client = TestClient::new(hello_handler);
//!     let response = client.get("/hello").send();
//!
//!     assert_eq!(response.status().as_u16(), 200);
//!     assert_eq!(response.text(), "Hello, World!");
//! }
//! ```
//!
//! # Deterministic Testing
//!
//! For reproducible tests involving concurrency, use [`TestClient::with_seed`]:
//!
//! ```ignore
//! let client = TestClient::with_seed(handler, 42);
//! // Same seed = same execution order for concurrent operations
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use asupersync::Cx;

use crate::context::RequestContext;
use crate::dependency::{DependencyOverrides, FromDependency};
use crate::middleware::Handler;
use crate::request::{Body, Method, Request};
use crate::response::{Response, ResponseBody, StatusCode};

/// A simple cookie jar for maintaining cookies across requests.
///
/// Cookies are stored as name-value pairs and automatically
/// added to subsequent requests.
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    cookies: HashMap<String, String>,
}

impl CookieJar {
    /// Creates an empty cookie jar.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a cookie in the jar.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.cookies.insert(name.into(), value.into());
    }

    /// Gets a cookie value by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.cookies.get(name).map(String::as_str)
    }

    /// Removes a cookie from the jar.
    pub fn remove(&mut self, name: &str) -> Option<String> {
        self.cookies.remove(name)
    }

    /// Clears all cookies from the jar.
    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    /// Returns the number of cookies in the jar.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    /// Returns `true` if the jar is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// Formats cookies for the Cookie header.
    #[must_use]
    pub fn to_cookie_header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            None
        } else {
            Some(
                self.cookies
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    }

    /// Parses a Set-Cookie header and adds the cookie to the jar.
    pub fn parse_set_cookie(&mut self, header_value: &[u8]) {
        if let Ok(value) = std::str::from_utf8(header_value) {
            // Parse simple name=value; ignore attributes for now
            if let Some(cookie_part) = value.split(';').next() {
                if let Some((name, val)) = cookie_part.split_once('=') {
                    self.set(name.trim(), val.trim());
                }
            }
        }
    }
}

/// Test client for in-process HTTP testing.
///
/// `TestClient` wraps a handler and provides an HTTP-like interface
/// for testing without network overhead. It maintains a cookie jar
/// for session persistence across requests.
///
/// # Thread Safety
///
/// `TestClient` is thread-safe and can be shared across test threads.
/// The internal cookie jar is protected by a mutex.
///
/// # Example
///
/// ```ignore
/// let client = TestClient::new(my_handler);
///
/// // Simple GET request
/// let response = client.get("/users").send();
/// assert_eq!(response.status(), StatusCode::OK);
///
/// // POST with JSON body
/// let response = client
///     .post("/users")
///     .json(&CreateUser { name: "Alice" })
///     .send();
/// assert_eq!(response.status(), StatusCode::CREATED);
///
/// // Request with headers
/// let response = client
///     .get("/protected")
///     .header("Authorization", "Bearer token123")
///     .send();
/// ```
pub struct TestClient<H> {
    handler: Arc<H>,
    cookies: Arc<Mutex<CookieJar>>,
    dependency_overrides: Arc<DependencyOverrides>,
    seed: Option<u64>,
    request_id_counter: Arc<std::sync::atomic::AtomicU64>,
}

impl<H: Handler + 'static> TestClient<H> {
    /// Creates a new test client wrapping the given handler.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let client = TestClient::new(my_handler);
    /// ```
    pub fn new(handler: H) -> Self {
        let dependency_overrides = handler
            .dependency_overrides()
            .unwrap_or_else(|| Arc::new(DependencyOverrides::new()));
        Self {
            handler: Arc::new(handler),
            cookies: Arc::new(Mutex::new(CookieJar::new())),
            dependency_overrides,
            seed: None,
            request_id_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    /// Creates a test client with a deterministic seed for the Lab runtime.
    ///
    /// Using the same seed produces identical execution order for
    /// concurrent operations, enabling reproducible test failures.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let client = TestClient::with_seed(my_handler, 42);
    /// ```
    pub fn with_seed(handler: H, seed: u64) -> Self {
        let dependency_overrides = handler
            .dependency_overrides()
            .unwrap_or_else(|| Arc::new(DependencyOverrides::new()));
        Self {
            handler: Arc::new(handler),
            cookies: Arc::new(Mutex::new(CookieJar::new())),
            dependency_overrides,
            seed: Some(seed),
            request_id_counter: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    /// Returns the seed used for deterministic testing, if set.
    #[must_use]
    pub fn seed(&self) -> Option<u64> {
        self.seed
    }

    /// Returns a reference to the cookie jar.
    ///
    /// Note: The jar is protected by a mutex, so concurrent access
    /// is safe but may block.
    pub fn cookies(&self) -> std::sync::MutexGuard<'_, CookieJar> {
        self.cookies.lock().expect("cookie jar mutex poisoned")
    }

    /// Clears all cookies from the jar.
    pub fn clear_cookies(&self) {
        self.cookies().clear();
    }

    /// Creates a GET request builder.
    #[must_use]
    pub fn get(&self, path: &str) -> RequestBuilder<'_, H> {
        RequestBuilder::new(self, Method::Get, path)
    }

    /// Creates a POST request builder.
    #[must_use]
    pub fn post(&self, path: &str) -> RequestBuilder<'_, H> {
        RequestBuilder::new(self, Method::Post, path)
    }

    /// Creates a PUT request builder.
    #[must_use]
    pub fn put(&self, path: &str) -> RequestBuilder<'_, H> {
        RequestBuilder::new(self, Method::Put, path)
    }

    /// Creates a DELETE request builder.
    #[must_use]
    pub fn delete(&self, path: &str) -> RequestBuilder<'_, H> {
        RequestBuilder::new(self, Method::Delete, path)
    }

    /// Creates a PATCH request builder.
    #[must_use]
    pub fn patch(&self, path: &str) -> RequestBuilder<'_, H> {
        RequestBuilder::new(self, Method::Patch, path)
    }

    /// Creates an OPTIONS request builder.
    #[must_use]
    pub fn options(&self, path: &str) -> RequestBuilder<'_, H> {
        RequestBuilder::new(self, Method::Options, path)
    }

    /// Creates a HEAD request builder.
    #[must_use]
    pub fn head(&self, path: &str) -> RequestBuilder<'_, H> {
        RequestBuilder::new(self, Method::Head, path)
    }

    /// Creates a request builder with a custom method.
    #[must_use]
    pub fn request(&self, method: Method, path: &str) -> RequestBuilder<'_, H> {
        RequestBuilder::new(self, method, path)
    }

    /// Register a dependency override for this test client.
    pub fn override_dependency<T, F, Fut>(&self, f: F)
    where
        T: FromDependency,
        F: Fn(&RequestContext, &mut Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, T::Error>> + Send + 'static,
    {
        self.dependency_overrides.insert::<T, F, Fut>(f);
    }

    /// Register a fixed dependency override value.
    pub fn override_dependency_value<T>(&self, value: T)
    where
        T: FromDependency,
    {
        self.dependency_overrides.insert_value(value);
    }

    /// Clear all registered dependency overrides.
    pub fn clear_dependency_overrides(&self) {
        self.dependency_overrides.clear();
    }

    /// Generates a unique request ID for tracing.
    fn next_request_id(&self) -> u64 {
        self.request_id_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Executes a request and returns the response.
    ///
    /// This is called internally by `RequestBuilder::send()`.
    fn execute(&self, mut request: Request) -> TestResponse {
        // Add cookies from jar to request
        {
            let jar = self.cookies();
            if let Some(cookie_header) = jar.to_cookie_header() {
                request
                    .headers_mut()
                    .insert("cookie", cookie_header.into_bytes());
            }
        }

        // Create test context with Cx::for_testing()
        let cx = Cx::for_testing();
        let request_id = self.next_request_id();
        let ctx =
            RequestContext::with_overrides(cx, request_id, Arc::clone(&self.dependency_overrides));

        // Execute handler synchronously for testing
        // In a real async context, this would be awaited
        let response = futures_executor::block_on(self.handler.call(&ctx, &mut request));

        // Extract cookies from response
        {
            let mut jar = self.cookies();
            for (name, value) in response.headers() {
                if name.eq_ignore_ascii_case("set-cookie") {
                    jar.parse_set_cookie(value);
                }
            }
        }

        TestResponse::new(response, request_id)
    }
}

impl<H> Clone for TestClient<H> {
    fn clone(&self) -> Self {
        Self {
            handler: Arc::clone(&self.handler),
            cookies: Arc::clone(&self.cookies),
            dependency_overrides: Arc::clone(&self.dependency_overrides),
            seed: self.seed,
            request_id_counter: Arc::clone(&self.request_id_counter),
        }
    }
}

/// Builder for constructing test requests with a fluent API.
///
/// Use the methods on [`TestClient`] to create a request builder,
/// then chain configuration methods and call [`send`](Self::send) to execute.
///
/// # Example
///
/// ```ignore
/// let response = client
///     .post("/api/items")
///     .header("Content-Type", "application/json")
///     .body(r#"{"name": "Widget"}"#)
///     .send();
/// ```
pub struct RequestBuilder<'a, H> {
    client: &'a TestClient<H>,
    method: Method,
    path: String,
    query: Option<String>,
    headers: Vec<(String, Vec<u8>)>,
    body: Body,
}

impl<'a, H: Handler + 'static> RequestBuilder<'a, H> {
    /// Creates a new request builder.
    fn new(client: &'a TestClient<H>, method: Method, path: &str) -> Self {
        // Split path and query string
        let (path, query) = if let Some(idx) = path.find('?') {
            (path[..idx].to_string(), Some(path[idx + 1..].to_string()))
        } else {
            (path.to_string(), None)
        };

        Self {
            client,
            method,
            path,
            query,
            headers: Vec::new(),
            body: Body::Empty,
        }
    }

    /// Sets a query string parameter.
    ///
    /// Multiple calls append parameters.
    ///
    /// # Example
    ///
    /// ```ignore
    /// client.get("/search").query("q", "rust").query("limit", "10").send()
    /// ```
    #[must_use]
    pub fn query(mut self, key: &str, value: &str) -> Self {
        let param = format!("{key}={value}");
        self.query = Some(match self.query {
            Some(q) => format!("{q}&{param}"),
            None => param,
        });
        self
    }

    /// Sets a request header.
    ///
    /// # Example
    ///
    /// ```ignore
    /// client.get("/api").header("Authorization", "Bearer token").send()
    /// ```
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Sets a request header with a string value.
    #[must_use]
    pub fn header_str(self, name: impl Into<String>, value: &str) -> Self {
        self.header(name, value.as_bytes().to_vec())
    }

    /// Sets the request body as raw bytes.
    ///
    /// # Example
    ///
    /// ```ignore
    /// client.post("/upload").body(b"binary data".to_vec()).send()
    /// ```
    #[must_use]
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Body::Bytes(body.into());
        self
    }

    /// Sets the request body as a string.
    #[must_use]
    pub fn body_str(self, body: &str) -> Self {
        self.body(body.as_bytes().to_vec())
    }

    /// Sets the request body as JSON.
    ///
    /// Automatically sets the Content-Type header to `application/json`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// #[derive(Serialize)]
    /// struct CreateUser { name: String }
    ///
    /// client.post("/users").json(&CreateUser { name: "Alice".into() }).send()
    /// ```
    #[must_use]
    pub fn json<T: serde::Serialize>(mut self, value: &T) -> Self {
        let bytes = serde_json::to_vec(value).expect("JSON serialization failed");
        self.body = Body::Bytes(bytes);
        self.headers
            .push(("content-type".to_string(), b"application/json".to_vec()));
        self
    }

    /// Sets a cookie for this request only.
    ///
    /// This does not affect the client's cookie jar.
    #[must_use]
    pub fn cookie(self, name: &str, value: &str) -> Self {
        let cookie = format!("{name}={value}");
        self.header("cookie", cookie.into_bytes())
    }

    /// Sends the request and returns the response.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let response = client.get("/").send();
    /// ```
    #[must_use]
    pub fn send(self) -> TestResponse {
        let mut request = Request::new(self.method, self.path);
        request.set_query(self.query);
        request.set_body(self.body);

        for (name, value) in self.headers {
            request.headers_mut().insert(name, value);
        }

        self.client.execute(request)
    }
}

/// Response from a test request with assertion helpers.
///
/// `TestResponse` wraps a [`Response`] and provides convenient methods
/// for accessing response data and making assertions in tests.
///
/// # Example
///
/// ```ignore
/// let response = client.get("/api/user").send();
///
/// assert_eq!(response.status(), StatusCode::OK);
/// assert!(response.header("content-type").contains("application/json"));
///
/// let user: User = response.json().unwrap();
/// assert_eq!(user.name, "Alice");
/// ```
#[derive(Debug)]
pub struct TestResponse {
    inner: Response,
    request_id: u64,
}

impl TestResponse {
    /// Creates a new test response.
    fn new(response: Response, request_id: u64) -> Self {
        Self {
            inner: response,
            request_id,
        }
    }

    /// Returns the request ID for tracing.
    #[must_use]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    /// Returns the HTTP status code.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.inner.status()
    }

    /// Returns the status code as a u16.
    #[must_use]
    pub fn status_code(&self) -> u16 {
        self.inner.status().as_u16()
    }

    /// Checks if the status is successful (2xx).
    #[must_use]
    pub fn is_success(&self) -> bool {
        let code = self.status_code();
        (200..300).contains(&code)
    }

    /// Checks if the status is a redirect (3xx).
    #[must_use]
    pub fn is_redirect(&self) -> bool {
        let code = self.status_code();
        (300..400).contains(&code)
    }

    /// Checks if the status is a client error (4xx).
    #[must_use]
    pub fn is_client_error(&self) -> bool {
        let code = self.status_code();
        (400..500).contains(&code)
    }

    /// Checks if the status is a server error (5xx).
    #[must_use]
    pub fn is_server_error(&self) -> bool {
        let code = self.status_code();
        (500..600).contains(&code)
    }

    /// Returns all headers.
    #[must_use]
    pub fn headers(&self) -> &[(String, Vec<u8>)] {
        self.inner.headers()
    }

    /// Returns a header value by name (case-insensitive).
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&[u8]> {
        let name_lower = name.to_ascii_lowercase();
        self.inner
            .headers()
            .iter()
            .find(|(n, _)| n.to_ascii_lowercase() == name_lower)
            .map(|(_, v)| v.as_slice())
    }

    /// Returns a header value as a string (case-insensitive).
    #[must_use]
    pub fn header_str(&self, name: &str) -> Option<&str> {
        self.header(name).and_then(|v| std::str::from_utf8(v).ok())
    }

    /// Returns the Content-Type header value.
    #[must_use]
    pub fn content_type(&self) -> Option<&str> {
        self.header_str("content-type")
    }

    /// Returns the body as raw bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        match self.inner.body_ref() {
            ResponseBody::Empty => &[],
            ResponseBody::Bytes(b) => b,
            ResponseBody::Stream(_) => {
                panic!("streaming response body not supported in TestResponse")
            }
        }
    }

    /// Returns the body as a UTF-8 string.
    ///
    /// # Panics
    ///
    /// Panics if the body is not valid UTF-8.
    #[must_use]
    pub fn text(&self) -> &str {
        std::str::from_utf8(self.bytes()).expect("response body is not valid UTF-8")
    }

    /// Tries to return the body as a UTF-8 string.
    #[must_use]
    pub fn text_opt(&self) -> Option<&str> {
        std::str::from_utf8(self.bytes()).ok()
    }

    /// Parses the body as JSON.
    ///
    /// # Errors
    ///
    /// Returns an error if the body cannot be parsed as the target type.
    ///
    /// # Example
    ///
    /// ```ignore
    /// #[derive(Deserialize)]
    /// struct User { name: String }
    ///
    /// let user: User = response.json().unwrap();
    /// ```
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(self.bytes())
    }

    /// Returns the body length.
    #[must_use]
    pub fn content_length(&self) -> usize {
        self.bytes().len()
    }

    /// Returns the underlying response.
    #[must_use]
    pub fn into_inner(self) -> Response {
        self.inner
    }

    // =========================================================================
    // Assertion Helpers
    // =========================================================================

    /// Asserts that the status code equals the expected value.
    ///
    /// # Panics
    ///
    /// Panics with a descriptive message if the assertion fails.
    #[must_use]
    pub fn assert_status(&self, expected: StatusCode) -> &Self {
        assert_eq!(
            self.status(),
            expected,
            "Expected status {}, got {} for request {}",
            expected.as_u16(),
            self.status_code(),
            self.request_id
        );
        self
    }

    /// Asserts that the status code equals the expected u16 value.
    ///
    /// # Panics
    ///
    /// Panics with a descriptive message if the assertion fails.
    #[must_use]
    pub fn assert_status_code(&self, expected: u16) -> &Self {
        assert_eq!(
            self.status_code(),
            expected,
            "Expected status {expected}, got {} for request {}",
            self.status_code(),
            self.request_id
        );
        self
    }

    /// Asserts that the response is successful (2xx).
    ///
    /// # Panics
    ///
    /// Panics if the status is not in the 2xx range.
    #[must_use]
    pub fn assert_success(&self) -> &Self {
        assert!(
            self.is_success(),
            "Expected success status, got {} for request {}",
            self.status_code(),
            self.request_id
        );
        self
    }

    /// Asserts that a header exists with the given value.
    ///
    /// # Panics
    ///
    /// Panics if the header doesn't exist or doesn't match.
    #[must_use]
    pub fn assert_header(&self, name: &str, expected: &str) -> &Self {
        let actual = self.header_str(name);
        assert_eq!(
            actual,
            Some(expected),
            "Expected header '{name}' to be '{expected}', got {:?} for request {}",
            actual,
            self.request_id
        );
        self
    }

    /// Asserts that the body equals the expected string.
    ///
    /// # Panics
    ///
    /// Panics if the body doesn't match.
    #[must_use]
    pub fn assert_text(&self, expected: &str) -> &Self {
        assert_eq!(
            self.text(),
            expected,
            "Body mismatch for request {}",
            self.request_id
        );
        self
    }

    /// Asserts that the body contains the expected substring.
    ///
    /// # Panics
    ///
    /// Panics if the body doesn't contain the substring.
    #[must_use]
    pub fn assert_text_contains(&self, expected: &str) -> &Self {
        assert!(
            self.text().contains(expected),
            "Expected body to contain '{}', got '{}' for request {}",
            expected,
            self.text(),
            self.request_id
        );
        self
    }

    /// Asserts that the JSON body equals the expected value.
    ///
    /// # Panics
    ///
    /// Panics if parsing fails or the value doesn't match.
    #[must_use]
    pub fn assert_json<T>(&self, expected: &T) -> &Self
    where
        T: serde::de::DeserializeOwned + serde::Serialize + PartialEq + std::fmt::Debug,
    {
        let actual: T = self.json().expect("Failed to parse response as JSON");
        assert_eq!(
            actual, *expected,
            "JSON body mismatch for request {}",
            self.request_id
        );
        self
    }

    /// Asserts that the JSON body contains all fields from the expected value.
    ///
    /// This performs partial matching: the actual response may contain additional
    /// fields not present in `expected`, but all fields in `expected` must be
    /// present in the actual response with matching values.
    ///
    /// # Panics
    ///
    /// Panics if parsing fails or partial matching fails.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Response body: {"id": 1, "name": "Alice", "email": "alice@example.com"}
    /// // This passes because all expected fields match:
    /// response.assert_json_contains(&json!({"name": "Alice"}));
    /// ```
    #[must_use]
    pub fn assert_json_contains(&self, expected: &serde_json::Value) -> &Self {
        let actual: serde_json::Value = self.json().expect("Failed to parse response as JSON");
        if let Err(path) = json_contains(&actual, expected) {
            panic!(
                "JSON partial match failed at path '{}' for request {}\n\
                 Expected (partial):\n{}\n\
                 Actual:\n{}",
                path,
                self.request_id,
                serde_json::to_string_pretty(expected).unwrap_or_else(|_| format!("{expected:?}")),
                serde_json::to_string_pretty(&actual).unwrap_or_else(|_| format!("{actual:?}")),
            );
        }
        self
    }

    /// Asserts that the body matches the given regex pattern.
    ///
    /// # Panics
    ///
    /// Panics if the pattern doesn't match or is invalid.
    ///
    /// # Example
    ///
    /// ```ignore
    /// response.assert_body_matches(r"user_\d+");
    /// ```
    #[cfg(feature = "regex")]
    #[must_use]
    pub fn assert_body_matches(&self, pattern: &str) -> &Self {
        let re = regex::Regex::new(pattern)
            .unwrap_or_else(|e| panic!("Invalid regex pattern '{pattern}': {e}"));
        let body = self.text();
        assert!(
            re.is_match(body),
            "Expected body to match pattern '{}', got '{}' for request {}",
            pattern,
            body,
            self.request_id
        );
        self
    }

    /// Asserts that a header exists and matches the given regex pattern.
    ///
    /// # Panics
    ///
    /// Panics if the header doesn't exist, can't be read as UTF-8,
    /// or doesn't match the pattern.
    #[cfg(feature = "regex")]
    #[must_use]
    pub fn assert_header_matches(&self, name: &str, pattern: &str) -> &Self {
        let re = regex::Regex::new(pattern)
            .unwrap_or_else(|e| panic!("Invalid regex pattern '{pattern}': {e}"));
        let value = self
            .header_str(name)
            .unwrap_or_else(|| panic!("Header '{name}' not found for request {}", self.request_id));
        assert!(
            re.is_match(value),
            "Expected header '{}' to match pattern '{}', got '{}' for request {}",
            name,
            pattern,
            value,
            self.request_id
        );
        self
    }

    /// Asserts that a header exists (regardless of value).
    ///
    /// # Panics
    ///
    /// Panics if the header doesn't exist.
    #[must_use]
    pub fn assert_header_exists(&self, name: &str) -> &Self {
        assert!(
            self.header(name).is_some(),
            "Expected header '{}' to exist for request {}",
            name,
            self.request_id
        );
        self
    }

    /// Asserts that a header does not exist.
    ///
    /// # Panics
    ///
    /// Panics if the header exists.
    #[must_use]
    pub fn assert_header_missing(&self, name: &str) -> &Self {
        assert!(
            self.header(name).is_none(),
            "Expected header '{}' to not exist for request {}, but found {:?}",
            name,
            self.request_id,
            self.header_str(name)
        );
        self
    }

    /// Asserts that the Content-Type header contains the expected value.
    ///
    /// This is a convenience method that checks if the Content-Type header
    /// contains the given string (useful for checking media types ignoring charset).
    ///
    /// # Panics
    ///
    /// Panics if the Content-Type header doesn't exist or doesn't contain the expected value.
    #[must_use]
    pub fn assert_content_type_contains(&self, expected: &str) -> &Self {
        let ct = self.content_type().unwrap_or_else(|| {
            panic!(
                "Content-Type header not found for request {}",
                self.request_id
            )
        });
        assert!(
            ct.contains(expected),
            "Expected Content-Type to contain '{}', got '{}' for request {}",
            expected,
            ct,
            self.request_id
        );
        self
    }
}

// =============================================================================
// Partial JSON Matching
// =============================================================================

/// Checks if `actual` contains all fields from `expected`.
///
/// Returns `Ok(())` if matching succeeds, or `Err(path)` where `path` is
/// the JSON path to the first mismatch.
///
/// # Matching Rules
///
/// - **Objects**: All keys in `expected` must exist in `actual` with matching values.
///   Extra keys in `actual` are ignored.
/// - **Arrays**: Must match exactly (same length, same elements in order).
/// - **Primitives**: Must be equal.
///
/// # Example
///
/// ```
/// use fastapi_core::testing::json_contains;
/// use serde_json::json;
///
/// // Partial match succeeds - actual has extra "email" field
/// let actual = json!({"id": 1, "name": "Alice", "email": "alice@example.com"});
/// let expected = json!({"name": "Alice"});
/// assert!(json_contains(&actual, &expected).is_ok());
///
/// // Mismatch fails
/// let expected = json!({"name": "Bob"});
/// assert!(json_contains(&actual, &expected).is_err());
/// ```
pub fn json_contains(
    actual: &serde_json::Value,
    expected: &serde_json::Value,
) -> Result<(), String> {
    json_contains_at_path(actual, expected, "$")
}

fn json_contains_at_path(
    actual: &serde_json::Value,
    expected: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    use serde_json::Value;

    match (actual, expected) {
        // For objects, check that all expected keys exist with matching values
        (Value::Object(actual_obj), Value::Object(expected_obj)) => {
            for (key, expected_val) in expected_obj {
                let child_path = format!("{path}.{key}");
                match actual_obj.get(key) {
                    Some(actual_val) => {
                        json_contains_at_path(actual_val, expected_val, &child_path)?;
                    }
                    None => {
                        return Err(child_path);
                    }
                }
            }
            Ok(())
        }
        // For arrays, require exact match (partial array matching is ambiguous)
        (Value::Array(actual_arr), Value::Array(expected_arr)) => {
            if actual_arr.len() != expected_arr.len() {
                return Err(format!("{path}[length]"));
            }
            for (i, (actual_elem, expected_elem)) in
                actual_arr.iter().zip(expected_arr.iter()).enumerate()
            {
                let child_path = format!("{path}[{i}]");
                json_contains_at_path(actual_elem, expected_elem, &child_path)?;
            }
            Ok(())
        }
        // For primitives, require exact match
        _ => {
            if actual == expected {
                Ok(())
            } else {
                Err(path.to_string())
            }
        }
    }
}

// =============================================================================
// Helper Traits for Assertion Macros
// =============================================================================

/// Helper trait to convert various types to u16 for status code comparison.
///
/// This enables the `assert_status!` macro to accept both `u16` literals
/// and `StatusCode` values.
pub trait IntoStatusU16 {
    fn into_status_u16(self) -> u16;
}

impl IntoStatusU16 for u16 {
    fn into_status_u16(self) -> u16 {
        self
    }
}

impl IntoStatusU16 for StatusCode {
    fn into_status_u16(self) -> u16 {
        self.as_u16()
    }
}

// Also implement for i32 since integer literals without suffix default to i32
impl IntoStatusU16 for i32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn into_status_u16(self) -> u16 {
        // This is intentional - HTTP status codes are always 3-digit positive numbers
        self as u16
    }
}

// =============================================================================
// Assertion Macros
// =============================================================================

/// Asserts that a test response has the expected HTTP status code.
///
/// Accepts either a `u16` literal or a `StatusCode` value.
///
/// # Examples
///
/// ```ignore
/// use fastapi_core::assert_status;
///
/// let response = client.get("/users").send();
/// assert_status!(response, 200);
/// assert_status!(response, StatusCode::OK);
/// ```
///
/// With custom message:
/// ```ignore
/// assert_status!(response, 404, "User should not be found");
/// ```
#[macro_export]
macro_rules! assert_status {
    ($response:expr, $expected:expr) => {{
        let response = &$response;
        let actual = response.status_code();
        // Use a trait to handle both u16 and StatusCode
        let expected_code: u16 = $crate::testing::IntoStatusU16::into_status_u16($expected);
        if actual != expected_code {
            panic!(
                "assertion failed: `(response.status() == {})`\n\
                 expected status: {}\n\
                 actual status: {}\n\
                 request id: {}\n\
                 response body: {}",
                expected_code,
                expected_code,
                actual,
                response.request_id(),
                response.text_opt().unwrap_or("<non-UTF8 body>")
            );
        }
    }};
    ($response:expr, $expected:expr, $($msg:tt)+) => {{
        let response = &$response;
        let actual = response.status_code();
        let expected_code: u16 = $crate::testing::IntoStatusU16::into_status_u16($expected);
        if actual != expected_code {
            panic!(
                "{}\n\
                 assertion failed: `(response.status() == {})`\n\
                 expected status: {}\n\
                 actual status: {}\n\
                 request id: {}\n\
                 response body: {}",
                format_args!($($msg)+),
                expected_code,
                expected_code,
                actual,
                response.request_id(),
                response.text_opt().unwrap_or("<non-UTF8 body>")
            );
        }
    }};
}

/// Asserts that a test response has a header with the expected value.
///
/// # Examples
///
/// ```ignore
/// use fastapi_core::assert_header;
///
/// let response = client.get("/api").send();
/// assert_header!(response, "Content-Type", "application/json");
/// ```
///
/// With custom message:
/// ```ignore
/// assert_header!(response, "X-Custom", "value", "Custom header should be set");
/// ```
#[macro_export]
macro_rules! assert_header {
    ($response:expr, $name:expr, $expected:expr) => {{
        let response = &$response;
        let name = $name;
        let expected = $expected;
        let actual = response.header_str(name);
        if actual != Some(expected) {
            panic!(
                "assertion failed: `(response.header(\"{}\") == \"{}\")`\n\
                 expected header '{}': \"{}\"\n\
                 actual header '{}': {:?}\n\
                 request id: {}",
                name,
                expected,
                name,
                expected,
                name,
                actual,
                response.request_id()
            );
        }
    }};
    ($response:expr, $name:expr, $expected:expr, $($msg:tt)+) => {{
        let response = &$response;
        let name = $name;
        let expected = $expected;
        let actual = response.header_str(name);
        if actual != Some(expected) {
            panic!(
                "{}\n\
                 assertion failed: `(response.header(\"{}\") == \"{}\")`\n\
                 expected header '{}': \"{}\"\n\
                 actual header '{}': {:?}\n\
                 request id: {}",
                format_args!($($msg)+),
                name,
                expected,
                name,
                expected,
                name,
                actual,
                response.request_id()
            );
        }
    }};
}

/// Asserts that a test response body contains the expected substring.
///
/// # Examples
///
/// ```ignore
/// use fastapi_core::assert_body_contains;
///
/// let response = client.get("/hello").send();
/// assert_body_contains!(response, "Hello");
/// ```
///
/// With custom message:
/// ```ignore
/// assert_body_contains!(response, "error", "Response should contain error message");
/// ```
#[macro_export]
macro_rules! assert_body_contains {
    ($response:expr, $expected:expr) => {{
        let response = &$response;
        let expected = $expected;
        let body = response.text();
        if !body.contains(expected) {
            panic!(
                "assertion failed: response body does not contain \"{}\"\n\
                 expected substring: \"{}\"\n\
                 actual body: \"{}\"\n\
                 request id: {}",
                expected, expected, body, response.request_id()
            );
        }
    }};
    ($response:expr, $expected:expr, $($msg:tt)+) => {{
        let response = &$response;
        let expected = $expected;
        let body = response.text();
        if !body.contains(expected) {
            panic!(
                "{}\n\
                 assertion failed: response body does not contain \"{}\"\n\
                 expected substring: \"{}\"\n\
                 actual body: \"{}\"\n\
                 request id: {}",
                format_args!($($msg)+),
                expected,
                expected,
                body,
                response.request_id()
            );
        }
    }};
}

/// Asserts that a test response body matches the expected JSON value (partial match).
///
/// This macro performs partial JSON matching: the actual response may contain
/// additional fields not present in `expected`, but all fields in `expected`
/// must be present with matching values.
///
/// # Examples
///
/// ```ignore
/// use fastapi_core::assert_json;
/// use serde_json::json;
///
/// let response = client.get("/user/1").send();
/// // Response: {"id": 1, "name": "Alice", "email": "alice@example.com"}
///
/// // Exact match
/// assert_json!(response, {"id": 1, "name": "Alice", "email": "alice@example.com"});
///
/// // Partial match (ignores email field)
/// assert_json!(response, {"name": "Alice"});
/// ```
///
/// With custom message:
/// ```ignore
/// assert_json!(response, {"status": "ok"}, "API should return success status");
/// ```
#[macro_export]
macro_rules! assert_json {
    ($response:expr, $expected:tt) => {{
        let response = &$response;
        let expected = serde_json::json!($expected);
        let actual: serde_json::Value = response
            .json()
            .expect("Failed to parse response body as JSON");

        if let Err(path) = $crate::testing::json_contains(&actual, &expected) {
            panic!(
                "assertion failed: JSON partial match failed at path '{}'\n\
                 expected (partial):\n{}\n\
                 actual:\n{}\n\
                 request id: {}",
                path,
                serde_json::to_string_pretty(&expected).unwrap_or_else(|_| format!("{:?}", expected)),
                serde_json::to_string_pretty(&actual).unwrap_or_else(|_| format!("{:?}", actual)),
                response.request_id()
            );
        }
    }};
    ($response:expr, $expected:tt, $($msg:tt)+) => {{
        let response = &$response;
        let expected = serde_json::json!($expected);
        let actual: serde_json::Value = response
            .json()
            .expect("Failed to parse response body as JSON");

        if let Err(path) = $crate::testing::json_contains(&actual, &expected) {
            panic!(
                "{}\n\
                 assertion failed: JSON partial match failed at path '{}'\n\
                 expected (partial):\n{}\n\
                 actual:\n{}\n\
                 request id: {}",
                format_args!($($msg)+),
                path,
                serde_json::to_string_pretty(&expected).unwrap_or_else(|_| format!("{:?}", expected)),
                serde_json::to_string_pretty(&actual).unwrap_or_else(|_| format!("{:?}", actual)),
                response.request_id()
            );
        }
    }};
}

/// Asserts that a test response body matches the given regex pattern.
///
/// Requires the `regex` feature to be enabled.
///
/// # Examples
///
/// ```ignore
/// use fastapi_core::assert_body_matches;
///
/// let response = client.get("/user/1").send();
/// assert_body_matches!(response, r"user_\d+");
/// assert_body_matches!(response, r"^Hello.*World$");
/// ```
#[cfg(feature = "regex")]
#[macro_export]
macro_rules! assert_body_matches {
    ($response:expr, $pattern:expr) => {{
        let response = &$response;
        let pattern = $pattern;
        let re = regex::Regex::new(pattern)
            .unwrap_or_else(|e| panic!("Invalid regex pattern '{}': {}", pattern, e));
        let body = response.text();
        if !re.is_match(body) {
            panic!(
                "assertion failed: response body does not match pattern\n\
                 pattern: \"{}\"\n\
                 actual body: \"{}\"\n\
                 request id: {}",
                pattern, body, response.request_id()
            );
        }
    }};
    ($response:expr, $pattern:expr, $($msg:tt)+) => {{
        let response = &$response;
        let pattern = $pattern;
        let re = regex::Regex::new(pattern)
            .unwrap_or_else(|e| panic!("Invalid regex pattern '{}': {}", pattern, e));
        let body = response.text();
        if !re.is_match(body) {
            panic!(
                "{}\n\
                 assertion failed: response body does not match pattern\n\
                 pattern: \"{}\"\n\
                 actual body: \"{}\"\n\
                 request id: {}",
                format_args!($($msg)+),
                pattern,
                body,
                response.request_id()
            );
        }
    }};
}

// Note: json_contains is already public and accessible via crate::testing::json_contains

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::dependency::{Depends, FromDependency};
    use crate::error::HttpError;
    use crate::middleware::BoxFuture;

    // Simple test handler
    struct EchoHandler;

    impl Handler for EchoHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            let method = format!("{:?}", req.method());
            let path = req.path().to_string();
            let body = format!("Method: {method}, Path: {path}");
            Box::pin(async move {
                Response::ok()
                    .header("content-type", b"text/plain".to_vec())
                    .body(ResponseBody::Bytes(body.into_bytes()))
            })
        }
    }

    // Handler that sets a cookie
    struct CookieHandler;

    impl Handler for CookieHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            Box::pin(async move {
                Response::ok()
                    .header("set-cookie", b"session=abc123".to_vec())
                    .body(ResponseBody::Bytes(b"Cookie set".to_vec()))
            })
        }
    }

    // Handler that echoes the cookie
    struct CookieEchoHandler;

    impl Handler for CookieEchoHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            let cookie = req.headers().get("cookie").map_or_else(
                || "no cookies".to_string(),
                |v| String::from_utf8_lossy(v).to_string(),
            );
            Box::pin(async move { Response::ok().body(ResponseBody::Bytes(cookie.into_bytes())) })
        }
    }

    #[derive(Clone)]
    struct OverrideDep {
        value: usize,
    }

    impl FromDependency for OverrideDep {
        type Error = HttpError;

        async fn from_dependency(
            _ctx: &RequestContext,
            _req: &mut Request,
        ) -> Result<Self, Self::Error> {
            Ok(Self { value: 1 })
        }
    }

    async fn override_dep_handler(ctx: &RequestContext, req: &mut Request) -> Response {
        let dep = Depends::<OverrideDep>::from_request(ctx, req)
            .await
            .expect("dependency extraction failed");
        Response::ok().body(ResponseBody::Bytes(dep.value.to_string().into_bytes()))
    }

    #[test]
    fn test_client_get() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/test/path").send();

        assert_eq!(response.status_code(), 200);
        assert_eq!(response.text(), "Method: Get, Path: /test/path");
    }

    #[test]
    fn test_client_post() {
        let client = TestClient::new(EchoHandler);
        let response = client.post("/api/items").send();

        assert_eq!(response.status_code(), 200);
        assert!(response.text().contains("Method: Post"));
    }

    #[test]
    fn test_app_dependency_override_used_by_test_client() {
        let app = App::builder()
            .route("/", Method::Get, override_dep_handler)
            .build();

        app.override_dependency_value(OverrideDep { value: 42 });

        let client = TestClient::new(app);
        let response = client.get("/").send();

        assert_eq!(response.text(), "42");
    }

    #[test]
    fn test_test_client_override_clear() {
        let client = TestClient::new(override_dep_handler);

        client.override_dependency_value(OverrideDep { value: 9 });
        let response = client.get("/").send();
        assert_eq!(response.text(), "9");

        client.clear_dependency_overrides();
        let response = client.get("/").send();
        assert_eq!(response.text(), "1");
    }

    #[test]
    fn test_client_all_methods() {
        let client = TestClient::new(EchoHandler);

        assert!(client.get("/").send().text().contains("Get"));
        assert!(client.post("/").send().text().contains("Post"));
        assert!(client.put("/").send().text().contains("Put"));
        assert!(client.delete("/").send().text().contains("Delete"));
        assert!(client.patch("/").send().text().contains("Patch"));
        assert!(client.options("/").send().text().contains("Options"));
        assert!(client.head("/").send().text().contains("Head"));
    }

    #[test]
    fn test_query_params() {
        let client = TestClient::new(EchoHandler);
        let response = client
            .get("/search")
            .query("q", "rust")
            .query("limit", "10")
            .send();

        assert_eq!(response.status_code(), 200);
    }

    #[test]
    fn test_response_assertions() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/test").send();

        let _ = response
            .assert_status_code(200)
            .assert_success()
            .assert_header("content-type", "text/plain")
            .assert_text_contains("Get");
    }

    #[test]
    fn test_response_status_checks() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/").send();

        assert!(response.is_success());
        assert!(!response.is_redirect());
        assert!(!response.is_client_error());
        assert!(!response.is_server_error());
    }

    #[test]
    fn test_cookie_jar() {
        let mut jar = CookieJar::new();
        assert!(jar.is_empty());

        jar.set("session", "abc123");
        jar.set("user", "alice");

        assert_eq!(jar.len(), 2);
        assert_eq!(jar.get("session"), Some("abc123"));
        assert_eq!(jar.get("user"), Some("alice"));

        let header = jar.to_cookie_header().unwrap();
        assert!(header.contains("session=abc123"));
        assert!(header.contains("user=alice"));

        jar.remove("session");
        assert_eq!(jar.len(), 1);
        assert_eq!(jar.get("session"), None);
    }

    #[test]
    fn test_cookie_persistence() {
        let client = TestClient::new(CookieHandler);

        // First request sets a cookie
        let _ = client.get("/set-cookie").send();

        // Cookie should be in the jar
        assert_eq!(client.cookies().get("session"), Some("abc123"));

        // Use a new handler that echoes cookies
        let client2 = TestClient::new(CookieEchoHandler);
        client2.cookies().set("session", "abc123");

        let response = client2.get("/check-cookie").send();
        assert!(response.text().contains("session=abc123"));
    }

    #[test]
    fn test_request_id_increments() {
        let client = TestClient::new(EchoHandler);

        let r1 = client.get("/").send();
        let r2 = client.get("/").send();
        let r3 = client.get("/").send();

        assert_eq!(r1.request_id(), 1);
        assert_eq!(r2.request_id(), 2);
        assert_eq!(r3.request_id(), 3);
    }

    #[test]
    fn test_client_with_seed() {
        let client = TestClient::with_seed(EchoHandler, 42);
        assert_eq!(client.seed(), Some(42));
    }

    #[test]
    fn test_client_clone() {
        let client = TestClient::new(EchoHandler);
        client.cookies().set("test", "value");

        let cloned = client.clone();

        // Cloned client shares cookies
        assert_eq!(cloned.cookies().get("test"), Some("value"));

        // And request ID counter
        let r1 = client.get("/").send();
        let r2 = cloned.get("/").send();
        assert_eq!(r1.request_id(), 1);
        assert_eq!(r2.request_id(), 2);
    }

    // =========================================================================
    // Tests for json_contains partial matching
    // =========================================================================

    #[test]
    fn test_json_contains_exact_match() {
        let actual = serde_json::json!({"id": 1, "name": "Alice"});
        let expected = serde_json::json!({"id": 1, "name": "Alice"});
        assert!(json_contains(&actual, &expected).is_ok());
    }

    #[test]
    fn test_json_contains_partial_match() {
        let actual = serde_json::json!({"id": 1, "name": "Alice", "email": "alice@example.com"});
        let expected = serde_json::json!({"name": "Alice"});
        assert!(json_contains(&actual, &expected).is_ok());
    }

    #[test]
    fn test_json_contains_nested_partial_match() {
        let actual = serde_json::json!({
            "user": {"id": 1, "name": "Alice", "email": "alice@example.com"},
            "status": "active"
        });
        let expected = serde_json::json!({
            "user": {"name": "Alice"}
        });
        assert!(json_contains(&actual, &expected).is_ok());
    }

    #[test]
    fn test_json_contains_mismatch_value() {
        let actual = serde_json::json!({"id": 1, "name": "Alice"});
        let expected = serde_json::json!({"name": "Bob"});
        let result = json_contains(&actual, &expected);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "$.name");
    }

    #[test]
    fn test_json_contains_missing_key() {
        let actual = serde_json::json!({"id": 1, "name": "Alice"});
        let expected = serde_json::json!({"email": "alice@example.com"});
        let result = json_contains(&actual, &expected);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "$.email");
    }

    #[test]
    fn test_json_contains_array_exact_match() {
        let actual = serde_json::json!({"items": [1, 2, 3]});
        let expected = serde_json::json!({"items": [1, 2, 3]});
        assert!(json_contains(&actual, &expected).is_ok());
    }

    #[test]
    fn test_json_contains_array_length_mismatch() {
        let actual = serde_json::json!({"items": [1, 2, 3]});
        let expected = serde_json::json!({"items": [1, 2]});
        let result = json_contains(&actual, &expected);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "$.items[length]");
    }

    #[test]
    fn test_json_contains_array_element_mismatch() {
        let actual = serde_json::json!({"items": [1, 2, 3]});
        let expected = serde_json::json!({"items": [1, 5, 3]});
        let result = json_contains(&actual, &expected);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "$.items[1]");
    }

    #[test]
    fn test_json_contains_primitives() {
        // Numbers
        assert!(json_contains(&serde_json::json!(42), &serde_json::json!(42)).is_ok());
        assert!(json_contains(&serde_json::json!(42), &serde_json::json!(43)).is_err());

        // Strings
        assert!(json_contains(&serde_json::json!("hello"), &serde_json::json!("hello")).is_ok());
        assert!(json_contains(&serde_json::json!("hello"), &serde_json::json!("world")).is_err());

        // Booleans
        assert!(json_contains(&serde_json::json!(true), &serde_json::json!(true)).is_ok());
        assert!(json_contains(&serde_json::json!(true), &serde_json::json!(false)).is_err());

        // Null
        assert!(json_contains(&serde_json::json!(null), &serde_json::json!(null)).is_ok());
    }

    #[test]
    fn test_json_contains_type_mismatch() {
        let actual = serde_json::json!({"id": "1"});
        let expected = serde_json::json!({"id": 1});
        let result = json_contains(&actual, &expected);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "$.id");
    }

    #[test]
    fn test_json_contains_deeply_nested() {
        let actual = serde_json::json!({
            "level1": {
                "level2": {
                    "level3": {
                        "value": 42,
                        "extra": "ignored"
                    }
                }
            }
        });
        let expected = serde_json::json!({
            "level1": {
                "level2": {
                    "level3": {
                        "value": 42
                    }
                }
            }
        });
        assert!(json_contains(&actual, &expected).is_ok());
    }

    // =========================================================================
    // Tests for assertion macros using handler that returns JSON
    // =========================================================================

    // Handler that returns JSON
    struct JsonHandler;

    impl Handler for JsonHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            let json = serde_json::json!({
                "id": 1,
                "name": "Alice",
                "email": "alice@example.com",
                "active": true
            });
            let body = serde_json::to_vec(&json).unwrap();
            Box::pin(async move {
                Response::ok()
                    .header("content-type", b"application/json".to_vec())
                    .header("x-request-id", b"req-123".to_vec())
                    .body(ResponseBody::Bytes(body))
            })
        }
    }

    // Handler that returns a specific status code
    #[allow(dead_code)]
    struct StatusHandler(u16);

    #[allow(dead_code)]
    impl Handler for StatusHandler {
        fn call<'a>(
            &'a self,
            _ctx: &'a RequestContext,
            _req: &'a mut Request,
        ) -> BoxFuture<'a, Response> {
            let status = StatusCode::from_u16(self.0);
            Box::pin(async move { Response::with_status(status) })
        }
    }

    #[test]
    fn test_assert_status_macro_with_u16() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/").send();
        crate::assert_status!(response, 200);
    }

    #[test]
    fn test_assert_status_macro_with_status_code() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/").send();
        crate::assert_status!(response, StatusCode::OK);
    }

    #[test]
    #[should_panic(expected = "assertion failed")]
    fn test_assert_status_macro_failure() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/").send();
        crate::assert_status!(response, 404);
    }

    #[test]
    fn test_assert_header_macro() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/").send();
        crate::assert_header!(response, "content-type", "text/plain");
    }

    #[test]
    #[should_panic(expected = "assertion failed")]
    fn test_assert_header_macro_failure() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/").send();
        crate::assert_header!(response, "content-type", "application/json");
    }

    #[test]
    fn test_assert_body_contains_macro() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/test").send();
        crate::assert_body_contains!(response, "Method: Get");
        crate::assert_body_contains!(response, "Path: /test");
    }

    #[test]
    #[should_panic(expected = "assertion failed")]
    fn test_assert_body_contains_macro_failure() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/test").send();
        crate::assert_body_contains!(response, "nonexistent");
    }

    #[test]
    fn test_assert_json_macro_partial_match() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/user").send();

        // Partial match - only check some fields
        crate::assert_json!(response, {"name": "Alice"});
        crate::assert_json!(response, {"id": 1, "active": true});
    }

    #[test]
    fn test_assert_json_macro_exact_match() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/user").send();

        // Exact match - all fields
        crate::assert_json!(response, {
            "id": 1,
            "name": "Alice",
            "email": "alice@example.com",
            "active": true
        });
    }

    #[test]
    #[should_panic(expected = "JSON partial match failed")]
    fn test_assert_json_macro_failure() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/user").send();
        crate::assert_json!(response, {"name": "Bob"});
    }

    // =========================================================================
    // Tests for method-based assertions
    // =========================================================================

    #[test]
    fn test_assert_json_contains_method() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/user").send();

        let _ = response.assert_json_contains(&serde_json::json!({"name": "Alice"}));
    }

    #[test]
    fn test_assert_header_exists() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/").send();

        let _ = response
            .assert_header_exists("content-type")
            .assert_header_exists("x-request-id");
    }

    #[test]
    #[should_panic(expected = "Expected header 'nonexistent' to exist")]
    fn test_assert_header_exists_failure() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/").send();
        let _ = response.assert_header_exists("nonexistent");
    }

    #[test]
    fn test_assert_header_missing() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/").send();

        let _ = response.assert_header_missing("x-nonexistent");
    }

    #[test]
    #[should_panic(expected = "Expected header 'content-type' to not exist")]
    fn test_assert_header_missing_failure() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/").send();
        let _ = response.assert_header_missing("content-type");
    }

    #[test]
    fn test_assert_content_type_contains() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/").send();

        let _ = response.assert_content_type_contains("application/json");
        let _ = response.assert_content_type_contains("json");
    }

    #[test]
    #[should_panic(expected = "Expected Content-Type to contain")]
    fn test_assert_content_type_contains_failure() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/").send();
        let _ = response.assert_content_type_contains("text/html");
    }

    #[test]
    fn test_assertion_chaining() {
        let client = TestClient::new(JsonHandler);
        let response = client.get("/user").send();

        // All assertions can be chained
        let _ = response
            .assert_status_code(200)
            .assert_success()
            .assert_header_exists("content-type")
            .assert_content_type_contains("json")
            .assert_json_contains(&serde_json::json!({"name": "Alice"}));
    }

    #[test]
    fn test_macro_with_custom_message() {
        let client = TestClient::new(EchoHandler);
        let response = client.get("/").send();

        // These should pass (custom message only shown on failure)
        crate::assert_status!(response, 200, "Expected 200 OK from echo handler");
        crate::assert_header!(
            response,
            "content-type",
            "text/plain",
            "Should have text content type"
        );
        crate::assert_body_contains!(response, "Get", "Should contain HTTP method");
    }
}
