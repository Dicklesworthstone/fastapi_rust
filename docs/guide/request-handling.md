# Request Handling

> **Status**: This chapter covers features that are partially implemented. Some extractors are available, with more coming soon.

## Request Object

Access request data through the `Request` type:

```rust
use fastapi::core::{Request, RequestContext, Response};

fn handler(_ctx: &RequestContext, req: &mut Request) -> std::future::Ready<Response> {
    // Get HTTP method
    let method = req.method();

    // Get path
    let path = req.path();

    // Get headers
    if let Some(auth) = req.header("Authorization") {
        // auth is &[u8]
    }

    std::future::ready(Response::ok())
}
```

## Available Now

### Headers

```rust
// Get a single header
let content_type = req.header("Content-Type");

// Get all headers
let headers = req.headers();
```

### Method and Path

```rust
let method = req.method();  // Method enum
let path = req.path();      // &str
```

## Coming Soon

The following extractors are planned:

- **Path Parameters**: Extract typed parameters from URL paths
- **Query Parameters**: Parse query strings into typed structs
- **JSON Body**: Deserialize JSON request bodies
- **Form Data**: Handle form submissions
- **Multipart**: File uploads

Example of planned API:

```rust
// Planned syntax (not yet implemented)
fn get_user(
    ctx: &RequestContext,
    id: Path<i64>,           // Extract from /users/{id}
    query: Query<Pagination>, // Extract from ?page=1&limit=10
) -> Json<User> {
    // ...
}
```

## Next Steps

- [Response Building](response-building.md) - Creating responses
- [Middleware](middleware.md) - Processing requests before handlers
