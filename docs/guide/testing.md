# Testing

fastapi_rust includes a powerful `TestClient` for testing your applications without running an HTTP server.

## TestClient Basics

Create a test client wrapping your app or handler:

```rust
use fastapi::core::{App, TestClient};

#[test]
fn test_app() {
    let app = App::builder()
        .get("/", hello_handler)
        .build();

    let client = TestClient::new(app);
    let response = client.get("/").send();

    assert_eq!(response.status().as_u16(), 200);
}
```

## Making Requests

### GET Requests

```rust
// Simple GET
let response = client.get("/users").send();

// GET with query parameters
let response = client.get("/users?page=2&limit=10").send();

// GET with headers
let response = client
    .get("/protected")
    .header("Authorization", "Bearer token123")
    .send();
```

### POST Requests

```rust
// POST with body
let response = client
    .post("/users")
    .body(b"{\"name\":\"Alice\"}".to_vec())
    .send();

// POST with JSON
let response = client
    .post("/users")
    .header("Content-Type", "application/json")
    .body(b"{\"name\":\"Bob\"}".to_vec())
    .send();
```

### Other Methods

```rust
// PUT
let response = client.put("/users/1").body(data).send();

// DELETE
let response = client.delete("/users/1").send();

// PATCH
let response = client.patch("/users/1").body(patch_data).send();
```

## Assertions

### Status Code

```rust
let response = client.get("/").send();

// Check status code
assert_eq!(response.status().as_u16(), 200);
assert!(response.status().is_success());

// Check specific status
assert_eq!(response.status(), StatusCode::OK);
assert_eq!(response.status(), StatusCode::NOT_FOUND);
```

### Response Body

```rust
let response = client.get("/hello").send();

// Get body as string
assert_eq!(response.text(), "Hello, World!");

// Get body as bytes
assert_eq!(response.bytes(), b"Hello, World!");

// Check body contains substring
assert!(response.text().contains("Hello"));
```

### Headers

```rust
let response = client.get("/").send();

// Check header exists and value
// Note: Use the appropriate method based on your test requirements
let headers = response.headers();
// Iterate or check specific headers
```

## Test Organization

### Unit Tests

Test individual handlers:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_handler() {
        let app = App::builder()
            .get("/", hello)
            .build();

        let client = TestClient::new(app);
        let response = client.get("/").send();

        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.text(), "Hello, World!");
    }
}
```

### Integration Tests

Create `tests/` directory for integration tests:

```rust
// tests/api_test.rs
use fastapi::core::{App, TestClient};
use my_api::{create_app};

#[test]
fn test_full_api() {
    let app = create_app();
    let client = TestClient::new(app);

    // Test user flow
    let response = client.get("/health").send();
    assert_eq!(response.status().as_u16(), 200);
}
```

### Test Fixtures

Share setup code across tests:

```rust
fn create_test_client() -> TestClient<App> {
    let app = App::builder()
        .get("/", hello)
        .get("/health", health)
        .middleware(RequestIdMiddleware::new())
        .build();

    TestClient::new(app)
}

#[test]
fn test_hello() {
    let client = create_test_client();
    let response = client.get("/").send();
    assert_eq!(response.status().as_u16(), 200);
}

#[test]
fn test_health() {
    let client = create_test_client();
    let response = client.get("/health").send();
    assert_eq!(response.status().as_u16(), 200);
}
```

## Dependency Overrides

Override dependencies for testing:

```rust
use fastapi::core::{DependencyOverrides, TestClient};

#[test]
fn test_with_mock_database() {
    let mut overrides = DependencyOverrides::new();
    overrides.set::<DatabasePool>(MockDatabasePool::new());

    let app = App::builder()
        .dependency_overrides(overrides)
        .get("/users", list_users)
        .build();

    let client = TestClient::new(app);
    // Tests use MockDatabasePool instead of real database
}
```

## Deterministic Testing

Use `with_seed` for reproducible tests involving randomness:

```rust
let client = TestClient::with_seed(app, 12345);

// Tests using this client will have deterministic random values
// Useful for request ID generation, etc.
```

## Cookie Handling

TestClient maintains cookies across requests:

```rust
// First request sets a cookie
let response = client.get("/login").send();

// Subsequent requests include the cookie automatically
let response = client.get("/protected").send();
// Cookie from /login is sent
```

Access the cookie jar:

```rust
let cookies = client.cookies();
// Inspect or modify cookies
```

## Testing Patterns

### Testing Error Responses

```rust
#[test]
fn test_not_found() {
    let client = create_test_client();

    let response = client.get("/nonexistent").send();
    assert_eq!(response.status().as_u16(), 404);
}

#[test]
fn test_method_not_allowed() {
    let client = create_test_client();

    // Assuming only GET is defined for /
    let response = client.post("/").send();
    assert_eq!(response.status().as_u16(), 405);
}
```

### Testing with State

```rust
struct Counter {
    value: std::sync::atomic::AtomicU64,
}

#[test]
fn test_counter_state() {
    let app = App::builder()
        .state(Counter { value: AtomicU64::new(0) })
        .get("/count", get_count)
        .post("/increment", increment)
        .build();

    let client = TestClient::new(app);

    // Initial count
    let response = client.get("/count").send();
    assert_eq!(response.text(), "0");

    // Increment
    client.post("/increment").send();

    // Verify increment
    let response = client.get("/count").send();
    assert_eq!(response.text(), "1");
}
```

### Testing Middleware

```rust
#[test]
fn test_security_headers_added() {
    let app = App::builder()
        .middleware(SecurityHeaders::new())
        .get("/", hello)
        .build();

    let client = TestClient::new(app);
    let response = client.get("/").send();

    // Verify security headers are present
    // (check headers collection for X-Content-Type-Options, etc.)
}
```

## Best Practices

### Test One Thing Per Test

```rust
// Good: Single assertion focus
#[test]
fn test_hello_returns_200() {
    let response = client.get("/").send();
    assert_eq!(response.status().as_u16(), 200);
}

#[test]
fn test_hello_returns_greeting() {
    let response = client.get("/").send();
    assert_eq!(response.text(), "Hello, World!");
}

// Avoid: Testing too many things
#[test]
fn test_hello_everything() {
    let response = client.get("/").send();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.text(), "Hello, World!");
    // ... many more assertions
}
```

### Descriptive Test Names

```rust
#[test]
fn get_user_with_valid_id_returns_user() { }

#[test]
fn get_user_with_invalid_id_returns_404() { }

#[test]
fn create_user_with_missing_name_returns_400() { }
```

### Test Edge Cases

```rust
#[test]
fn test_empty_path() {
    let response = client.get("").send();
    // Define expected behavior
}

#[test]
fn test_unicode_in_path() {
    let response = client.get("/users/??????").send();
    // Define expected behavior
}
```

## Running Tests

```bash
# Run all tests
cargo test

# Run tests for specific crate
cargo test -p my_api

# Run specific test
cargo test test_hello

# Run with output
cargo test -- --nocapture
```

## Next Steps

- [Error Handling](error-handling.md) - Test error scenarios
- [Middleware](middleware.md) - Test middleware behavior
