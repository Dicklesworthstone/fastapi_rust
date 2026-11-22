# Error Handling

Handle errors gracefully and return appropriate HTTP responses.

## HttpError

Use `HttpError` for errors with HTTP semantics:

```rust
use fastapi::core::HttpError;

// Create an HTTP error
let error = HttpError::not_found("User not found");
let error = HttpError::bad_request("Invalid input");
let error = HttpError::unauthorized("Authentication required");
let error = HttpError::forbidden("Access denied");
let error = HttpError::internal("Something went wrong");
```

## Exception Handlers

Register per-`App` custom exception handlers on `AppBuilder`:

```rust
use fastapi_rust::prelude::*;
use fastapi_rust::ResponseBody;

let app = App::builder()
    .exception_handler::<MyCustomError, _>(|_ctx, err| {
        Response::with_status(StatusCode::BAD_REQUEST)
            .body(ResponseBody::Bytes(format!("Custom error: {err}").into_bytes()))
    })
    .route_entry(my_handler_route())
    .get("/fluent", |_ctx, _req| async {
        Err::<&'static str, MyCustomError>(MyCustomError::from("invalid"))
    })
    .build();
```

### Dispatch and Fallback Rules

- **Route `Result` errors and extractor rejections**: Macro routes (`#[get]`, `#[post]`, etc.) and fluent routes (`AppBuilder::get`/`post`/...) convert `Result<T, E>` errors (including type aliases and `-> impl IntoResponse`) and `FromRequest::Error` rejections by consulting the active `App`'s `ExceptionHandlers` registry before response middleware `after()` hooks and dependency cleanups run.
- **Exact-type lookup**: Lookup matches the concrete Rust `TypeId` of the error (for extractors, the outer `FromRequest::Error` rejection type such as `PathExtractError` or `JsonExtractError`, rather than inferred HTTP status codes or inner `ValidationErrors`).
- **Fallback and boundaries**: Unregistered errors (including response-valued errors that do not implement `std::error::Error` or `Sync`) fall back directly to their `IntoResponse::into_response` implementation. Already-converted `Response` values and successful responses are never reinterpreted, and `Option<T>` extractors continue to swallow extraction failures as `None`. Registries are scoped per `App` with no global cross-talk.

## Error Response Pattern

Create consistent error responses:

```rust
use serde::Serialize;

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    code: u32,
    details: Option<String>,
}

fn error_response(status: StatusCode, code: u32, message: &str) -> Response {
    let body = ErrorResponse {
        error: message.into(),
        code,
        details: None,
    };

    let json = serde_json::to_vec(&body).unwrap();

    Response::with_status(status)
        .header("Content-Type", "application/json")
        .body(ResponseBody::Bytes(json))
}
```

## Validation Errors

Use `ValidationError` for input validation:

```rust
use fastapi::core::{ValidationError, ValidationErrors};

let mut errors = ValidationErrors::new();
errors.add("email", ValidationError::new("invalid_format", "Invalid email format"));
errors.add("password", ValidationError::new("too_short", "Password must be at least 8 characters"));

if !errors.is_empty() {
    // Return 400 Bad Request with errors
}
```

## Middleware Error Handling

Middleware can catch and transform errors:

```rust
impl Middleware for ErrorHandlerMiddleware {
    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        Box::pin(async move {
            if response.status().is_server_error() {
                // Log error, maybe transform response
                eprintln!("Server error: {}", response.status());
            }
            response
        })
    }
}
```

## Best Practices

### Use Specific Error Types

```rust
// Good: Specific error
HttpError::not_found("User with ID 123 not found")

// Avoid: Generic error
HttpError::internal("Error occurred")
```

### Include Helpful Details

```rust
// In development
let error = HttpError::bad_request("Invalid JSON at line 5, column 10");

// In production (hide details)
let error = HttpError::bad_request("Invalid request format");
```

### Log Server Errors

```rust
fn handler(...) -> Response {
    match do_something() {
        Ok(result) => Response::ok(),
        Err(e) => {
            eprintln!("Error in handler: {:?}", e);
            HttpError::internal("Internal error").into_response()
        }
    }
}
```

## Next Steps

- [Middleware](middleware.md) - Error handling middleware
- [Testing](testing.md) - Test error scenarios
