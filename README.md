# fastapi_rust

<div align="center">
  <img src="fastapi_rust_illustration.webp" alt="fastapi_rust - High-performance Rust web framework with FastAPI-inspired ergonomics">
</div>

<div align="center">

**High-performance Rust web framework with FastAPI-inspired ergonomics**

*A Rust port inspired by [tiangolo/fastapi](https://github.com/tiangolo/fastapi) (Python), extended with [asupersync](https://github.com/Dicklesworthstone/asupersync) for structured concurrency, zero-copy parsing, and deterministic testing.*

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-early%20development-yellow.svg)]()

*Type-safe routing | Zero-copy parsing | Structured concurrency | OpenAPI generation*

</div>

<div align="center">
<h3>Quick Install</h3>

```bash
curl -fsSL "https://raw.githubusercontent.com/Dicklesworthstone/fastapi_rust/main/install.sh?$(date +%s)" | bash
```

**Or add to your project:**

```toml
# Cargo.toml
[dependencies]
fastapi = { git = "https://github.com/Dicklesworthstone/fastapi_rust.git" }
asupersync = { git = "https://github.com/Dicklesworthstone/asupersync.git" }
serde = { version = "1", features = ["derive"] }
```

<p><em>Requires Rust 1.85+ (2024 edition). Co-developed with <a href="https://github.com/Dicklesworthstone/asupersync">asupersync</a>.</em></p>
</div>

---

## TL;DR

**The Problem**: Rust web frameworks either sacrifice developer ergonomics for performance (raw `hyper`) or hide allocations behind layers of abstraction (Axum + Tower). None leverage structured concurrency for cancel-correct request handling, and most require a massive dependency tree.

**The Solution**: fastapi_rust brings FastAPI's intuitive, type-driven API design to Rust with zero-copy HTTP parsing, compile-time route validation, and first-class integration with [asupersync](https://github.com/Dicklesworthstone/asupersync) for structured concurrency and deterministic testing.

### Why fastapi_rust?

| Feature | What It Does |
|---------|--------------|
| **Zero-copy HTTP parsing** | Requests parsed directly from buffers; no allocations on fast paths |
| **Compile-time route validation** | Invalid routes fail at build time via proc macros, not at runtime |
| **Structured concurrency** | Request handlers run in regions; cancellation is automatic and correct |
| **Type-driven extractors** | Declare parameter types; framework extracts and validates automatically |
| **Minimal dependencies** | Only `asupersync` + `serde` - no Tokio, no Tower, no hidden layers |
| **Deterministic testing** | Lab runtime for reproducible concurrent request tests |
| **FastAPI-compatible errors** | Validation errors match FastAPI's JSON format exactly |

---

## Quick Example

```rust
use fastapi_rust::prelude::*;

#[derive(Serialize, Deserialize, JsonSchema)]
struct Item {
    id: i64,
    name: String,
    price: f64,
}

#[get("/items/{id}")]
async fn get_item(cx: &Cx, id: Path<i64>) -> Json<Item> {
    cx.checkpoint()?;  // Cancellation-safe yield point

    Json(Item {
        id: id.0,
        name: "Widget".into(),
        price: 29.99,
    })
}

#[post("/items")]
async fn create_item(cx: &Cx, item: Json<Item>) -> Response {
    // Automatic JSON deserialization with validation
    // Wrong Content-Type -> 415
    // Parse error -> 422 with detailed location
    // Payload too large -> 413
    Response::created().json(&item.0)
}

#[get("/search")]
async fn search(
    cx: &Cx,
    q: Query<SearchParams>,           // ?q=...&limit=...
    auth: Header<Option<Bearer>>,     // Optional auth header
) -> Result<Json<Results>, HttpError> {
    // All extraction happens automatically
    // Wrong types -> compile error
    // Missing required -> 422 response
}

fn main() {
    let app = App::builder()
        .title("My API")
        .version("1.0.0")
        .route_entry(get_item_route())
        .route_entry(create_item_route())
        .route_entry(search_route())
        .middleware(RequestIdMiddleware::new())
        .middleware(Cors::permissive())
        .build();

    // Run with asupersync
    let rt = asupersync::runtime::RuntimeBuilder::current_thread()
        .build()
        .expect("runtime must build");
    rt.block_on(async move {
        serve(app, "0.0.0.0:8000")
            .await
            .expect("server must start");
    });
}
```

---

## Design Philosophy

### 1. Extract Spec, Never Translate

We study FastAPI's behavior and ergonomics, then implement idiomatically in Rust. No line-by-line Python translation - Rust has better tools for these problems:

- Python decorators -> Rust procedural macros
- Pydantic validation -> Compile-time type checking + serde
- ASGI lifecycle -> Structured concurrency regions
- Runtime reflection -> Compile-time code generation

### 2. Zero-Cost Abstractions

| Technique | Implementation |
|-----------|----------------|
| No runtime reflection | Proc macros analyze types at compile time |
| No trait objects on hot paths | Monomorphization via generics |
| Pre-allocated buffers | 4KB default, configurable per-route |
| Zero-copy HTTP parsing | Borrowed types reference request buffer |
| Inline critical paths | `#[inline(always)]` on hot code |

### 3. Cancel-Correct by Default

Every request handler runs in an asupersync **region**. Client disconnects, timeouts, and shutdowns trigger graceful cancellation:

```
Connection Accepted
    |
    v
+-----------------------------------------------+
|  Request Region (owns all request work)       |
|  +-------------------------------------------+|
|  |  Handler Task                             ||
|  |  +-- Dependency Task (DB query)           ||
|  |  +-- Dependency Task (cache lookup)       ||
|  |  +-- Background Task (logging)            ||
|  +-------------------------------------------+|
|                                               |
|  Region close waits for ALL tasks to finish  |
+-----------------------------------------------+
    |
    v
Response Sent (only after region quiescent)
```

**No orphaned tasks.** Client disconnect -> cancel region -> all tasks cleaned up.

### 4. Minimal Dependencies

| Crate | Purpose | Why |
|-------|---------|-----|
| `asupersync` | Async runtime | Our own - cancel-correct, capability-secure |
| `serde` | Serialization traits | Zero-cost, industry standard |
| `serde_json` | JSON parsing | Fast, well-optimized |

**Explicitly avoided:** Tokio, Hyper, Axum, Tower, runtime-reflection crates. Total dependency count: 3 crates vs. 80+ for Axum.

---

## How fastapi_rust Compares

| Feature | fastapi_rust | Axum | Actix-web | Rocket |
|---------|--------------|------|-----------|--------|
| Zero-copy HTTP parsing | **Custom** | Hyper | Partial | No |
| Compile-time routes | **Proc macros** | Runtime | Runtime | Macros |
| Structured concurrency | **asupersync** | Tokio spawn | Actix-rt | Tokio |
| Cancel-correct shutdown | **Native** | Manual | Manual | Manual |
| Dependency injection | **Native + cache** | State only | Data only | Managed |
| OpenAPI generation | **Compile-time** | External | External | External |
| Deterministic testing | **Lab runtime** | No | No | No |
| Dependencies | **3 crates** | ~80+ | ~60+ | ~50+ |
| FastAPI-style errors | **Yes (422 format)** | No | No | No |

### When to Use fastapi_rust

- You need cancel-correct request handling (graceful shutdown, timeouts)
- You want compile-time route validation
- You're building with asupersync for structured concurrency
- You want deterministic tests for concurrent code
- You're familiar with FastAPI and want similar ergonomics in Rust

### When to Consider Alternatives

- You need production-proven stability today (fastapi_rust is v0.1.2)
- You require WebSocket support (coming in Phase 2)
- You have existing Tokio-based infrastructure
- You need the massive ecosystem of Tower middleware

---

## Installation

### Add to Cargo.toml

```toml
[dependencies]
fastapi_rust = { package = "fastapi-rust", version = "0.1.2" }
asupersync = "0.1.0"
serde = { version = "1", features = ["derive"] }
```

**Note**: The crates.io package is `fastapi-rust`, and the crate name is `fastapi_rust`.

### From Source

```bash
git clone https://github.com/Dicklesworthstone/fastapi_rust.git
cd fastapi_rust
cargo build --release
```

### Requirements

- **Rust 1.85+** (2024 edition)
- **[asupersync](https://github.com/Dicklesworthstone/asupersync)** (co-developed runtime)

---

## Architecture

```
+-------------------------------------------------------------------+
|                       fastapi_rust (facade)                        |
|   Re-exports all public types, prelude module                      |
+-------------------------------------------------------------------+
        |           |           |           |           |
        v           v           v           v           v
+-----------+ +-----------+ +-----------+ +-----------+ +-----------+
|   core    | |   http    | |  router   | |  macros   | |  openapi  |
|           | |           | |           | |           | |           |
| - Request | | - Parser  | | - Trie    | | - #[get]  | | - Schema  |
| - Response| | - Body    | | - Match   | | - #[post] | | - Builder |
| - Context | | - Query   | | - Registry| | - Derive  | | - Spec    |
| - Extract | | - Headers | |           | |           | |           |
| - Depends | | - Writer  | |           | |           | |           |
| - Error   | | - Server  | |           | |           | |           |
| - Middle. | | - Stream  | |           | |           | |           |
| - Logging | |           | |           | |           | |           |
| - Testing | |           | |           | |           | |           |
| - Shutdown| |           | |           | |           | |           |
+-----------+ +-----------+ +-----------+ +-----------+ +-----------+
        |
        v
+-------------------------------------------------------------------+
|                         asupersync                                 |
|   Structured concurrency - Cx - Regions - Budgets - Lab           |
+-------------------------------------------------------------------+
```

### Crate Overview

| Crate | ~Lines | Purpose |
|-------|--------|---------|
| `fastapi_rust` | 100 | Facade: re-exports, prelude |
| `fastapi-core` | 6,000 | Request, Response, extractors, DI, middleware, logging, testing, shutdown |
| `fastapi-http` | 2,500 | Zero-copy HTTP/1.1 parser, body handling, query parsing, streaming |
| `fastapi-router` | 600 | Radix trie routing, path matching, conflict detection |
| `fastapi-macros` | 400 | `#[get]`, `#[post]`, `#[derive(Validate)]`, `#[derive(JsonSchema)]` |
| `fastapi-openapi` | 500 | OpenAPI 3.1 types, schema builder, spec generation |

---

## Extractors

Extract typed data from requests declaratively:

```rust
use fastapi_rust::prelude::*;

#[get("/users/{id}")]
async fn get_user(
    cx: &Cx,                           // Capability context (required)
    id: Path<i64>,                     // Path parameter: /users/123
    q: Query<SearchParams>,            // Query string: ?q=...&limit=...
    auth: Header<Authorization>,       // Required header
    accept: Header<Option<Accept>>,    // Optional header
) -> Result<Json<User>, HttpError> {
    // Types declare what you need - framework handles extraction
    // Wrong types -> compile error
    // Missing required -> 422 with FastAPI-compatible error
}

#[post("/items")]
async fn create(
    cx: &Cx,
    item: Json<CreateItem>,            // JSON body
) -> Result<Response, HttpError> {
    // 415 if wrong Content-Type
    // 413 if payload too large (configurable)
    // 422 if parse error (with location path)
}
```

### Available Extractors

| Extractor | Description | Error Response |
|-----------|-------------|----------------|
| `Path<T>` | URL path parameters | 422 if missing/wrong type |
| `Query<T>` | Query string | 422 if missing/invalid |
| `Json<T>` | JSON request body | 415/413/422 |
| `Header<T>` | Single header value | 422 if missing/invalid |
| `HeaderValues<T>` | All values for header | 422 if invalid |
| `State<T>` | Application state | 500 if not configured |
| `Depends<T>` | Dependency injection | Depends on factory |
| `Option<T>` | Any extractor, optional | Never fails |

---

## Middleware

Composable middleware with onion model execution:

```rust
use fastapi_rust::prelude::*;

// Built-in middleware
let app = App::builder()
    .middleware(RequestIdMiddleware::new())      // Add X-Request-Id
    .middleware(RequestResponseLogger::default()) // Log all requests
    .middleware(Cors::permissive())               // CORS handling
    .middleware(RequireHeader::new("X-API-Key")) // Require header
    .middleware(AddResponseHeader::new("X-Powered-By", b"fastapi_rust"))
    .build();

// Custom middleware
struct Timing;

impl Middleware for Timing {
    async fn before(&self, ctx: &RequestContext, req: &mut Request) -> ControlFlow {
        // Store start time in context
        ControlFlow::Continue
    }

    async fn after(&self, ctx: &RequestContext, req: &Request, mut resp: Response) -> Response {
        // Add X-Response-Time header
        resp
    }
}
```

### Execution Order

```
Request -> MW1.before -> MW2.before -> MW3.before -> Handler
                                                        |
Response <- MW1.after <- MW2.after <- MW3.after <- Response
```

First registered runs first on the way in, last on the way out (onion model).

---

## Dependency Injection

Request-scoped dependencies with caching:

```rust
use fastapi_rust::prelude::*;

// Define a dependency
#[derive(Clone)]
struct DatabasePool { /* ... */ }

impl FromDependency for DatabasePool {
    type Error = HttpError;

    async fn from_dependency(ctx: &RequestContext, req: &mut Request) -> Result<Self, HttpError> {
        // Resolved once per request, cached for subsequent uses
        Ok(DatabasePool::connect().await?)
    }
}

// Use in handler
#[get("/users/{id}")]
async fn get_user(
    cx: &Cx,
    id: Path<i64>,
    db: Depends<DatabasePool>,  // Automatically resolved and cached
) -> Result<Json<User>, HttpError> {
    let user = db.fetch_user(id.0).await?;
    Ok(Json(user))
}

// Override for testing
let overrides = DependencyOverrides::new()
    .with::<DatabasePool>(MockDatabase::new());
```

### Dependency Scopes

| Scope | Behavior |
|-------|----------|
| `Request` (default) | Resolve once, cache for request lifetime |
| `Function` | Resolve on every extraction |
| `NoCache` | Explicit opt-out of caching |

---

## Testing

In-process testing without network I/O:

```rust
use fastapi_rust::testing::*;

#[test]
fn test_get_item() {
    let client = TestClient::new(app);

    let resp = client.get("/items/42")
        .header("Authorization", "Bearer token")
        .send();

    assert_eq!(resp.status(), 200);

    let item: Item = resp.json();
    assert_eq!(item.id, 42);
}

#[test]
fn test_deterministic() {
    // Same seed = same execution order for concurrent operations
    let client = TestClient::with_seed(app, 12345);

    // Reproducible even with concurrent handlers
    let resp = client.post("/items")
        .json(&new_item)
        .send();

    assert_eq!(resp.status(), 201);
}

#[test]
fn test_with_overrides() {
    let overrides = DependencyOverrides::new()
        .with::<Database>(MockDatabase::new());

    let client = TestClient::new(app)
        .with_overrides(overrides);

    // Handler receives MockDatabase instead of real one
}
```

### Assertion Helpers

```rust
use fastapi_core::{assert_status, assert_header, assert_json};

assert_status!(resp, 200);
assert_header!(resp, "Content-Type", "application/json");
assert_json!(resp, {"id": 42, "name": "Widget"});
```

---

## Error Handling

FastAPI-compatible validation errors:

```rust
// Handler returns HttpError
#[get("/items/{id}")]
async fn get_item(id: Path<i64>) -> Result<Json<Item>, HttpError> {
    if id.0 < 0 {
        return Err(HttpError::unprocessable_entity()
            .detail("ID must be positive")
            .loc(["path", "id"]));
    }
    // ...
}
```

**Error response format (FastAPI-compatible):**

```json
{
  "detail": [
    {
      "type": "value_error",
      "loc": ["path", "id"],
      "msg": "ID must be positive",
      "input": -1
    }
  ]
}
```

### Built-in Error Types

| Status | Constructor | Use Case |
|--------|-------------|----------|
| 400 | `HttpError::bad_request()` | Malformed request |
| 401 | `HttpError::unauthorized()` | Missing/invalid auth |
| 403 | `HttpError::forbidden()` | Permission denied |
| 404 | `HttpError::not_found()` | Resource not found |
| 413 | `HttpError::payload_too_large()` | Body exceeds limit |
| 415 | `HttpError::unsupported_media_type()` | Wrong Content-Type |
| 422 | `HttpError::unprocessable_entity()` | Validation failed |
| 500 | `HttpError::internal()` | Server error |

---

## Graceful Shutdown

Cancel-correct shutdown with configurable grace periods:

```rust
use fastapi_rust::prelude::*;
use fastapi_core::shutdown::*;

let app = App::builder()
    .graceful_shutdown(GracefulConfig {
        grace_period: Duration::from_secs(30),
        force_timeout: Duration::from_secs(5),
    })
    .on_shutdown(|phase| async move {
        match phase {
            ShutdownPhase::GracePeriod => {
                // Stop accepting new connections
                // Wait for in-flight requests
            }
            ShutdownPhase::ForceClose => {
                // Cancel remaining requests
            }
        }
        Ok(())
    })
    .build();
```

Shutdown propagates through asupersync regions - no orphaned tasks.

---

## Configuration

```rust
use fastapi_rust::prelude::*;

	let app = App::builder()
	    // Metadata
	    .title("My API")
	    .version("1.0.0")
	    .description("A sample API built with fastapi_rust")

	    // Routes
	    .route_entry(get_item_route())
	    .route_entry(create_item_route())
	    .route_entry(delete_item_route())

    // Middleware (order matters)
    .middleware(RequestIdMiddleware::new())
    .middleware(Cors::new(CorsConfig {
        allow_origins: vec!["https://example.com".into()],
        allow_methods: vec![Method::Get, Method::Post],
        allow_headers: vec!["Authorization".into()],
        max_age: Some(3600),
    }))

    // Shared state
    .state(DatabasePool::new())
    .state(CacheClient::new())

    // Exception handlers
    .exception_handler(|err: DatabaseError| {
        HttpError::internal().detail(err.to_string())
    })

    // Lifecycle hooks
    .on_startup(|| async {
        println!("Starting up...");
        Ok(())
    })
    .on_shutdown(|_| async {
        println!("Shutting down...");
        Ok(())
    })

    // Build
    .build();
```

---

## Troubleshooting

### Common Issues

| Problem | Cause | Solution |
|---------|-------|----------|
| `asupersync not found` | Missing dependency | Add `asupersync` to Cargo.toml |
| `Cx lifetime error` | Holding Cx across await | Use `cx.checkpoint()` pattern |
| Route conflicts | Overlapping path patterns | Check for `{param}` vs literal conflicts |
| 422 on valid JSON | Missing `#[derive(Deserialize)]` | Add serde derive to your types |
| Middleware not running | Wrong registration order | Check middleware ordering |

### Debugging Tips

```rust
// Enable request logging
.middleware(RequestResponseLogger::new(LogConfig {
    log_bodies: true,
    log_headers: true,
}))

// Check route registration
app.routes().for_each(|r| println!("{} {}", r.method, r.path));

// Deterministic test reproduction
TestClient::with_seed(app, failing_seed)
```

---

## Limitations

### What fastapi_rust Doesn't Do (Yet)

| Feature | Status | Target Phase |
|---------|--------|--------------|
| TCP server | Scaffolding complete | Phase 1 (asupersync I/O) |
| WebSocket support | Not started | Phase 2 |
| File uploads / multipart | Not started | Phase 6 |
| HTTP/2 | Not planned | Post-v1.0 |
| Production deployment | Early development | Post-v1.0 |

### Known Constraints

- **Requires asupersync**: Won't work with Tokio (by design)
- **Rust 1.85+**: Uses 2024 edition features
- **Early development**: API will change before v1.0
- **No ecosystem**: Can't use Tower middleware or Axum extractors

---

## Development Status

```
Phase 0: [DONE] Foundation
         - Core types (Request, Response, Error)
         - Zero-copy HTTP parser
         - Extractor system
         - Middleware abstraction

Phase 1: [IN PROGRESS] TCP Server
         - asupersync I/O integration
         - Connection handling
         - Request lifecycle

Phase 2: [PLANNED] Router + Params
         - Radix trie routing
         - Path parameter extraction
         - Route conflict detection

Phase 3: [PLANNED] Validation
         - Derive macros
         - Error formatting

Phase 4: [PARTIAL] Dependency Injection
         - Basic DI working
         - Need: scopes, overrides

Phase 5: [PARTIAL] OpenAPI
         - Spec types defined
         - Need: generation from routes

Phase 6: [PLANNED] Advanced Features
         - File uploads
         - Streaming responses
         - Background tasks
```

---

## FAQ

### Why "fastapi_rust"?

It's a Rust web framework inspired by Python's [FastAPI](https://fastapi.tiangolo.com/), preserving the type-driven API design while achieving native performance and cancel-correctness.

### Why not use Tokio/Axum?

Tokio's spawn model makes cancel-correctness difficult - tasks can outlive their scope, leading to resource leaks and subtle bugs. asupersync's structured concurrency ensures all request-related work completes or cancels together.

### Can I use this in production?

Not yet. This is v0.1.2 in active development. The HTTP server implementation is pending asupersync's I/O support.

### How fast is it?

We haven't benchmarked yet (no TCP server), but the architecture is designed for:
- Zero allocations on the fast path
- Zero-copy request parsing
- No runtime reflection
- Pre-allocated buffers (4KB default)

### Does it support async/await?

Yes, fully. All handlers, middleware, and extractors are async-native, built on asupersync's structured concurrency model.

### Why the minimal dependency approach?

Each dependency is a maintenance burden, security surface, and compile-time cost. By keeping dependencies to 3 crates, we:
- Reduce build times significantly
- Have full control over behavior
- Avoid dependency conflicts
- Make auditing practical

### How do validation errors compare to FastAPI?

Identical format. The same client code that handles FastAPI 422 responses will work with fastapi_rust:

```json
{
  "detail": [
    {"type": "missing", "loc": ["body", "email"], "msg": "Field required"}
  ]
}
```

---

## About Contributions

Please don't take this the wrong way, but I do not accept outside contributions for any of my projects. I simply don't have the mental bandwidth to review anything, and it's my name on the thing, so I'm responsible for any problems it causes; thus, the risk-reward is highly asymmetric from my perspective. I'd also have to worry about other "stakeholders," which seems unwise for tools I mostly make for myself for free. Feel free to submit issues, and even PRs if you want to illustrate a proposed fix, but know I won't merge them directly. Instead, I'll have Claude or Codex review submissions via `gh` and independently decide whether and how to address them. Bug reports in particular are welcome. Sorry if this offends, but I want to avoid wasted time and hurt feelings. I understand this isn't in sync with the prevailing open-source ethos that seeks community contributions, but it's the only way I can move at this velocity and keep my sanity.

---

## License

MIT license. See [LICENSE](LICENSE).

---

## Related Projects

| Project | Description |
|---------|-------------|
| [asupersync](https://github.com/Dicklesworthstone/asupersync) | Structured concurrency async runtime (co-developed) |
| [FastAPI](https://fastapi.tiangolo.com/) | The Python framework that inspired this project |
