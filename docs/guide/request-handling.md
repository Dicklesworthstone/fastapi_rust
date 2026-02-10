# Request Handling

> **Status (as of 2026-02-10)**: This chapter documents the request API and the extractor surface that exists today (Path, Query, Json, headers, cookies, auth).

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

## Core Request API

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

## Extractors

fastapi_rust supports typed extraction in handler signatures. Examples:

```rust
use fastapi::prelude::*;

#[derive(Deserialize)]
struct Pagination {
    page: Option<u32>,
    limit: Option<u32>,
}

#[get("/users/{id}")]
async fn get_user(_cx: &Cx, id: Path<i64>, q: Query<Pagination>) -> StatusCode {
    let _id: i64 = id.0;
    let _page: Option<u32> = q.page;
    let _limit: Option<u32> = q.limit;
    StatusCode::OK
}
```

## Next Steps

- [Response Building](response-building.md) - Creating responses
- [Middleware](middleware.md) - Processing requests before handlers
