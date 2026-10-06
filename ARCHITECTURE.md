# fastapi_rust Architecture

> Technical architecture guide for developers working on or integrating with fastapi_rust.

---

## Overview

fastapi_rust is an ultra-optimized Rust web framework inspired by FastAPI's developer experience. The architecture prioritizes:

1. **Zero-copy parsing** - Parse directly from request buffers without allocations
2. **Compile-time validation** - Route and type errors caught at build time via proc macros
3. **Structured concurrency** - Cancel-correct request handling via asupersync
4. **Minimal dependencies** - Only asupersync + serde (3 crates vs 80+ for alternatives)
5. **FastAPI compatibility** - Familiar API patterns and compatible error formats

---

## Crate Structure

```
fastapi_rust/
├── crates/
│   ├── fastapi/           # Facade crate - re-exports all public APIs
│   ├── fastapi-core/      # Core types, extractors, middleware, DI, testing
│   ├── fastapi-http/      # Zero-copy HTTP/1.1 parser and server
│   ├── fastapi-router/    # Radix trie router with path matching
│   ├── fastapi-macros/    # Procedural macros (#[get], #[derive(Validate)])
│   ├── fastapi-openapi/   # OpenAPI 3.1 spec generation
│   └── fastapi-output/    # Rich terminal output and theming
```

### Crate Dependency Graph

```
                        fastapi (facade)
                              │
       ┌──────────────────────┼──────────────────────┐
       │                      │                      │
       ▼                      ▼                      ▼
  fastapi-http         fastapi-core           fastapi-router
       │                      │                      │
       │                      ├──────────────────────┤
       │                      │                      │
       ▼                      ▼                      ▼
  fastapi-openapi      fastapi-macros         fastapi-output
       │                      │
       └──────────────────────┘
                 │
                 ▼
            asupersync (runtime)
```

---

## Crate Details

### `fastapi` (Facade)

**Purpose:** Single entry point combining all sub-crates.

**Key exports:**
- `fastapi::prelude::*` - Common imports
- `fastapi::core::*` - Core types
- `fastapi::extractors::*` - All extractors
- `fastapi::testing::*` - Test utilities

**Lines:** ~270

---

### `fastapi-core` (Core Framework)

**Purpose:** Fundamental framework building blocks.

**Key modules:**

| Module | Purpose | Key Types |
|--------|---------|-----------|
| `extract.rs` | Request data extraction | `Json<T>`, `Path<T>`, `Query<T>`, `Header<T>`, `BearerToken`, 25+ extractors |
| `middleware.rs` | Request/response processing | `Middleware`, `Cors`, `RateLimitMiddleware`, `SecurityHeaders` |
| `dependency.rs` | Dependency injection | `Depends<T>`, `DependsCleanup<T>`, `DependencyCache` |
| `context.rs` | Request context | `RequestContext`, wraps asupersync's `Cx` |
| `request.rs` | HTTP request types | `Request`, `Method`, `Headers`, `Body` |
| `response.rs` | HTTP response types | `Response`, `ResponseBody`, `StatusCode`, `IntoResponse` |
| `app.rs` | Application builder | `App`, `AppBuilder`, `AppConfig` |
| `testing.rs` | Test infrastructure | `TestClient`, `TestResponse`, assertion macros |
| `health.rs` | Health checks | `HealthCheckRegistry`, `HealthStatus` |
| `shutdown.rs` | Graceful shutdown | `GracefulShutdown`, `ShutdownController` |
| `error.rs` | Error handling | `HttpError`, `ValidationError` |

**Lines:** ~50,000+

---

### `fastapi-http` (HTTP Parser & Server)

**Purpose:** Zero-copy HTTP/1.1 implementation.

**Key components:**

| Component | Purpose |
|-----------|---------|
| `parser.rs` | Zero-allocation request line and header parsing |
| `body.rs` | Content-Length and chunked transfer encoding |
| `query.rs` | Query string parsing with percent-decoding |
| `response.rs` | Response writing with chunked encoding |
| `server.rs` | TCP server with connection management |
| `streaming.rs` | Streaming response bodies |
| `multipart.rs` | Multipart form data parsing |
| `range.rs` | HTTP Range request support |

**Zero-Copy Design:**
```rust
// Borrowed types reference the original buffer
pub struct RequestLine<'a> {
    method: &'a [u8],
    path: &'a [u8],
    query: Option<&'a [u8]>,
}
```

---

### `fastapi-router` (Radix Trie Router)

**Purpose:** High-performance path routing with parameter extraction.

**Key types:**
- `Router` - Main router type
- `Route` - Single route definition
- `RouteMatch` - Matched route with extracted params
- `Converter` - Path parameter type converters (`Int`, `Uuid`, etc.)

**Trie structure:**
```
          /
         /|\
        / | \
       /  |  \
    api users items
     |     |     |
   v1    {id}  {id}
```

---

### `fastapi-macros` (Procedural Macros)

**Purpose:** Compile-time code generation.

**Macros:**
- `#[get("/path")]`, `#[post("/path")]`, etc. - Route handlers
- `#[derive(Validate)]` - Validation code generation
- `#[derive(JsonSchema)]` - OpenAPI schema generation

---

### `fastapi-openapi` (OpenAPI 3.1)

**Purpose:** Specification generation from types.

**Key types:**
- `OpenApi` - Full OpenAPI document
- `OpenApiBuilder` - Fluent builder
- `Schema` - JSON Schema types
- `SchemaRegistry` - Type -> Schema mapping

---

### `fastapi-output` (Terminal Output)

**Purpose:** Rich terminal formatting for CLI tools.

**Key types:**
- `FastApiTheme` - Color and style configuration
- `ThemePreset` - Predefined themes (fastapi, neon, minimal, monokai)
- `BoxStyle` - Border drawing characters

---

## Request Lifecycle

```
 ┌─────────────────────────────────────────────────────────────────┐
 │                         TCP Connection                          │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  1. Parse HTTP Request (fastapi-http)                           │
 │     - Zero-copy request line parsing                            │
 │     - Header parsing without allocation                         │
 │     - Body handling (Content-Length or chunked)                 │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  2. Route Matching (fastapi-router)                             │
 │     - Radix trie lookup O(k) where k = path length              │
 │     - Path parameter extraction                                 │
 │     - Method matching                                           │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  3. Create Request Context (fastapi-core)                       │
 │     - Wrap asupersync Cx for structured concurrency             │
 │     - Initialize DependencyCache                                │
 │     - Set up CleanupStack                                       │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  4. Middleware.before() (registration order)                    │
 │     - RequestIdMiddleware: Generate request ID                  │
 │     - Cors: Handle preflight and headers                        │
 │     - RateLimitMiddleware: Check rate limits                    │
 │     - SecurityHeaders: Add security headers                     │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  5. Dependency Resolution (fastapi-core)                        │
 │     - Resolve Depends<T> extractors                             │
 │     - Request-scoped caching                                    │
 │     - Circular dependency detection                             │
 │     - Register cleanup functions                                │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  6. Extractor Chain (fastapi-core)                              │
 │     - Path<T>: Extract and deserialize path params              │
 │     - Query<T>: Parse query string                              │
 │     - Json<T>: Deserialize request body                         │
 │     - Header<T>: Extract typed headers                          │
 │     - BearerToken: Auth extraction                              │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  7. Handler Execution                                           │
 │     - Run async handler with extracted values                   │
 │     - cx.checkpoint() for cancellation points                   │
 │     - Return Response or HttpError                              │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  8. Middleware.after() (reverse order)                          │
 │     - Add response headers                                      │
 │     - Log request/response                                      │
 │     - Compress response body                                    │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  9. Send Response (fastapi-http)                                │
 │     - Write status line and headers                             │
 │     - Stream body (chunked if needed)                            │
 └─────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
 ┌─────────────────────────────────────────────────────────────────┐
 │  10. Finalize request                                          │
 │      - Background tasks after successful response write          │
 │      - Dependency cleanup stack in LIFO order                    │
 │      - Cleanup also attempted after HTTP write errors             │
 └─────────────────────────────────────────────────────────────────┘
```

---

## Key Design Decisions

The server owns finalization after response transmission, preserving dependency
resources while streamed bodies and background tasks use them. HTTP/1 handler
timeouts retain already registered callbacks after abandoning the handler.
Completed WebSocket handlers finalize their separate context. Test helpers collect
finite streams first. Direct `App::handle` callers must finalize their own context.
Handler, stream, background-task, or destructor panics and dropping the complete
request future can bypass async teardown; callbacks have no automatic timeout.

### 1. Zero-Copy HTTP Parsing

**Problem:** Traditional parsers allocate strings for each header, wasting memory and CPU.

**Solution:** Borrowed types reference the original request buffer:
```rust
// No allocation - just points into the buffer
pub struct Headers<'a> {
    data: &'a [u8],
    indices: SmallVec<[(usize, usize, usize, usize); 16]>,
}
```

### 2. Structured Concurrency with asupersync

**Problem:** Tokio's spawn model can leak tasks and resources.

**Solution:** All request work runs in a region that ensures cleanup:
```
Request Region
├── Handler Task
├── Dependency Tasks
└── Background Tasks
    └── Region close waits for ALL tasks
```

Client disconnect → Cancel region → All tasks cleaned up.

### 3. Compile-Time Route Validation

**Problem:** Runtime route conflicts are hard to debug.

**Solution:** Proc macros validate at compile time:
```rust
#[get("/users/{id}")]  // Validates {id} extractor exists
async fn get_user(id: Path<i64>) -> Json<User> { ... }
```

### 4. Type-Driven Extraction

**Problem:** Manual parameter parsing is error-prone and verbose.

**Solution:** Declare types, framework extracts automatically:
```rust
async fn handler(
    id: Path<i64>,           // From URL path
    q: Query<SearchParams>,   // From query string
    auth: BearerToken,        // From Authorization header
) -> Result<Json<Data>, HttpError>
```

### 5. FastAPI-Compatible Errors

**Problem:** Different error formats break client code.

**Solution:** Match FastAPI's 422 format exactly:
```json
{
  "detail": [
    {"type": "missing", "loc": ["body", "email"], "msg": "Field required"}
  ]
}
```

---

## Dependency Injection

### Resolution Flow

```
Depends<T> extraction
        │
        ▼
┌───────────────────┐
│ Check Overrides   │ ─── Found ──► Return override value
└───────────────────┘
        │ Not found
        ▼
┌───────────────────┐
│ Check Cache       │ ─── Found ──► Return cached value
└───────────────────┘
        │ Not found
        ▼
┌───────────────────┐
│ Check Cycle       │ ─── Cycle ──► Return CircularDependencyError
└───────────────────┘
        │ No cycle
        ▼
┌───────────────────┐
│ Push to Stack     │
└───────────────────┘
        │
        ▼
┌───────────────────┐
│ Resolve T         │ ─── May recursively resolve dependencies
└───────────────────┘
        │
        ▼
┌───────────────────┐
│ Pop from Stack    │
│ Cache if Request  │
│ Register Cleanup  │
└───────────────────┘
        │
        ▼
    Return T
```

### Scopes

| Scope | Behavior |
|-------|----------|
| `Request` (default) | Cache for request lifetime |
| `Function` | Resolve on every extraction |

---

## Middleware Architecture

### Onion Model

```
Request ───► MW1.before ───► MW2.before ───► MW3.before ───► Handler
                                                                │
Response ◄── MW1.after ◄─── MW2.after ◄─── MW3.after ◄────────┘
```

First registered runs first on the way in, last on the way out.

### Control Flow

```rust
pub enum ControlFlow {
    Continue,           // Proceed to next middleware/handler
    Break(Response),    // Short-circuit with response
}
```

### Built-in Middleware

| Middleware | Purpose |
|------------|---------|
| `RequestIdMiddleware` | Add X-Request-Id header |
| `Cors` | CORS preflight and headers |
| `RateLimitMiddleware` | Token bucket/sliding window rate limiting |
| `SecurityHeaders` | HSTS, CSP, X-Frame-Options |
| `CsrfMiddleware` | CSRF token validation |
| `CompressionMiddleware` | gzip/deflate response compression |

---

## Testing Infrastructure

### TestClient

In-process testing without network I/O:

```rust
let client = TestClient::new(app);
let response = client.get("/users/1")
    .header("Authorization", "Bearer token")
    .send();

assert_eq!(response.status().as_u16(), 200);
```

### Assertion Macros

```rust
assert_status!(response, 200);
assert_header!(response, "Content-Type", "application/json");
assert_json!(response, {"id": 42, "name": "Widget"});
```

### Dependency Overrides

```rust
let overrides = DependencyOverrides::new()
    .with::<Database>(MockDatabase::new());

let client = TestClient::new(app)
    .with_overrides(overrides);
```

---

## Extension Points

### Custom Extractors

Implement `FromRequest`:

```rust
impl FromRequest for MyExtractor {
    type Error = HttpError;

    async fn from_request(
        ctx: &RequestContext,
        req: &mut Request,
    ) -> Result<Self, Self::Error> {
        // Custom extraction logic
    }
}
```

### Custom Middleware

Implement `Middleware`:

```rust
impl Middleware for MyMiddleware {
    async fn before(&self, ctx: &RequestContext, req: &mut Request) -> ControlFlow {
        // Pre-handler logic
        ControlFlow::Continue
    }

    async fn after(&self, ctx: &RequestContext, req: &Request, resp: Response) -> Response {
        // Post-handler logic
        resp
    }
}
```

### Custom Dependencies

Implement `FromDependency`:

```rust
impl FromDependency for MyService {
    type Error = HttpError;

    async fn from_dependency(
        ctx: &RequestContext,
        req: &mut Request,
    ) -> Result<Self, Self::Error> {
        // Resolve and return service
    }
}
```

---

## Performance Considerations

### Hot Path Optimizations

| Technique | Benefit |
|-----------|---------|
| Zero-copy parsing | No allocations for headers/path |
| Monomorphization | No trait objects on hot paths |
| Pre-allocated buffers | 4KB default per request |
| Inline critical paths | `#[inline(always)]` on hot code |
| Static route optimization | Skip trie for exact matches |

### Memory Layout

- Request buffers: 4KB default, configurable
- Header indices: SmallVec with inline storage for 16 headers
- Response bodies: Enum variants avoid Box for common cases

---

## Integration with asupersync

### Key Concepts

| asupersync | fastapi_rust |
|------------|--------------|
| `Cx` (capability context) | `RequestContext` |
| Region | Request lifetime |
| Budget | Request timeout |
| Checkpoint | Cancellation points |

### Cancel-Correctness

```rust
async fn handler(cx: &Cx) -> Response {
    cx.checkpoint()?;  // Yield point - can be cancelled here

    // Long operation with periodic checkpoints
    for item in items {
        process(item).await;
        cx.checkpoint()?;
    }

    Response::ok()
}
```

---

## File Organization

```
fastapi_rust/
├── AGENTS.md               # AI agent guidelines
├── ARCHITECTURE.md         # This file
├── Cargo.toml              # Workspace configuration
├── crates/
│   ├── fastapi/
│   │   ├── src/lib.rs      # Re-exports
│   │   └── examples/       # Example applications
│   ├── fastapi-core/
│   │   └── src/
│   │       ├── lib.rs      # Module exports
│   │       ├── extract.rs  # All extractors (largest file)
│   │       ├── middleware.rs
│   │       ├── dependency.rs
│   │       └── ...
│   ├── fastapi-http/
│   │   └── src/
│   │       ├── parser.rs   # Zero-copy HTTP parsing
│   │       ├── server.rs   # TCP server
│   │       └── ...
│   └── ...
└── legacy_fastapi/          # Python FastAPI (reference only)
```

---

## Related Documentation

- [AGENTS.md](AGENTS.md) - Guidelines for AI coding agents
- [README.md](README.md) - Project overview and quick start
- [asupersync](https://github.com/Dicklesworthstone/asupersync) - Async runtime documentation
