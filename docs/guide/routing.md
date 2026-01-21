# Routing

Routing connects URL paths to handler functions. This chapter covers how to define routes and organize your API structure.

## Basic Routing

Register routes using the fluent builder API:

```rust
use fastapi::core::{App, Request, RequestContext, Response, ResponseBody};

fn index(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    std::future::ready(Response::ok().body(ResponseBody::Bytes(b"Home".to_vec())))
}

fn about(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    std::future::ready(Response::ok().body(ResponseBody::Bytes(b"About".to_vec())))
}

let app = App::builder()
    .get("/", index)
    .get("/about", about)
    .build();
```

## HTTP Methods

fastapi_rust supports all standard HTTP methods:

```rust
let app = App::builder()
    .get("/items", list_items)       // Read collection
    .post("/items", create_item)     // Create new item
    .put("/items", replace_items)    // Replace collection
    .delete("/items", delete_items)  // Delete collection
    .patch("/items", update_items)   // Partial update
    .build();
```

### Method Semantics

| Method | Typical Use |
|--------|-------------|
| GET | Retrieve data (idempotent, safe) |
| POST | Create new resource |
| PUT | Replace resource entirely |
| PATCH | Partial resource update |
| DELETE | Remove resource |

## Handler Functions

Handlers are functions that process requests and return responses:

```rust
use fastapi::core::{Request, RequestContext, Response, ResponseBody};

// Sync handler (returns Ready future)
fn sync_handler(
    _ctx: &RequestContext,
    _req: &mut Request
) -> std::future::Ready<Response> {
    std::future::ready(
        Response::ok().body(ResponseBody::Bytes(b"Hello".to_vec()))
    )
}
```

### Handler Parameters

| Parameter | Type | Purpose |
|-----------|------|---------|
| `ctx` | `&RequestContext` | Request context with ID, timing, etc. |
| `req` | `&mut Request` | The HTTP request with headers, body, etc. |

### Return Type

Handlers must return a type that implements `Future<Output = Response>`. The simplest approach is `std::future::Ready<Response>`:

```rust
std::future::ready(Response::ok())
```

## Response Types

Create responses with the fluent builder:

```rust
// 200 OK
Response::ok()

// 200 OK with body
Response::ok().body(ResponseBody::Bytes(b"content".to_vec()))

// 404 Not Found
Response::with_status(StatusCode::NOT_FOUND)

// 500 Internal Server Error with body
Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
    .body(ResponseBody::Bytes(b"error".to_vec()))
```

## Route Organization with APIRouter

Group related routes using `APIRouter`:

```rust
use fastapi::core::{App, APIRouter, Request, RequestContext, Response};

// Create a router for user-related endpoints
let users_router = APIRouter::new()
    .prefix("/users")
    .get("", list_users)       // GET /users
    .post("", create_user);    // POST /users

// Create a router for item-related endpoints
let items_router = APIRouter::new()
    .prefix("/items")
    .get("", list_items)       // GET /items
    .post("", create_item);    // POST /items

// Include both routers in the app
let app = App::builder()
    .include_router(users_router)
    .include_router(items_router)
    .build();
```

### Router Configuration

Configure routers with metadata:

```rust
let api_router = APIRouter::new()
    .prefix("/api/v1")
    .tags(vec!["api".into()])
    .deprecated(false);
```

### Nested Routers

Compose routers by nesting:

```rust
let admin_users = APIRouter::new()
    .prefix("/users")
    .get("", admin_list_users);

let admin_router = APIRouter::new()
    .prefix("/admin")
    .include_router(admin_users);  // /admin/users

let app = App::builder()
    .include_router(admin_router)
    .build();
```

## Common Patterns

### RESTful Resource

```rust
let users = APIRouter::new()
    .prefix("/users")
    .get("", list_users)           // GET /users
    .post("", create_user);        // POST /users
    // Path parameters coming soon:
    // .get("/{id}", get_user)     // GET /users/{id}
    // .put("/{id}", update_user)  // PUT /users/{id}
    // .delete("/{id}", delete_user)  // DELETE /users/{id}
```

### API Versioning

```rust
let v1 = APIRouter::new()
    .prefix("/api/v1")
    .get("/status", v1_status);

let v2 = APIRouter::new()
    .prefix("/api/v2")
    .get("/status", v2_status);

let app = App::builder()
    .include_router(v1)
    .include_router(v2)
    .build();
```

## Pitfalls to Avoid

### Conflicting Routes

Routes with the same method and path will cause issues:

```rust
// Bad: conflicting routes
let app = App::builder()
    .get("/users", handler_a)
    .get("/users", handler_b)  // Which one runs?
    .build();
```

### Missing Leading Slash

Always include the leading slash:

```rust
// Good
.get("/users", handler)

// Bad - may not match
.get("users", handler)
```

## Current Limitations

> **Note**: The following features are implemented in the router but not yet integrated with the App:

- **Path Parameters**: `/users/{id}` syntax is recognized but parameters aren't extracted to handlers yet
- **Path Converters**: `{id:int}`, `{id:uuid}` type validation exists but isn't exposed

These features are coming in a future release.

## Next Steps

- [Request Handling](request-handling.md) - Learn about processing request data
- [Middleware](middleware.md) - Add cross-cutting concerns to your routes
