//! Comprehensive tests for response handling in fastapi_rust.
//!
//! This test suite covers:
#![allow(clippy::redundant_closure_for_method_calls)] // .map(|s| s.to_string()) is idiomatic
//! - Response builder (headers, status, body)
//! - Response model validation (include/exclude fields, aliases)
//! - Streaming responses
//! - Response types (JSON, File, Redirect, HTML, Text, Binary)
//! - Cookie setting and deletion
//! - Content-Type inference
//! - No-body status codes (204, 304)
//! - Edge cases and error conditions
//!
//! Test failures include detailed logging showing expected vs actual response.

use fastapi_core::{
    // Response types
    Binary,
    // Context and types
    BoxFuture,
    FileResponse,
    Handler,
    Html,
    IntoResponse,
    NoContent,
    Redirect,
    Request,
    RequestContext,
    Response,
    ResponseBody,
    ResponseModelConfig,
    SameSite,
    SetCookie,
    StatusCode,
    // Testing
    TestClient,
    Text,
    ValidatedResponse,
    // Response helpers
    exclude_fields,
    include_fields,
    mime_type_for_extension,
};

use serde::{Deserialize, Serialize};

// =============================================================================
// Test Helpers
// =============================================================================

/// Handler that returns a response from a closure.
struct FnHandler<F: Fn() -> Response + Send + Sync>(F);

impl<F: Fn() -> Response + Send + Sync> Handler for FnHandler<F> {
    fn call<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a mut Request,
    ) -> BoxFuture<'a, Response> {
        let resp = (self.0)();
        Box::pin(async move { resp })
    }
}

/// Helper to create a simple sync handler.
fn sync_handler<F>(f: F) -> impl Handler
where
    F: Fn() -> Response + Send + Sync + 'static,
{
    FnHandler(f)
}

/// Format response details for logging on test failure.
fn format_response_details(response: &Response) -> String {
    let headers_str: Vec<String> = response
        .headers()
        .iter()
        .map(|(k, v)| format!("  {}: {}", k, String::from_utf8_lossy(v)))
        .collect();

    let body_preview = match response.body_ref() {
        ResponseBody::Empty => "<empty>".to_string(),
        ResponseBody::Bytes(b) => {
            let preview = String::from_utf8_lossy(&b[..b.len().min(200)]);
            if b.len() > 200 {
                format!("{}... ({} bytes total)", preview, b.len())
            } else {
                preview.to_string()
            }
        }
        ResponseBody::Stream(_) => "<streaming>".to_string(),
    };

    format!(
        "Status: {}\nHeaders:\n{}\nBody: {}",
        response.status().as_u16(),
        headers_str.join("\n"),
        body_preview
    )
}

// =============================================================================
// Section 1: Response Builder Tests
// =============================================================================

mod response_builder {
    use super::*;

    #[test]
    fn creates_response_with_status() {
        let response = Response::with_status(StatusCode::CREATED);
        assert_eq!(response.status().as_u16(), 201);
    }

    #[test]
    fn ok_creates_200_response() {
        let response = Response::ok();
        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn created_creates_201_response() {
        let response = Response::created();
        assert_eq!(response.status().as_u16(), 201);
    }

    #[test]
    fn no_content_creates_204_response() {
        let response = Response::no_content();
        assert_eq!(response.status().as_u16(), 204);
    }

    #[test]
    fn internal_error_creates_500_response() {
        let response = Response::internal_error();
        assert_eq!(response.status().as_u16(), 500);
    }

    #[test]
    fn adds_single_header() {
        let response = Response::ok().header("X-Custom", b"value".to_vec());

        let header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "X-Custom")
            .map(|(_, v)| v);

        assert_eq!(header, Some(&b"value".to_vec()));
    }

    #[test]
    fn adds_multiple_headers() {
        let response = Response::ok()
            .header("X-First", b"one".to_vec())
            .header("X-Second", b"two".to_vec())
            .header("X-Third", b"three".to_vec());

        assert_eq!(response.headers().len(), 3);
    }

    #[test]
    fn allows_duplicate_headers() {
        // HTTP allows multiple headers with the same name (e.g., Set-Cookie)
        let response = Response::ok()
            .header("Set-Cookie", b"a=1".to_vec())
            .header("Set-Cookie", b"b=2".to_vec());

        let cookie_headers: Vec<_> = response
            .headers()
            .iter()
            .filter(|(k, _)| k == "Set-Cookie")
            .collect();

        assert_eq!(cookie_headers.len(), 2);
    }

    #[test]
    fn sets_body_bytes() {
        let data = b"Hello, World!".to_vec();
        let response = Response::ok().body(ResponseBody::Bytes(data.clone()));

        if let ResponseBody::Bytes(body) = response.body_ref() {
            assert_eq!(body, &data);
        } else {
            panic!(
                "Expected Bytes body. Response details:\n{}",
                format_response_details(&response)
            );
        }
    }

    #[test]
    fn sets_empty_body() {
        let response = Response::ok().body(ResponseBody::Empty);
        assert!(response.body_ref().is_empty());
    }

    #[test]
    fn body_len_returns_correct_size() {
        let data = b"12345".to_vec();
        let body = ResponseBody::Bytes(data);
        assert_eq!(body.len(), 5);
    }

    #[test]
    fn empty_body_len_is_zero() {
        assert_eq!(ResponseBody::Empty.len(), 0);
    }

    #[test]
    fn chained_builder_methods() {
        let response = Response::with_status(StatusCode::ACCEPTED)
            .header("Content-Type", b"text/plain".to_vec())
            .header("X-Request-Id", b"abc123".to_vec())
            .body(ResponseBody::Bytes(b"Accepted".to_vec()));

        assert_eq!(response.status().as_u16(), 202);
        assert_eq!(response.headers().len(), 2);
        assert!(!response.body_ref().is_empty());
    }

    #[test]
    fn into_parts_decomposes_response() {
        let response = Response::ok()
            .header("X-Test", b"value".to_vec())
            .body(ResponseBody::Bytes(b"body".to_vec()));

        let (status, headers, body) = response.into_parts();

        assert_eq!(status.as_u16(), 200);
        assert_eq!(headers.len(), 1);
        if let ResponseBody::Bytes(b) = body {
            assert_eq!(b, b"body");
        }
    }

    #[test]
    fn json_response_serializes_struct() {
        #[derive(Serialize)]
        struct Data {
            id: i32,
            name: String,
        }

        let data = Data {
            id: 42,
            name: "test".to_string(),
        };
        let response = Response::json(&data).expect("JSON serialization failed");

        assert_eq!(response.status().as_u16(), 200);

        let content_type = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert_eq!(content_type, Some("application/json".to_string()));

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes.as_slice()).unwrap();
            assert_eq!(parsed["id"], 42);
            assert_eq!(parsed["name"], "test");
        }
    }

    #[test]
    fn json_response_handles_nested_structures() {
        #[derive(Serialize)]
        struct Inner {
            value: i32,
        }

        #[derive(Serialize)]
        struct Outer {
            items: Vec<Inner>,
            metadata: std::collections::HashMap<String, String>,
        }

        let data = Outer {
            items: vec![Inner { value: 1 }, Inner { value: 2 }],
            metadata: [("key".to_string(), "value".to_string())]
                .into_iter()
                .collect(),
        };

        let response = Response::json(&data).expect("JSON serialization failed");

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes.as_slice()).unwrap();
            assert_eq!(parsed["items"].as_array().unwrap().len(), 2);
            assert_eq!(parsed["metadata"]["key"], "value");
        }
    }
}

// =============================================================================
// Section 2: Response Model Validation Tests
// =============================================================================

mod response_model_validation {
    use super::*;

    #[test]
    fn config_include_whitelist_filters_fields() {
        let config = ResponseModelConfig::new()
            .include(["id", "name"].iter().map(|s| s.to_string()).collect());

        let value = serde_json::json!({
            "id": 1,
            "name": "Alice",
            "email": "alice@example.com",
            "password": "secret"
        });

        let filtered = config.filter_json(value).unwrap();

        assert!(filtered.get("id").is_some());
        assert!(filtered.get("name").is_some());
        assert!(filtered.get("email").is_none(), "email should be excluded");
        assert!(
            filtered.get("password").is_none(),
            "password should be excluded"
        );
    }

    #[test]
    fn config_exclude_blacklist_removes_fields() {
        let config = ResponseModelConfig::new().exclude(
            ["password", "secret_key"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );

        let value = serde_json::json!({
            "id": 1,
            "name": "Alice",
            "password": "secret123",
            "secret_key": "abc123"
        });

        let filtered = config.filter_json(value).unwrap();

        assert!(filtered.get("id").is_some());
        assert!(filtered.get("name").is_some());
        assert!(
            filtered.get("password").is_none(),
            "password should be excluded"
        );
        assert!(
            filtered.get("secret_key").is_none(),
            "secret_key should be excluded"
        );
    }

    #[test]
    fn config_exclude_none_removes_null_values() {
        let config = ResponseModelConfig::new().exclude_none(true);

        let value = serde_json::json!({
            "id": 1,
            "name": "Alice",
            "middle_name": null,
            "suffix": null
        });

        let filtered = config.filter_json(value).unwrap();

        assert!(filtered.get("id").is_some());
        assert!(filtered.get("name").is_some());
        assert!(
            filtered.get("middle_name").is_none(),
            "null middle_name should be excluded"
        );
        assert!(
            filtered.get("suffix").is_none(),
            "null suffix should be excluded"
        );
    }

    #[test]
    fn config_combined_include_and_exclude_none() {
        let config = ResponseModelConfig::new()
            .include(
                ["id", "name", "bio"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            )
            .exclude_none(true);

        let value = serde_json::json!({
            "id": 1,
            "name": "Alice",
            "bio": null,
            "password": "secret"
        });

        let filtered = config.filter_json(value).unwrap();

        assert!(filtered.get("id").is_some());
        assert!(filtered.get("name").is_some());
        assert!(
            filtered.get("bio").is_none(),
            "null bio should be excluded by exclude_none"
        );
        assert!(
            filtered.get("password").is_none(),
            "password not in include list"
        );
    }

    #[test]
    fn config_has_filtering_detects_active_config() {
        assert!(!ResponseModelConfig::new().has_filtering());

        assert!(
            ResponseModelConfig::new()
                .include(["id"].iter().map(|s| s.to_string()).collect())
                .has_filtering()
        );

        assert!(
            ResponseModelConfig::new()
                .exclude(["password"].iter().map(|s| s.to_string()).collect())
                .has_filtering()
        );

        assert!(
            ResponseModelConfig::new()
                .exclude_none(true)
                .has_filtering()
        );
    }

    #[test]
    fn validated_response_applies_config() {
        #[derive(Serialize)]
        struct User {
            id: i64,
            name: String,
            password_hash: String,
        }

        let user = User {
            id: 1,
            name: "Alice".to_string(),
            password_hash: "hashed_secret".to_string(),
        };

        let response = ValidatedResponse::new(user)
            .with_config(
                ResponseModelConfig::new()
                    .exclude(["password_hash"].iter().map(|s| s.to_string()).collect()),
            )
            .into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes.as_slice()).unwrap();
            assert!(parsed.get("id").is_some());
            assert!(parsed.get("name").is_some());
            assert!(
                parsed.get("password_hash").is_none(),
                "password_hash should be filtered out"
            );
        } else {
            panic!(
                "Expected Bytes body. Response:\n{}",
                format_response_details(&response)
            );
        }
    }

    #[test]
    fn exclude_fields_helper_works() {
        #[derive(Serialize)]
        struct Record {
            public_id: String,
            internal_id: i64,
            data: String,
        }

        let record = Record {
            public_id: "pub-123".to_string(),
            internal_id: 999,
            data: "content".to_string(),
        };

        let response = exclude_fields(record, &["internal_id"]).into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes.as_slice()).unwrap();
            assert!(parsed.get("public_id").is_some());
            assert!(parsed.get("data").is_some());
            assert!(
                parsed.get("internal_id").is_none(),
                "internal_id should be excluded"
            );
        }
    }

    #[test]
    fn include_fields_helper_works() {
        #[derive(Serialize)]
        struct Profile {
            id: i64,
            username: String,
            email: String,
            phone: String,
            ssn: String,
        }

        let profile = Profile {
            id: 1,
            username: "alice".to_string(),
            email: "alice@example.com".to_string(),
            phone: "555-1234".to_string(),
            ssn: "123-45-6789".to_string(),
        };

        let response = include_fields(profile, &["id", "username"]).into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes.as_slice()).unwrap();
            assert!(parsed.get("id").is_some());
            assert!(parsed.get("username").is_some());
            assert!(parsed.get("email").is_none());
            assert!(parsed.get("phone").is_none());
            assert!(parsed.get("ssn").is_none());
        }
    }

    #[test]
    fn filter_preserves_nested_objects() {
        let config = ResponseModelConfig::new()
            .exclude(["password"].iter().map(|s| s.to_string()).collect());

        let value = serde_json::json!({
            "user": {
                "id": 1,
                "name": "Alice"
            },
            "password": "secret"
        });

        let filtered = config.filter_json(value).unwrap();

        assert!(filtered.get("user").is_some());
        assert_eq!(filtered["user"]["id"], 1);
        assert!(filtered.get("password").is_none());
    }

    #[test]
    fn filter_preserves_arrays() {
        let config =
            ResponseModelConfig::new().include(["items"].iter().map(|s| s.to_string()).collect());

        let value = serde_json::json!({
            "items": [1, 2, 3],
            "metadata": "should be excluded"
        });

        let filtered = config.filter_json(value).unwrap();

        assert!(filtered.get("items").is_some());
        assert_eq!(filtered["items"].as_array().unwrap().len(), 3);
        assert!(filtered.get("metadata").is_none());
    }
}

// =============================================================================
// Section 3: Streaming Response Tests
// =============================================================================

mod streaming_responses {
    use super::*;

    #[test]
    fn stream_body_is_not_empty() {
        use asupersync::stream;

        let chunks = vec![b"chunk1".to_vec(), b"chunk2".to_vec()];
        let stream = stream::iter(chunks);
        let body = ResponseBody::stream(stream);

        // Stream bodies report 0 length (unknown until consumed)
        assert_eq!(body.len(), 0);
        // But they're not "empty" in the sense of having no content
        assert!(!matches!(body, ResponseBody::Empty));
    }

    #[test]
    fn response_with_stream_body() {
        use asupersync::stream;

        let chunks = vec![b"hello".to_vec(), b" world".to_vec()];
        let stream = stream::iter(chunks);
        let body = ResponseBody::stream(stream);

        let response = Response::ok()
            .header("content-type", b"text/plain".to_vec())
            .header("transfer-encoding", b"chunked".to_vec())
            .body(body);

        assert_eq!(response.status().as_u16(), 200);
        assert!(matches!(response.body_ref(), ResponseBody::Stream(_)));
    }

    #[test]
    fn response_body_debug_for_stream() {
        use asupersync::stream;

        let stream = stream::iter(vec![b"data".to_vec()]);
        let body = ResponseBody::stream(stream);

        let debug_str = format!("{body:?}");
        assert!(debug_str.contains("Stream"));
    }
}

// =============================================================================
// Section 4: Response Types Tests
// =============================================================================

mod response_types {
    use super::*;

    // -------------------------------------------------------------------------
    // Redirect tests
    // -------------------------------------------------------------------------

    #[test]
    fn redirect_temporary_307() {
        let redirect = Redirect::temporary("/new-location");
        let response = redirect.into_response();

        assert_eq!(response.status().as_u16(), 307);

        let location = response
            .headers()
            .iter()
            .find(|(k, _)| k == "location")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert_eq!(location, Some("/new-location".to_string()));
    }

    #[test]
    fn redirect_permanent_308() {
        let redirect = Redirect::permanent("https://example.com/new");
        let response = redirect.into_response();

        assert_eq!(response.status().as_u16(), 308);
    }

    #[test]
    fn redirect_see_other_303() {
        let redirect = Redirect::see_other("/result");
        let response = redirect.into_response();

        assert_eq!(response.status().as_u16(), 303);
    }

    #[test]
    fn redirect_moved_permanently_301() {
        let redirect = Redirect::moved_permanently("/old-to-new");
        let response = redirect.into_response();

        assert_eq!(response.status().as_u16(), 301);
    }

    #[test]
    fn redirect_found_302() {
        let redirect = Redirect::found("/temporary");
        let response = redirect.into_response();

        assert_eq!(response.status().as_u16(), 302);
    }

    #[test]
    fn redirect_with_query_params() {
        let redirect = Redirect::temporary("/search?q=test&page=1");
        let response = redirect.into_response();

        let location = response
            .headers()
            .iter()
            .find(|(k, _)| k == "location")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert_eq!(location, Some("/search?q=test&page=1".to_string()));
    }

    #[test]
    fn redirect_accessors() {
        let redirect = Redirect::permanent("https://example.com");
        assert_eq!(redirect.location(), "https://example.com");
        assert_eq!(redirect.status().as_u16(), 308);
    }

    // -------------------------------------------------------------------------
    // HTML tests
    // -------------------------------------------------------------------------

    #[test]
    fn html_response_content_type() {
        let html = Html::new("<html><body>Hello</body></html>");
        let response = html.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert_eq!(content_type, Some("text/html; charset=utf-8".to_string()));
    }

    #[test]
    fn html_response_status_200() {
        let html = Html::new("<p>test</p>");
        let response = html.into_response();
        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn html_from_string_conversion() {
        let html: Html = "content".into();
        assert_eq!(html.content(), "content");

        let html: Html = String::from("owned").into();
        assert_eq!(html.content(), "owned");
    }

    #[test]
    fn html_with_special_characters() {
        let html = Html::new("<p>&lt;script&gt;alert('xss')&lt;/script&gt;</p>");
        let response = html.into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let body = String::from_utf8_lossy(bytes);
            assert!(body.contains("&lt;script&gt;"));
        }
    }

    // -------------------------------------------------------------------------
    // Text tests
    // -------------------------------------------------------------------------

    #[test]
    fn text_response_content_type() {
        let text = Text::new("Plain text content");
        let response = text.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert_eq!(content_type, Some("text/plain; charset=utf-8".to_string()));
    }

    #[test]
    fn text_response_status_200() {
        let text = Text::new("hello");
        let response = text.into_response();
        assert_eq!(response.status().as_u16(), 200);
    }

    #[test]
    fn text_from_string_conversion() {
        let text: Text = "content".into();
        assert_eq!(text.content(), "content");
    }

    #[test]
    fn text_with_unicode() {
        let text = Text::new("Hello, 世界! 🌍");
        let response = text.into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let body = String::from_utf8_lossy(bytes);
            assert!(body.contains("世界"));
            assert!(body.contains("🌍"));
        }
    }

    // -------------------------------------------------------------------------
    // Binary tests
    // -------------------------------------------------------------------------

    #[test]
    fn binary_response_content_type() {
        let binary = Binary::new(vec![0x00, 0x01, 0x02]);
        let response = binary.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert_eq!(content_type, Some("application/octet-stream".to_string()));
    }

    #[test]
    fn binary_with_custom_content_type() {
        let binary = Binary::new(vec![0x89, 0x50, 0x4E, 0x47]).with_content_type("image/png");
        let response = binary.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert_eq!(content_type, Some("image/png".to_string()));
    }

    #[test]
    fn binary_from_slice() {
        let data = [0xDE, 0xAD, 0xBE, 0xEF];
        let binary: Binary = (&data[..]).into();
        assert_eq!(binary.data(), &data);
    }

    #[test]
    fn binary_from_vec() {
        let data = vec![1, 2, 3, 4, 5];
        let binary: Binary = data.clone().into();
        assert_eq!(binary.data(), &data[..]);
    }

    #[test]
    fn binary_preserves_null_bytes() {
        let data = vec![0x00, 0x00, 0x01, 0x00];
        let binary = Binary::new(data.clone());
        let response = binary.into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert_eq!(bytes.as_slice(), data.as_slice());
        }
    }

    // -------------------------------------------------------------------------
    // NoContent tests
    // -------------------------------------------------------------------------

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

    #[test]
    fn no_content_default() {
        let nc = NoContent;
        let response = nc.into_response();
        assert_eq!(response.status().as_u16(), 204);
    }

    // -------------------------------------------------------------------------
    // FileResponse tests
    // -------------------------------------------------------------------------

    #[test]
    fn file_response_not_found_returns_404() {
        let file = FileResponse::new("/nonexistent/file.txt");
        let response = file.into_response();
        assert_eq!(response.status().as_u16(), 404);
    }

    #[test]
    fn file_response_with_custom_content_type() {
        // Create a temp file
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_file_response.bin");
        std::fs::write(&test_file, b"binary content").unwrap();

        let file = FileResponse::new(&test_file).content_type("application/custom");
        let response = file.into_response();

        let content_type = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert_eq!(content_type, Some("application/custom".to_string()));

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn file_response_download_as_sets_disposition() {
        // Create a temp file for testing
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_download_as.txt");
        std::fs::write(&test_file, b"test content").unwrap();

        let file = FileResponse::new(&test_file).download_as("report.pdf");
        let response = file.into_response();

        let disposition = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-disposition")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert!(disposition.is_some());
        let disp = disposition.unwrap();
        assert!(disp.contains("attachment"));
        assert!(disp.contains("report.pdf"));

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn file_response_inline_disposition() {
        // Create a temp file for testing
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_inline.png");
        std::fs::write(&test_file, b"fake png").unwrap();

        let file = FileResponse::new(&test_file).inline();
        let response = file.into_response();

        let disposition = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-disposition")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert_eq!(disposition, Some("inline".to_string()));

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn file_response_escapes_filename() {
        // Create a temp file for testing
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_escape.pdf");
        std::fs::write(&test_file, b"fake pdf").unwrap();

        let file = FileResponse::new(&test_file).download_as("file\"with\"quotes.pdf");
        let response = file.into_response();

        let disposition = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-disposition")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert!(disposition.is_some());
        // The quotes should be escaped
        assert!(disposition.unwrap().contains("\\\""));

        let _ = std::fs::remove_file(test_file);
    }
}

// =============================================================================
// Section 5: Cookie Setting Tests
// =============================================================================

mod cookie_setting {
    use super::*;

    #[test]
    fn set_cookie_adds_header() {
        let response = Response::ok().set_cookie(SetCookie::new("session", "abc123"));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert!(cookie_header.is_some());
        assert!(cookie_header.unwrap().contains("session=abc123"));
    }

    #[test]
    fn set_cookie_with_http_only() {
        let response =
            Response::ok().set_cookie(SetCookie::new("session", "token").http_only(true));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("HttpOnly"));
    }

    #[test]
    fn set_cookie_with_secure() {
        let response = Response::ok().set_cookie(SetCookie::new("session", "token").secure(true));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("Secure"));
    }

    #[test]
    fn set_cookie_with_same_site_strict() {
        let response = Response::ok()
            .set_cookie(SetCookie::new("session", "token").same_site(SameSite::Strict));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("SameSite=Strict"));
    }

    #[test]
    fn set_cookie_with_same_site_lax() {
        let response =
            Response::ok().set_cookie(SetCookie::new("session", "token").same_site(SameSite::Lax));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("SameSite=Lax"));
    }

    #[test]
    fn set_cookie_with_max_age() {
        let response = Response::ok().set_cookie(SetCookie::new("session", "token").max_age(3600));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("Max-Age=3600"));
    }

    #[test]
    fn set_cookie_with_path() {
        let response = Response::ok().set_cookie(SetCookie::new("session", "token").path("/api"));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("Path=/api"));
    }

    #[test]
    fn set_cookie_with_domain() {
        let response =
            Response::ok().set_cookie(SetCookie::new("session", "token").domain("example.com"));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("Domain=example.com"));
    }

    #[test]
    fn set_cookie_with_all_attributes() {
        let response = Response::ok().set_cookie(
            SetCookie::new("session", "token123")
                .http_only(true)
                .secure(true)
                .same_site(SameSite::Strict)
                .max_age(7200)
                .path("/app")
                .domain("example.com"),
        );

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("session=token123"));
        assert!(cookie_header.contains("HttpOnly"));
        assert!(cookie_header.contains("Secure"));
        assert!(cookie_header.contains("SameSite=Strict"));
        assert!(cookie_header.contains("Max-Age=7200"));
        assert!(cookie_header.contains("Path=/app"));
        assert!(cookie_header.contains("Domain=example.com"));
    }

    #[test]
    fn set_multiple_cookies() {
        let response = Response::ok()
            .set_cookie(SetCookie::new("session", "abc"))
            .set_cookie(SetCookie::new("preferences", "dark"))
            .set_cookie(SetCookie::new("locale", "en-US"));

        let cookie_headers: Vec<_> = response
            .headers()
            .iter()
            .filter(|(k, _)| k == "set-cookie")
            .collect();

        assert_eq!(cookie_headers.len(), 3);
    }

    #[test]
    fn delete_cookie_sets_expired() {
        let response = Response::ok().delete_cookie("session");

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("session="));
        assert!(cookie_header.contains("Max-Age=0"));
    }

    #[test]
    fn set_and_delete_cookies_together() {
        let response = Response::ok()
            .set_cookie(SetCookie::new("new_session", "xyz"))
            .delete_cookie("old_session");

        let cookie_headers: Vec<String> = response
            .headers()
            .iter()
            .filter(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
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

// =============================================================================
// Section 6: Content-Type Inference Tests
// =============================================================================

mod content_type_inference {
    use super::*;

    #[test]
    fn mime_type_text_extensions() {
        assert_eq!(mime_type_for_extension("html"), "text/html; charset=utf-8");
        assert_eq!(mime_type_for_extension("htm"), "text/html; charset=utf-8");
        assert_eq!(mime_type_for_extension("css"), "text/css; charset=utf-8");
        assert_eq!(
            mime_type_for_extension("js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            mime_type_for_extension("mjs"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(mime_type_for_extension("txt"), "text/plain; charset=utf-8");
        assert_eq!(mime_type_for_extension("csv"), "text/csv; charset=utf-8");
        assert_eq!(
            mime_type_for_extension("md"),
            "text/markdown; charset=utf-8"
        );
    }

    #[test]
    fn mime_type_application_extensions() {
        assert_eq!(mime_type_for_extension("json"), "application/json");
        assert_eq!(mime_type_for_extension("map"), "application/json");
        assert_eq!(mime_type_for_extension("xml"), "application/xml");
        assert_eq!(mime_type_for_extension("pdf"), "application/pdf");
        assert_eq!(mime_type_for_extension("zip"), "application/zip");
        assert_eq!(mime_type_for_extension("gz"), "application/gzip");
        assert_eq!(mime_type_for_extension("gzip"), "application/gzip");
        assert_eq!(mime_type_for_extension("wasm"), "application/wasm");
    }

    #[test]
    fn mime_type_image_extensions() {
        assert_eq!(mime_type_for_extension("png"), "image/png");
        assert_eq!(mime_type_for_extension("jpg"), "image/jpeg");
        assert_eq!(mime_type_for_extension("jpeg"), "image/jpeg");
        assert_eq!(mime_type_for_extension("gif"), "image/gif");
        assert_eq!(mime_type_for_extension("webp"), "image/webp");
        assert_eq!(mime_type_for_extension("svg"), "image/svg+xml");
        assert_eq!(mime_type_for_extension("ico"), "image/x-icon");
        assert_eq!(mime_type_for_extension("bmp"), "image/bmp");
        assert_eq!(mime_type_for_extension("avif"), "image/avif");
    }

    #[test]
    fn mime_type_font_extensions() {
        assert_eq!(mime_type_for_extension("woff"), "font/woff");
        assert_eq!(mime_type_for_extension("woff2"), "font/woff2");
        assert_eq!(mime_type_for_extension("ttf"), "font/ttf");
        assert_eq!(mime_type_for_extension("otf"), "font/otf");
        assert_eq!(
            mime_type_for_extension("eot"),
            "application/vnd.ms-fontobject"
        );
    }

    #[test]
    fn mime_type_audio_extensions() {
        assert_eq!(mime_type_for_extension("mp3"), "audio/mpeg");
        assert_eq!(mime_type_for_extension("wav"), "audio/wav");
        assert_eq!(mime_type_for_extension("ogg"), "audio/ogg");
        assert_eq!(mime_type_for_extension("flac"), "audio/flac");
        assert_eq!(mime_type_for_extension("aac"), "audio/aac");
        assert_eq!(mime_type_for_extension("m4a"), "audio/mp4");
    }

    #[test]
    fn mime_type_video_extensions() {
        assert_eq!(mime_type_for_extension("mp4"), "video/mp4");
        assert_eq!(mime_type_for_extension("webm"), "video/webm");
        assert_eq!(mime_type_for_extension("avi"), "video/x-msvideo");
        assert_eq!(mime_type_for_extension("mov"), "video/quicktime");
        assert_eq!(mime_type_for_extension("mkv"), "video/x-matroska");
    }

    #[test]
    fn mime_type_document_extensions() {
        assert_eq!(mime_type_for_extension("doc"), "application/msword");
        assert_eq!(
            mime_type_for_extension("docx"),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        assert_eq!(mime_type_for_extension("xls"), "application/vnd.ms-excel");
        assert_eq!(
            mime_type_for_extension("xlsx"),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        assert_eq!(
            mime_type_for_extension("ppt"),
            "application/vnd.ms-powerpoint"
        );
        assert_eq!(
            mime_type_for_extension("pptx"),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        );
    }

    #[test]
    fn mime_type_archive_extensions() {
        assert_eq!(mime_type_for_extension("tar"), "application/x-tar");
        assert_eq!(mime_type_for_extension("rar"), "application/vnd.rar");
        assert_eq!(mime_type_for_extension("7z"), "application/x-7z-compressed");
    }

    #[test]
    fn mime_type_case_insensitive() {
        assert_eq!(mime_type_for_extension("HTML"), "text/html; charset=utf-8");
        assert_eq!(mime_type_for_extension("PNG"), "image/png");
        assert_eq!(mime_type_for_extension("Json"), "application/json");
        assert_eq!(mime_type_for_extension("PDF"), "application/pdf");
        assert_eq!(mime_type_for_extension("MP4"), "video/mp4");
    }

    #[test]
    fn mime_type_unknown_returns_octet_stream() {
        assert_eq!(
            mime_type_for_extension("unknown"),
            "application/octet-stream"
        );
        assert_eq!(mime_type_for_extension("xyz"), "application/octet-stream");
        assert_eq!(mime_type_for_extension("foo"), "application/octet-stream");
        assert_eq!(mime_type_for_extension(""), "application/octet-stream");
    }
}

// =============================================================================
// Section 7: No-Body Status Code Tests
// =============================================================================

mod no_body_status_codes {
    use super::*;

    #[test]
    fn status_204_no_content_disallows_body() {
        assert!(!StatusCode::NO_CONTENT.allows_body());
    }

    #[test]
    fn status_304_not_modified_disallows_body() {
        assert!(!StatusCode::NOT_MODIFIED.allows_body());
    }

    #[test]
    fn status_100_continue_disallows_body() {
        assert!(!StatusCode::CONTINUE.allows_body());
    }

    #[test]
    fn status_101_switching_protocols_disallows_body() {
        assert!(!StatusCode::SWITCHING_PROTOCOLS.allows_body());
    }

    #[test]
    fn status_200_allows_body() {
        assert!(StatusCode::OK.allows_body());
    }

    #[test]
    fn status_201_allows_body() {
        assert!(StatusCode::CREATED.allows_body());
    }

    #[test]
    fn status_301_allows_body() {
        assert!(StatusCode::MOVED_PERMANENTLY.allows_body());
    }

    #[test]
    fn status_400_allows_body() {
        assert!(StatusCode::BAD_REQUEST.allows_body());
    }

    #[test]
    fn status_500_allows_body() {
        assert!(StatusCode::INTERNAL_SERVER_ERROR.allows_body());
    }

    #[test]
    fn canonical_reason_for_common_codes() {
        assert_eq!(StatusCode::OK.canonical_reason(), "OK");
        assert_eq!(StatusCode::CREATED.canonical_reason(), "Created");
        assert_eq!(StatusCode::NO_CONTENT.canonical_reason(), "No Content");
        assert_eq!(StatusCode::NOT_MODIFIED.canonical_reason(), "Not Modified");
        assert_eq!(StatusCode::BAD_REQUEST.canonical_reason(), "Bad Request");
        assert_eq!(StatusCode::NOT_FOUND.canonical_reason(), "Not Found");
        assert_eq!(
            StatusCode::INTERNAL_SERVER_ERROR.canonical_reason(),
            "Internal Server Error"
        );
    }

    #[test]
    fn canonical_reason_for_unknown_code() {
        let unknown = StatusCode::from_u16(999);
        assert_eq!(unknown.canonical_reason(), "Unknown");
    }

    #[test]
    fn status_code_from_u16() {
        let code = StatusCode::from_u16(418);
        assert_eq!(code.as_u16(), 418);
    }

    #[test]
    fn status_code_equality() {
        assert_eq!(StatusCode::OK, StatusCode::from_u16(200));
        assert_ne!(StatusCode::OK, StatusCode::CREATED);
    }

    #[test]
    fn status_code_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(StatusCode::OK);
        set.insert(StatusCode::CREATED);
        set.insert(StatusCode::OK); // Duplicate

        assert_eq!(set.len(), 2);
    }
}

// =============================================================================
// Section 8: IntoResponse Trait Tests
// =============================================================================

mod into_response_trait {
    use super::*;

    #[test]
    fn unit_into_response_returns_204() {
        let response = ().into_response();
        assert_eq!(response.status().as_u16(), 204);
    }

    #[test]
    fn static_str_into_response() {
        let response = "Hello, World!".into_response();

        assert_eq!(response.status().as_u16(), 200);

        let content_type = response
            .headers()
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());
        assert_eq!(content_type, Some("text/plain; charset=utf-8".to_string()));

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert_eq!(bytes, b"Hello, World!");
        }
    }

    #[test]
    fn string_into_response() {
        let response = String::from("Dynamic content").into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert_eq!(bytes, b"Dynamic content");
        }
    }

    #[test]
    fn result_ok_into_response() {
        let result: Result<&str, &str> = Ok("Success");
        let response = result.into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert_eq!(bytes, b"Success");
        }
    }

    #[test]
    fn result_err_into_response() {
        let result: Result<&str, Response> = Err(Response::with_status(StatusCode::BAD_REQUEST));
        let response = result.into_response();

        assert_eq!(response.status().as_u16(), 400);
    }

    #[test]
    fn response_into_response_identity() {
        let original = Response::created().header("X-Test", b"value".to_vec());
        let response = original.into_response();

        assert_eq!(response.status().as_u16(), 201);
    }

    #[test]
    fn status_code_into_response() {
        // StatusCode doesn't impl IntoResponse directly, but Response::with_status does
        let response = Response::with_status(StatusCode::ACCEPTED);
        assert_eq!(response.status().as_u16(), 202);
    }
}

// =============================================================================
// Section 9: Edge Cases and Error Conditions
// =============================================================================

mod edge_cases {
    use super::*;

    #[test]
    fn empty_body_is_empty() {
        let body = ResponseBody::Empty;
        assert!(body.is_empty());
        assert_eq!(body.len(), 0);
    }

    #[test]
    fn empty_bytes_body_is_empty() {
        let body = ResponseBody::Bytes(vec![]);
        assert!(body.is_empty());
        assert_eq!(body.len(), 0);
    }

    #[test]
    fn non_empty_bytes_body_is_not_empty() {
        let body = ResponseBody::Bytes(vec![1]);
        assert!(!body.is_empty());
        assert_eq!(body.len(), 1);
    }

    #[test]
    fn response_with_empty_header_value() {
        let response = Response::ok().header("X-Empty", b"".to_vec());

        let header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "X-Empty")
            .map(|(_, v)| v);

        assert_eq!(header, Some(&b"".to_vec()));
    }

    #[test]
    fn response_with_binary_header_value() {
        // Null bytes (0x00) are stripped by sanitize_header_value for HTTP safety
        let binary_value = vec![0x00, 0x01, 0xFF];
        let expected_sanitized = vec![0x01, 0xFF]; // Null byte removed
        let response = Response::ok().header("X-Binary", binary_value);

        let header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "X-Binary")
            .map(|(_, v)| v);

        assert_eq!(header, Some(&expected_sanitized));
    }

    #[test]
    fn response_with_unicode_header_value() {
        let unicode = "Hello, 世界!".as_bytes().to_vec();
        let response = Response::ok().header("X-Unicode", unicode.clone());

        let header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "X-Unicode")
            .map(|(_, v)| v);

        assert_eq!(header, Some(&unicode));
    }

    #[test]
    fn very_large_body() {
        let large_body = vec![b'x'; 1_000_000]; // 1MB
        let response = Response::ok().body(ResponseBody::Bytes(large_body.clone()));

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert_eq!(bytes.len(), 1_000_000);
        }
    }

    #[test]
    fn json_serialization_with_special_chars() {
        #[derive(Serialize)]
        struct Data {
            text: String,
        }

        let data = Data {
            text: "Hello \"world\" with <script> and \n newlines".to_string(),
        };

        let response = Response::json(&data).unwrap();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            let parsed: serde_json::Value = serde_json::from_slice(bytes.as_slice()).unwrap();
            // JSON escaping should work correctly
            assert!(parsed["text"].as_str().unwrap().contains("\"world\""));
        }
    }

    #[test]
    fn response_model_config_filter_non_object() {
        let config =
            ResponseModelConfig::new().include(["id"].iter().map(|s| s.to_string()).collect());

        // Filtering a non-object should return it unchanged
        let array = serde_json::json!([1, 2, 3]);
        let filtered = config.filter_json(array.clone()).unwrap();
        assert_eq!(filtered, serde_json::json!([1, 2, 3]));

        let scalar = serde_json::json!(42);
        let filtered = config.filter_json(scalar).unwrap();
        assert_eq!(filtered, serde_json::json!(42));

        let null = serde_json::json!(null);
        let filtered = config.filter_json(null).unwrap();
        assert_eq!(filtered, serde_json::json!(null));
    }

    #[test]
    fn redirect_with_fragment() {
        let redirect = Redirect::temporary("/page#section");
        let response = redirect.into_response();

        let location = response
            .headers()
            .iter()
            .find(|(k, _)| k == "location")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert_eq!(location, Some("/page#section".to_string()));
    }

    #[test]
    fn redirect_with_absolute_url() {
        let redirect = Redirect::permanent("https://example.com/path?query=1#frag");
        let response = redirect.into_response();

        let location = response
            .headers()
            .iter()
            .find(|(k, _)| k == "location")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string());

        assert_eq!(
            location,
            Some("https://example.com/path?query=1#frag".to_string())
        );
    }

    #[test]
    fn cookie_with_special_value_characters() {
        // Cookie values can contain some special chars
        let response = Response::ok().set_cookie(SetCookie::new("data", "a=b&c=d"));

        let cookie_header = response
            .headers()
            .iter()
            .find(|(k, _)| k == "set-cookie")
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .unwrap();

        assert!(cookie_header.contains("data=a=b&c=d"));
    }

    #[test]
    fn html_with_empty_content() {
        let html = Html::new("");
        let response = html.into_response();

        assert_eq!(response.status().as_u16(), 200);

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert!(bytes.is_empty());
        }
    }

    #[test]
    fn text_with_empty_content() {
        let text = Text::new("");
        let response = text.into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert!(bytes.is_empty());
        }
    }

    #[test]
    fn binary_with_empty_data() {
        let binary = Binary::new(vec![]);
        let response = binary.into_response();

        if let ResponseBody::Bytes(bytes) = response.body_ref() {
            assert!(bytes.is_empty());
        }
    }
}

// =============================================================================
// Section 10: TestClient Integration Tests
// =============================================================================

mod test_client_integration {
    use super::*;

    #[test]
    fn test_client_get_request() {
        let handler = sync_handler(|| Response::ok().body(ResponseBody::Bytes(b"Hello".to_vec())));

        let client = TestClient::new(handler);
        let response = client.get("/test").send();

        let _ = response.assert_status(StatusCode::OK);
        assert_eq!(response.text(), "Hello");
    }

    #[test]
    fn test_client_post_with_json() {
        #[derive(Serialize)]
        struct Input {
            value: i32,
        }

        let handler = sync_handler(|| {
            Response::created()
                .header("content-type", b"application/json".to_vec())
                .body(ResponseBody::Bytes(b"{\"created\":true}".to_vec()))
        });

        let client = TestClient::new(handler);
        let response = client.post("/items").json(&Input { value: 42 }).send();

        let _ = response.assert_status(StatusCode::CREATED);
    }

    #[test]
    fn test_client_cookie_persistence() {
        let handler =
            sync_handler(|| Response::ok().set_cookie(SetCookie::new("session", "abc123")));

        let client = TestClient::new(handler);

        // First request sets cookie
        let _ = client.get("/login").send();

        // Cookie should be in jar
        assert_eq!(client.cookies().get("session"), Some("abc123"));
    }

    #[test]
    fn test_client_clear_cookies() {
        let handler = sync_handler(|| Response::ok().set_cookie(SetCookie::new("test", "value")));

        let client = TestClient::new(handler);
        let _ = client.get("/").send();

        assert!(!client.cookies().is_empty());

        client.clear_cookies();

        assert!(client.cookies().is_empty());
    }

    #[test]
    fn test_client_custom_headers() {
        let handler = sync_handler(Response::ok);

        let client = TestClient::new(handler);
        let response = client
            .get("/api")
            .header("Authorization", "Bearer token123")
            .header("Accept", "application/json")
            .send();

        let _ = response.assert_success();
    }

    #[test]
    fn test_client_query_params() {
        let handler = sync_handler(Response::ok);

        let client = TestClient::new(handler);
        let response = client
            .get("/search")
            .query("q", "rust")
            .query("page", "1")
            .send();

        let _ = response.assert_success();
    }

    #[test]
    fn test_response_is_success() {
        let handler = sync_handler(Response::ok);
        let client = TestClient::new(handler);
        let response = client.get("/").send();

        assert!(response.is_success());
        assert!(!response.is_redirect());
        assert!(!response.is_client_error());
        assert!(!response.is_server_error());
    }

    #[test]
    fn test_response_is_redirect() {
        let handler = sync_handler(|| Redirect::temporary("/new").into_response());
        let client = TestClient::new(handler);
        let response = client.get("/old").send();

        assert!(!response.is_success());
        assert!(response.is_redirect());
    }

    #[test]
    fn test_response_is_client_error() {
        let handler = sync_handler(|| Response::with_status(StatusCode::NOT_FOUND));
        let client = TestClient::new(handler);
        let response = client.get("/missing").send();

        assert!(!response.is_success());
        assert!(response.is_client_error());
    }

    #[test]
    fn test_response_is_server_error() {
        let handler = sync_handler(Response::internal_error);
        let client = TestClient::new(handler);
        let response = client.get("/error").send();

        assert!(!response.is_success());
        assert!(response.is_server_error());
    }

    #[test]
    fn test_response_json_parsing() {
        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        struct User {
            id: i32,
            name: String,
        }

        let handler = sync_handler(|| {
            Response::json(&User {
                id: 1,
                name: "Alice".to_string(),
            })
            .unwrap()
        });

        let client = TestClient::new(handler);
        let response = client.get("/user").send();

        let user: User = response.json().expect("Failed to parse JSON");
        assert_eq!(user.id, 1);
        assert_eq!(user.name, "Alice");
    }

    #[test]
    fn test_response_content_type() {
        let handler = sync_handler(|| Html::new("<html><body>Test</body></html>").into_response());

        let client = TestClient::new(handler);
        let response = client.get("/page").send();

        assert_eq!(response.content_type(), Some("text/html; charset=utf-8"));
    }

    #[test]
    fn test_response_header_lookup() {
        let handler = sync_handler(|| {
            Response::ok()
                .header("X-Custom-Header", b"custom-value".to_vec())
                .header("X-Another", b"another-value".to_vec())
        });

        let client = TestClient::new(handler);
        let response = client.get("/").send();

        // Case-insensitive lookup
        assert_eq!(response.header_str("x-custom-header"), Some("custom-value"));
        assert_eq!(response.header_str("X-ANOTHER"), Some("another-value"));
        assert!(response.header_str("nonexistent").is_none());
    }

    #[test]
    fn test_response_assertion_helpers() {
        let handler = sync_handler(|| {
            Response::ok()
                .header("content-type", b"text/plain".to_vec())
                .body(ResponseBody::Bytes(b"Hello, World!".to_vec()))
        });

        let client = TestClient::new(handler);
        let response = client.get("/").send();

        // Chain assertions
        let _ = response
            .assert_status(StatusCode::OK)
            .assert_header("content-type", "text/plain")
            .assert_text("Hello, World!")
            .assert_success();
    }

    #[test]
    fn test_response_text_contains() {
        let handler = sync_handler(|| {
            Response::ok().body(ResponseBody::Bytes(
                b"The quick brown fox jumps over the lazy dog".to_vec(),
            ))
        });

        let client = TestClient::new(handler);
        let response = client.get("/").send();

        let _ = response
            .assert_text_contains("quick brown")
            .assert_text_contains("lazy dog");
    }

    #[test]
    fn test_request_id_increments() {
        let handler = sync_handler(Response::ok);
        let client = TestClient::new(handler);

        let r1 = client.get("/").send();
        let r2 = client.get("/").send();
        let r3 = client.get("/").send();

        assert!(r2.request_id() > r1.request_id());
        assert!(r3.request_id() > r2.request_id());
    }

    #[test]
    fn test_client_with_seed_for_determinism() {
        let handler = sync_handler(Response::ok);

        // Same seed should work consistently
        let client1 = TestClient::with_seed(handler, 42);
        assert_eq!(client1.seed(), Some(42));
    }
}
