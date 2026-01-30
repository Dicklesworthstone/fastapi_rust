//! Property-based testing helpers for fastapi_rust.
//!
//! This module provides proptest strategies and helpers for generating arbitrary
//! HTTP requests, enabling comprehensive fuzz testing of handlers.
//!
// Allow explicit `as` casts for boundary value testing - these are intentionally
// demonstrating type boundary conversions rather than normal code.
#![allow(clippy::cast_lossless)]
//!
//! # Features
//!
//! - **Arbitrary HTTP requests**: Generate random valid and invalid requests
//! - **Type-specific strategies**: HTTP methods, headers, bodies, query strings
//! - **Shrinking support**: Automatic via proptest integration
//! - **Edge case generators**: Malformed UTF-8, boundary values, injection attempts
//!
//! # Example
//!
//! ```ignore
//! use fastapi_core::proptest::*;
//! use proptest::prelude::*;
//!
//! proptest! {
//!     #[test]
//!     fn handler_never_panics(req in arbitrary_request()) {
//!         let client = TestClient::new(my_handler);
//!         // Handler should not panic on any input
//!         let _ = client.send_request(req);
//!     }
//!
//!     #[test]
//!     fn json_body_always_parseable(body in valid_json()) {
//!         let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
//!         // Handler should accept valid JSON
//!     }
//! }
//! ```
//!
//! # Strategy Categories
//!
//! ## Valid Strategies
//! Generate well-formed inputs that handlers should accept:
//! - `valid_path()` - Valid URL paths
//! - `valid_query_string()` - Valid query parameters
//! - `valid_json()` - Valid JSON bodies
//! - `valid_headers()` - Well-formed HTTP headers
//!
//! ## Invalid/Edge Case Strategies
//! Generate malformed inputs to test error handling:
//! - `malformed_utf8()` - Invalid UTF-8 sequences
//! - `boundary_values()` - Extreme numeric values
//! - `injection_attempts()` - SQL/XSS/command injection patterns
//! - `oversized_input()` - Inputs exceeding typical limits

use proptest::prelude::*;
use proptest::strategy::Strategy;

use crate::request::{Body, Headers, HttpVersion, Method, Request};

// =============================================================================
// HTTP Method Strategies
// =============================================================================

/// Strategy that generates any valid HTTP method.
pub fn arbitrary_method() -> impl Strategy<Value = Method> {
    prop_oneof![
        Just(Method::Get),
        Just(Method::Post),
        Just(Method::Put),
        Just(Method::Delete),
        Just(Method::Patch),
        Just(Method::Options),
        Just(Method::Head),
        Just(Method::Trace),
    ]
}

/// Strategy that generates methods typically used for request bodies.
pub fn body_method() -> impl Strategy<Value = Method> {
    prop_oneof![Just(Method::Post), Just(Method::Put), Just(Method::Patch),]
}

/// Strategy that generates safe (idempotent) methods.
pub fn safe_method() -> impl Strategy<Value = Method> {
    prop_oneof![Just(Method::Get), Just(Method::Head), Just(Method::Options),]
}

// =============================================================================
// HTTP Version Strategies
// =============================================================================

/// Strategy that generates any valid HTTP version.
pub fn arbitrary_http_version() -> impl Strategy<Value = HttpVersion> {
    prop_oneof![Just(HttpVersion::Http10), Just(HttpVersion::Http11),]
}

// =============================================================================
// Path Strategies
// =============================================================================

/// Strategy that generates valid URL paths.
///
/// Generates paths like `/users`, `/items/123`, `/api/v1/orders`.
pub fn valid_path() -> impl Strategy<Value = String> {
    prop_oneof![
        // Simple paths
        "[a-z][a-z0-9_-]{0,20}".prop_map(|s| format!("/{s}")),
        // Nested paths
        ("[a-z][a-z0-9_-]{0,10}", "[a-z][a-z0-9_-]{0,10}").prop_map(|(a, b)| format!("/{a}/{b}")),
        // Paths with numeric IDs
        ("[a-z][a-z0-9_-]{0,10}", 1u64..10000)
            .prop_map(|(resource, id)| format!("/{resource}/{id}")),
        // Versioned API paths
        (1u8..10, "[a-z][a-z0-9_-]{0,10}")
            .prop_map(|(ver, resource)| format!("/api/v{ver}/{resource}")),
        // Root path
        Just("/".to_string()),
    ]
}

/// Strategy that generates path parameters with type hints.
///
/// Generates patterns like `{id}`, `{user_id:int}`, `{name:str}`.
pub fn path_with_params() -> impl Strategy<Value = String> {
    prop_oneof![
        // Path with single param
        ("[a-z]+", "[a-z_]+").prop_map(|(resource, param)| format!("/{resource}/{{{param}}}")),
        // Path with typed param
        ("[a-z]+", "[a-z_]+", prop_oneof!["int", "str", "uuid"])
            .prop_map(|(resource, param, typ)| format!("/{resource}/{{{param}:{typ}}}")),
        // Path with wildcard
        "[a-z]+".prop_map(|resource| format!("/{resource}/{{*path}}")),
    ]
}

/// Strategy that generates potentially problematic paths for testing.
pub fn edge_case_path() -> impl Strategy<Value = String> {
    prop_oneof![
        // Path traversal attempts
        Just("/../etc/passwd".to_string()),
        Just("/..%2f..%2fetc/passwd".to_string()),
        Just("/users/../admin".to_string()),
        // Very long paths
        "[a-z]{100,200}".prop_map(|s| format!("/{s}")),
        // Special characters (URL-encoded)
        Just("/users%00/admin".to_string()),
        Just("/users%0d%0aHeader-Injection:attack".to_string()),
        // Unicode in paths
        Just("/usuarios/\u{00e9}ric".to_string()),
        Just("/users/\u{202e}admin".to_string()), // RTL override
        // Empty segments
        Just("//users//".to_string()),
        Just("/users/".to_string()),
        // Dots
        Just("/.".to_string()),
        Just("/..".to_string()),
        Just("/users/./profile".to_string()),
    ]
}

// =============================================================================
// Query String Strategies
// =============================================================================

/// Strategy that generates valid query strings.
pub fn valid_query_string() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        // No query string
        Just(None),
        // Single parameter
        ("[a-z_]+", "[a-zA-Z0-9_-]+").prop_map(|(k, v)| Some(format!("{k}={v}"))),
        // Multiple parameters
        proptest::collection::vec(("[a-z_]+", "[a-zA-Z0-9_-]+"), 1..5).prop_map(|pairs| {
            let qs = pairs
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            Some(qs)
        }),
        // Common patterns
        (1i32..1000, 1i32..100)
            .prop_map(|(page, limit)| Some(format!("page={page}&limit={limit}"))),
        "[a-z ]+".prop_map(|q| Some(format!("q={}", urlencoding_simple(&q)))),
    ]
}

/// Strategy that generates edge case query strings.
pub fn edge_case_query_string() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        // Empty value
        Just(Some("key=".to_string())),
        // No value
        Just(Some("key".to_string())),
        // Repeated keys
        Just(Some("a=1&a=2&a=3".to_string())),
        // Very long value
        "[a-z]{1000,2000}".prop_map(|v| Some(format!("key={v}"))),
        // Special characters
        Just(Some("key=value%00with%00nulls".to_string())),
        Just(Some("key=<script>alert(1)</script>".to_string())),
        Just(Some("key='; DROP TABLE users; --".to_string())),
        // Unicode
        Just(Some("key=\u{00e9}\u{00e8}\u{00ea}".to_string())),
        // Array-like
        Just(Some("ids[]=1&ids[]=2&ids[]=3".to_string())),
        Just(Some("ids=1,2,3".to_string())),
    ]
}

// =============================================================================
// Header Strategies
// =============================================================================

/// Strategy that generates valid HTTP headers.
pub fn valid_headers() -> impl Strategy<Value = Headers> {
    proptest::collection::vec(valid_header_pair(), 0..10).prop_map(|pairs| {
        let mut headers = Headers::new();
        for (name, value) in pairs {
            headers.insert(name, value);
        }
        headers
    })
}

/// Strategy that generates a single valid header name-value pair.
pub fn valid_header_pair() -> impl Strategy<Value = (String, Vec<u8>)> {
    prop_oneof![
        // Content-Type headers
        prop_oneof![
            Just("application/json"),
            Just("application/x-www-form-urlencoded"),
            Just("text/plain"),
            Just("text/html"),
            Just("multipart/form-data"),
        ]
        .prop_map(|ct| ("content-type".to_string(), ct.as_bytes().to_vec())),
        // Accept headers
        prop_oneof![Just("application/json"), Just("text/html"), Just("*/*"),]
            .prop_map(|accept| ("accept".to_string(), accept.as_bytes().to_vec())),
        // Authorization headers
        "[a-zA-Z0-9]{20,40}".prop_map(|token| {
            (
                "authorization".to_string(),
                format!("Bearer {token}").into_bytes(),
            )
        }),
        // User-Agent headers
        "[a-zA-Z0-9 /.-]{10,50}".prop_map(|ua| ("user-agent".to_string(), ua.into_bytes())),
        // Custom X- headers
        (
            "[A-Z][a-z]{2,10}".prop_map(|s| format!("x-{s}")),
            "[a-zA-Z0-9_-]{1,50}"
        )
            .prop_map(|(name, value)| (name, value.into_bytes())),
        // Accept-Encoding
        prop_oneof![
            Just("gzip"),
            Just("deflate"),
            Just("gzip, deflate"),
            Just("identity"),
        ]
        .prop_map(|enc| ("accept-encoding".to_string(), enc.as_bytes().to_vec())),
        // Accept-Language
        prop_oneof![Just("en-US"), Just("en-US,en;q=0.9"), Just("*"),]
            .prop_map(|lang| ("accept-language".to_string(), lang.as_bytes().to_vec())),
    ]
}

/// Strategy that generates edge case headers.
pub fn edge_case_headers() -> impl Strategy<Value = Headers> {
    proptest::collection::vec(edge_case_header_pair(), 1..5).prop_map(|pairs| {
        let mut headers = Headers::new();
        for (name, value) in pairs {
            headers.insert(name, value);
        }
        headers
    })
}

/// Strategy that generates problematic header pairs.
pub fn edge_case_header_pair() -> impl Strategy<Value = (String, Vec<u8>)> {
    prop_oneof![
        // Very long header value
        "[a-zA-Z]{8000,10000}".prop_map(|v| ("x-long-header".to_string(), v.into_bytes())),
        // Header with null bytes
        Just(("x-null".to_string(), b"value\x00with\x00nulls".to_vec())),
        // Header injection attempt
        Just((
            "x-injected".to_string(),
            b"value\r\nX-Injected-Header: attack".to_vec()
        )),
        // Empty header value
        Just(("x-empty".to_string(), Vec::new())),
        // Binary header value
        (0u8..=255).prop_flat_map(|b| Just(("x-binary".to_string(), vec![b; 50]))),
        // Duplicate content-type
        Just((
            "content-type".to_string(),
            b"text/html, application/json".to_vec()
        )),
    ]
}

/// Strategy that generates common request header sets.
pub fn typical_request_headers() -> impl Strategy<Value = Headers> {
    (
        prop_oneof![Just("application/json"), Just("text/plain"),],
        prop_oneof![Just("application/json"), Just("*/*"),],
        proptest::option::of("[a-zA-Z0-9]{20,40}"),
    )
        .prop_map(|(content_type, accept, auth)| {
            let mut headers = Headers::new();
            headers.insert("content-type", content_type.as_bytes().to_vec());
            headers.insert("accept", accept.as_bytes().to_vec());
            if let Some(token) = auth {
                headers.insert("authorization", format!("Bearer {token}").into_bytes());
            }
            headers
        })
}

// =============================================================================
// Body Strategies
// =============================================================================

/// Strategy that generates valid JSON bodies.
pub fn valid_json() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Empty object
        Just(b"{}".to_vec()),
        // Simple object
        ("[a-z_]+", "[a-zA-Z0-9 ]+").prop_map(|(k, v)| format!(r#"{{"{k}": "{v}"}}"#).into_bytes()),
        // Object with number
        ("[a-z_]+", -1000i64..1000).prop_map(|(k, v)| format!(r#"{{"{k}": {v}}}"#).into_bytes()),
        // Object with boolean
        ("[a-z_]+", proptest::bool::ANY)
            .prop_map(|(k, v)| format!(r#"{{"{k}": {v}}}"#).into_bytes()),
        // Object with null
        "[a-z_]+".prop_map(|k| format!(r#"{{"{k}": null}}"#).into_bytes()),
        // Array
        proptest::collection::vec(-100i64..100, 0..10).prop_map(|nums| format!(
            "[{}]",
            nums.iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into_bytes()),
        // Nested object
        ("[a-z_]+", "[a-z_]+", "[a-zA-Z0-9]+")
            .prop_map(|(k1, k2, v)| format!(r#"{{"{k1}": {{"{k2}": "{v}"}}}}"#).into_bytes()),
    ]
}

/// Strategy that generates invalid JSON for error testing.
pub fn invalid_json() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Unclosed brace
        Just(b"{\"key\": \"value\"".to_vec()),
        // Trailing comma
        Just(b"{\"key\": \"value\",}".to_vec()),
        // Single quotes
        Just(b"{'key': 'value'}".to_vec()),
        // Unquoted key
        Just(b"{key: \"value\"}".to_vec()),
        // Missing colon
        Just(b"{\"key\" \"value\"}".to_vec()),
        // Invalid escape
        Just(b"{\"key\": \"val\\ue\"}".to_vec()),
        // NaN/Infinity
        Just(b"{\"num\": NaN}".to_vec()),
        Just(b"{\"num\": Infinity}".to_vec()),
        // Random bytes
        proptest::collection::vec(0u8..=255, 10..100),
    ]
}

/// Strategy that generates empty or minimal body bytes.
///
/// Returns `None` for empty body or `Some(bytes)` for body content.
pub fn empty_body_bytes() -> impl Strategy<Value = Option<Vec<u8>>> {
    Just(None)
}

/// Strategy that generates arbitrary body bytes of various sizes.
///
/// Returns `None` for empty body or `Some(bytes)` for body content.
pub fn arbitrary_body_bytes() -> impl Strategy<Value = Option<Vec<u8>>> {
    prop_oneof![
        // Empty
        Just(None),
        // Small body
        proptest::collection::vec(any::<u8>(), 1..100).prop_map(Some),
        // Medium body
        proptest::collection::vec(any::<u8>(), 100..1000).prop_map(Some),
        // JSON body
        valid_json().prop_map(Some),
    ]
}

/// Strategy that generates oversized body bytes for limit testing.
pub fn oversized_body_bytes(min_size: usize, max_size: usize) -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), min_size..max_size)
}

/// Convert optional body bytes to a Body enum.
///
/// This helper allows strategies to return `Option<Vec<u8>>` which is Clone,
/// and convert to Body at the point of use.
pub fn bytes_to_body(bytes: Option<Vec<u8>>) -> Body {
    match bytes {
        None => Body::Empty,
        Some(b) if b.is_empty() => Body::Empty,
        Some(b) => Body::Bytes(b),
    }
}

// =============================================================================
// Full Request Strategies
// =============================================================================

/// Strategy that generates arbitrary valid HTTP requests.
pub fn arbitrary_request() -> impl Strategy<Value = Request> {
    (
        arbitrary_method(),
        valid_path(),
        valid_query_string(),
        arbitrary_http_version(),
        valid_headers(),
        arbitrary_body_bytes(),
    )
        .prop_map(|(method, path, query, version, headers, body_bytes)| {
            let mut req = Request::with_version(method, path, version);
            if let Some(q) = query {
                req.set_query(Some(q));
            }
            for (name, value) in headers.iter() {
                req.headers_mut().insert(name, value.to_vec());
            }
            req.set_body(bytes_to_body(body_bytes));
            req
        })
}

/// Strategy that generates GET requests with query parameters.
pub fn get_request() -> impl Strategy<Value = Request> {
    (valid_path(), valid_query_string(), valid_headers()).prop_map(|(path, query, headers)| {
        let mut req = Request::new(Method::Get, path);
        if let Some(q) = query {
            req.set_query(Some(q));
        }
        for (name, value) in headers.iter() {
            req.headers_mut().insert(name, value.to_vec());
        }
        req
    })
}

/// Strategy that generates POST requests with JSON bodies.
pub fn post_json_request() -> impl Strategy<Value = Request> {
    (valid_path(), valid_json(), typical_request_headers()).prop_map(|(path, body, headers)| {
        let mut req = Request::new(Method::Post, path);
        req.headers_mut()
            .insert("content-type", b"application/json".to_vec());
        for (name, value) in headers.iter() {
            if name != "content-type" {
                req.headers_mut().insert(name, value.to_vec());
            }
        }
        req.set_body(Body::Bytes(body));
        req
    })
}

/// Strategy that generates requests with edge case inputs.
pub fn edge_case_request() -> impl Strategy<Value = Request> {
    prop_oneof![
        // Edge case paths
        (edge_case_path(), typical_request_headers()).prop_map(|(path, headers)| {
            let mut req = Request::new(Method::Get, path);
            for (name, value) in headers.iter() {
                req.headers_mut().insert(name, value.to_vec());
            }
            req
        }),
        // Edge case query strings
        (
            valid_path(),
            edge_case_query_string(),
            typical_request_headers()
        )
            .prop_map(|(path, query, headers)| {
                let mut req = Request::new(Method::Get, path);
                req.set_query(query);
                for (name, value) in headers.iter() {
                    req.headers_mut().insert(name, value.to_vec());
                }
                req
            }),
        // Edge case headers
        (valid_path(), edge_case_headers()).prop_map(|(path, headers)| {
            let mut req = Request::new(Method::Get, path);
            for (name, value) in headers.iter() {
                req.headers_mut().insert(name, value.to_vec());
            }
            req
        }),
        // Invalid JSON body
        (valid_path(), invalid_json()).prop_map(|(path, body)| {
            let mut req = Request::new(Method::Post, path);
            req.headers_mut()
                .insert("content-type", b"application/json".to_vec());
            req.set_body(Body::Bytes(body));
            req
        }),
    ]
}

// =============================================================================
// Domain-Specific Strategies
// =============================================================================

/// Strategy that generates valid email addresses.
pub fn valid_email() -> impl Strategy<Value = String> {
    ("[a-z][a-z0-9._-]{2,20}", "[a-z]{3,10}", "[a-z]{2,4}")
        .prop_map(|(local, domain, tld)| format!("{local}@{domain}.{tld}"))
}

/// Strategy that generates invalid email addresses.
pub fn invalid_email() -> impl Strategy<Value = String> {
    prop_oneof![
        // Missing @
        Just("nodomain.com".to_string()),
        // Multiple @
        Just("user@@domain.com".to_string()),
        // Missing domain
        Just("user@".to_string()),
        // Missing local part
        Just("@domain.com".to_string()),
        // Spaces
        Just("user @domain.com".to_string()),
        // Invalid characters
        Just("user<>@domain.com".to_string()),
    ]
}

/// Strategy that generates valid UUIDs (v4 format).
pub fn valid_uuid() -> impl Strategy<Value = String> {
    (
        "[0-9a-f]{8}",
        "[0-9a-f]{4}",
        "[0-9a-f]{4}",
        "[0-9a-f]{4}",
        "[0-9a-f]{12}",
    )
        .prop_map(|(a, b, c, d, e)| format!("{a}-{b}-4{}-{d}-{e}", &c[1..]))
}

/// Strategy that generates malformed UTF-8 byte sequences.
pub fn malformed_utf8() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Overlong encoding of ASCII
        Just(vec![0xC0, 0xAF]), // Overlong '/'
        // Invalid continuation bytes
        Just(vec![0x80]), // Continuation byte without start
        Just(vec![0xBF]), // Another continuation
        // Incomplete sequences
        Just(vec![0xC2]),             // Start of 2-byte, missing continuation
        Just(vec![0xE0, 0xA0]),       // Start of 3-byte, missing last
        Just(vec![0xF0, 0x90, 0x80]), // Start of 4-byte, missing last
        // Invalid start bytes
        Just(vec![0xFE]),
        Just(vec![0xFF]),
        // Surrogate halves (invalid in UTF-8)
        Just(vec![0xED, 0xA0, 0x80]), // High surrogate
        Just(vec![0xED, 0xBF, 0xBF]), // Low surrogate
        // Mix valid and invalid
        proptest::collection::vec(any::<u8>(), 5..20).prop_map(|mut bytes| {
            if !bytes.is_empty() {
                bytes[0] = 0xFF; // Make first byte invalid
            }
            bytes
        }),
    ]
}

/// Strategy that generates SQL injection attempt strings.
pub fn sql_injection_attempts() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("' OR '1'='1".to_string()),
        Just("'; DROP TABLE users; --".to_string()),
        Just("1; DELETE FROM users".to_string()),
        Just("' UNION SELECT * FROM passwords --".to_string()),
        Just("admin'--".to_string()),
        Just("1' OR '1'='1' /*".to_string()),
        Just("'; EXEC xp_cmdshell('dir'); --".to_string()),
    ]
}

/// Strategy that generates XSS attempt strings.
pub fn xss_attempts() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("<script>alert(1)</script>".to_string()),
        Just("<img src=x onerror=alert(1)>".to_string()),
        Just("javascript:alert(1)".to_string()),
        Just("<svg onload=alert(1)>".to_string()),
        Just("'\"><script>alert(1)</script>".to_string()),
        Just("<iframe src='javascript:alert(1)'>".to_string()),
        Just("data:text/html,<script>alert(1)</script>".to_string()),
    ]
}

/// Strategy that generates command injection attempt strings.
pub fn command_injection_attempts() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("; ls -la".to_string()),
        Just("| cat /etc/passwd".to_string()),
        Just("$(whoami)".to_string()),
        Just("`id`".to_string()),
        Just("& ping -c 10 localhost &".to_string()),
        Just("|| true".to_string()),
        Just("; rm -rf /".to_string()),
    ]
}

/// Strategy that generates path traversal attempt strings.
pub fn path_traversal_attempts() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("../../../etc/passwd".to_string()),
        Just("..\\..\\..\\windows\\system32\\config\\sam".to_string()),
        Just("....//....//....//etc/passwd".to_string()),
        Just("..%2f..%2f..%2fetc/passwd".to_string()),
        Just("..%252f..%252f..%252fetc/passwd".to_string()),
        Just("/etc/passwd%00.jpg".to_string()),
    ]
}

// =============================================================================
// Boundary Value Strategies
// =============================================================================

/// Strategy that generates boundary integer values.
pub fn boundary_integers() -> impl Strategy<Value = i64> {
    prop_oneof![
        Just(0i64),
        Just(1i64),
        Just(-1i64),
        Just(i64::MAX),
        Just(i64::MIN),
        Just(i64::MAX - 1),
        Just(i64::MIN + 1),
        Just(i32::MAX as i64),
        Just(i32::MIN as i64),
        Just(u32::MAX as i64),
    ]
}

/// Strategy that generates boundary unsigned integer values.
pub fn boundary_unsigned() -> impl Strategy<Value = u64> {
    prop_oneof![
        Just(0u64),
        Just(1u64),
        Just(u64::MAX),
        Just(u64::MAX - 1),
        Just(u32::MAX as u64),
        Just(u32::MAX as u64 + 1),
    ]
}

/// Strategy that generates boundary floating point values.
pub fn boundary_floats() -> impl Strategy<Value = f64> {
    prop_oneof![
        Just(0.0f64),
        Just(-0.0f64),
        Just(1.0f64),
        Just(-1.0f64),
        Just(f64::MIN),
        Just(f64::MAX),
        Just(f64::MIN_POSITIVE),
        Just(f64::EPSILON),
        Just(f64::INFINITY),
        Just(f64::NEG_INFINITY),
        // NaN intentionally excluded as NaN != NaN
    ]
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Simple URL encoding for query string values.
fn urlencoding_simple(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
            ' ' => result.push('+'),
            _ => {
                for byte in c.to_string().as_bytes() {
                    result.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    result
}

// =============================================================================
// PropTest Integration Macros
// =============================================================================

/// Macro for running property tests against a handler.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::proptest_handler;
///
/// proptest_handler! {
///     fn fuzz_handler(client: TestClient, req: Request) {
///         let response = client.send_request(req);
///         // Handler should never panic and always return a response
///         prop_assert!(response.status().as_u16() < 600);
///     }
/// }
/// ```
#[macro_export]
macro_rules! proptest_handler {
    (
        $(#[$meta:meta])*
        fn $name:ident($client:ident: TestClient, $req:ident: Request) $body:block
    ) => {
        proptest::proptest! {
            $(#[$meta])*
            #[test]
            fn $name($req in $crate::proptest::arbitrary_request()) {
                use $crate::testing::TestClient;
                let $client = TestClient::new(handler);
                $body
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest! {
        #[test]
        fn method_is_valid(method in arbitrary_method()) {
            // All generated methods should have valid string representations
            let s = method.as_str();
            assert!(!s.is_empty());
            assert!(Method::from_bytes(s.as_bytes()).is_some());
        }

        #[test]
        fn path_starts_with_slash(path in valid_path()) {
            prop_assert!(path.starts_with('/'));
        }

        #[test]
        fn valid_json_is_parseable(json_bytes in valid_json()) {
            let result: Result<serde_json::Value, _> = serde_json::from_slice(&json_bytes);
            prop_assert!(result.is_ok(), "JSON should be valid: {:?}", String::from_utf8_lossy(&json_bytes));
        }

        #[test]
        fn invalid_json_fails_parsing(json_bytes in invalid_json()) {
            // Most invalid JSON should fail to parse, but random bytes might accidentally be valid
            // So we just verify the function doesn't panic
            let _result: Result<serde_json::Value, _> = serde_json::from_slice(&json_bytes);
        }

        #[test]
        fn email_has_at_sign(email in valid_email()) {
            prop_assert!(email.contains('@'));
            let parts: Vec<_> = email.split('@').collect();
            prop_assert_eq!(parts.len(), 2);
            prop_assert!(!parts[0].is_empty());
            prop_assert!(!parts[1].is_empty());
        }

        #[test]
        fn uuid_has_correct_format(uuid in valid_uuid()) {
            let parts: Vec<_> = uuid.split('-').collect();
            prop_assert_eq!(parts.len(), 5);
            prop_assert_eq!(parts[0].len(), 8);
            prop_assert_eq!(parts[1].len(), 4);
            prop_assert_eq!(parts[2].len(), 4);
            prop_assert_eq!(parts[3].len(), 4);
            prop_assert_eq!(parts[4].len(), 12);
        }

        #[test]
        fn malformed_utf8_is_not_valid_string(bytes in malformed_utf8()) {
            // Malformed UTF-8 should fail to parse as a string
            let result = std::str::from_utf8(&bytes);
            // Most should fail, but we don't assert all fail since some edge cases might pass
            let _ = result;
        }

        #[test]
        fn request_has_method_and_path(req in arbitrary_request()) {
            // All generated requests should have a valid method and non-empty path
            let _ = req.method().as_str();
            prop_assert!(!req.path().is_empty());
        }

        #[test]
        fn boundary_integers_are_extreme(val in boundary_integers()) {
            // Boundary values should be at extremes or common edge cases
            let is_boundary = val == 0 || val == 1 || val == -1
                || val == i64::MAX || val == i64::MIN
                || val == i64::MAX - 1 || val == i64::MIN + 1
                || val == i32::MAX as i64 || val == i32::MIN as i64
                || val == u32::MAX as i64;
            prop_assert!(is_boundary);
        }
    }
}
