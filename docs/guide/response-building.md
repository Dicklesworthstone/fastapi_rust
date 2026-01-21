# Response Building

Create HTTP responses with the fluent `Response` API.

## Basic Responses

### Status-Only Responses

```rust
use fastapi::core::{Response, StatusCode};

// 200 OK
Response::ok()

// Other status codes
Response::with_status(StatusCode::NOT_FOUND)
Response::with_status(StatusCode::CREATED)
Response::with_status(StatusCode::BAD_REQUEST)
Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
```

### Responses with Body

```rust
use fastapi::core::{Response, ResponseBody};

// Text body
Response::ok().body(ResponseBody::Bytes(b"Hello, World!".to_vec()))

// JSON body
Response::ok().body(ResponseBody::Bytes(
    b"{\"message\":\"success\"}".to_vec()
))
```

## Headers

Add headers to responses:

```rust
let response = Response::ok()
    .header("Content-Type", "application/json")
    .header("X-Custom-Header", "value")
    .body(ResponseBody::Bytes(data));
```

## Status Codes

Common status codes:

| Code | Constant | Meaning |
|------|----------|---------|
| 200 | `StatusCode::OK` | Success |
| 201 | `StatusCode::CREATED` | Resource created |
| 204 | `StatusCode::NO_CONTENT` | Success, no body |
| 400 | `StatusCode::BAD_REQUEST` | Invalid request |
| 401 | `StatusCode::UNAUTHORIZED` | Auth required |
| 403 | `StatusCode::FORBIDDEN` | Not allowed |
| 404 | `StatusCode::NOT_FOUND` | Resource not found |
| 405 | `StatusCode::METHOD_NOT_ALLOWED` | Wrong HTTP method |
| 500 | `StatusCode::INTERNAL_SERVER_ERROR` | Server error |

## Convenience Methods

```rust
// Redirect
Response::redirect("/new-location")

// No content (204)
Response::no_content()
```

## Response Patterns

### JSON Response

```rust
fn json_handler(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    let json = serde_json::json!({
        "status": "ok",
        "data": { "id": 1 }
    }).to_string();

    std::future::ready(
        Response::ok()
            .header("Content-Type", "application/json")
            .body(ResponseBody::Bytes(json.into_bytes()))
    )
}
```

### Error Response

```rust
fn error_response(status: StatusCode, message: &str) -> Response {
    let json = serde_json::json!({
        "error": message
    }).to_string();

    Response::with_status(status)
        .header("Content-Type", "application/json")
        .body(ResponseBody::Bytes(json.into_bytes()))
}

// Usage
error_response(StatusCode::NOT_FOUND, "User not found")
error_response(StatusCode::BAD_REQUEST, "Invalid input")
```

## Coming Soon

The following response helpers are planned:

- **Json<T>**: Automatic JSON serialization
- **Html**: HTML responses with proper content type
- **FileResponse**: Serve files
- **Streaming**: Streaming response bodies

## Next Steps

- [Error Handling](error-handling.md) - Structured error responses
- [Testing](testing.md) - Verify response content
