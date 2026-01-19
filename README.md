# fastapi_rust

<div align="center">

**Ultra-optimized Rust web framework inspired by Python's FastAPI**

[![License: MIT](https://img.shields.io/badge/License-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org/)
[![Status](https://img.shields.io/badge/status-early%20development-yellow.svg)]()

*Type-safe routing • Zero-copy parsing • Structured concurrency • OpenAPI generation*

</div>

---

## TL;DR

**The Problem**: Rust web frameworks either sacrifice developer ergonomics for performance (raw `hyper`) or hide allocations behind layers of abstraction (Axum + Tower). None leverage structured concurrency for cancel-correct request handling.

**The Solution**: fastapi_rust brings FastAPI's intuitive, type-driven API design to Rust with zero-copy HTTP parsing, compile-time route validation, and first-class integration with [asupersync](../asupersync) for structured concurrency and deterministic testing.

### Why fastapi_rust?

| Feature | What It Does |
|---------|--------------|
| **Zero-copy parsing** | HTTP requests parsed directly from buffers, no allocations on the fast path |
| **Compile-time validation** | Invalid routes fail at build time via proc macros, not runtime |
| **Structured concurrency** | Request handlers run in regions; cancellation is automatic and correct |
| **Type-driven extractors** | Declare types, framework extracts and validates automatically |
| **Minimal dependencies** | Only `asupersync` + `serde` — no Tokio, no Tower, no hidden layers |
| **Deterministic testing** | Lab runtime for reproducible concurrent request tests |

---

## Quick Example

```rust
use fastapi::prelude::*;

#[derive(Serialize, Deserialize, JsonSchema)]
struct Item {
    id: i64,
    name: String,
    price: f64,
}

#[get("/items/{id}")]
async fn get_item(cx: &Cx, id: Path<i64>) -> Json<Item> {
    // cx provides: checkpoint(), budget(), region_id(), task_id()
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
    Response::created()
        .json(&item.0)
}

fn main() {
    let app = App::new()
        .title("My API")
        .version("1.0.0")
        .route(get_item)
        .route(create_item);

    // Serve with asupersync (coming soon)
    // asupersync::block_on(app.serve("0.0.0.0:8000"));
}
```

---

## Design Philosophy

### 1. Extract Spec, Never Translate

We study FastAPI's behavior and ergonomics, then implement idiomatically in Rust. No line-by-line Python translation — Rust has better tools for these problems.

### 2. Minimal Dependencies

| Crate | Purpose |
|-------|---------|
| `asupersync` | Our own async runtime — cancel-correct, capability-secure |
| `serde` | Serialization traits (zero-cost, industry standard) |
| `serde_json` | JSON parsing (fast, well-optimized) |

We explicitly avoid Tokio, Hyper, Axum, Tower, and runtime-reflection crates.

### 3. Zero-Cost Abstractions

- No runtime reflection — proc macros analyze types at compile time
- No trait objects on hot paths — monomorphization via generics
- Pre-allocated buffers — zero allocation on fast paths
- Zero-copy HTTP parsing — borrowed types reference request buffer

### 4. Cancel-Correct

Every handler runs in an asupersync region. Client disconnects, timeouts, and shutdowns trigger graceful cancellation. Resources clean up automatically via structured concurrency.

### 5. Type-Driven API

```rust
// Types declare what you need — framework handles extraction
#[get("/users/{id}")]
async fn get_user(
    cx: &Cx,                    // Capability context (required)
    id: Path<i64>,              // Path parameter
    q: Query<SearchParams>,     // Query string
    auth: Header<Authorization>,// Header
) -> Result<Json<User>, HttpError> {
    // ...
}
```

---

## How fastapi_rust Compares

| Feature | fastapi_rust | Axum | Actix-web | Rocket |
|---------|--------------|------|-----------|--------|
| Zero-copy HTTP parsing | ✅ Custom | ❌ Hyper | ⚠️ Partial | ❌ |
| Compile-time routes | ✅ Proc macros | ❌ Runtime | ❌ Runtime | ✅ Macros |
| Structured concurrency | ✅ asupersync | ❌ Tokio | ❌ Actix-rt | ❌ Tokio |
| Cancel-correct shutdown | ✅ Native | ⚠️ Manual | ⚠️ Manual | ⚠️ Manual |
| Dependency injection | ✅ Native | ⚠️ State only | ⚠️ Data only | ⚠️ Managed |
| OpenAPI generation | ✅ Compile-time | ❌ External | ❌ External | ❌ External |
| Deterministic testing | ✅ Lab runtime | ❌ | ❌ | ❌ |
| Dependencies | 3 crates | ~80+ | ~60+ | ~50+ |

**Choose fastapi_rust when**:
- You need cancel-correct request handling (graceful shutdown, timeouts)
- You want compile-time route validation
- You're building with asupersync for structured concurrency
- You want deterministic tests for concurrent code

**Consider alternatives when**:
- You need production-proven stability today (fastapi_rust is v0.1.0)
- You require WebSocket support (coming in Phase 2)
- You have existing Tokio-based infrastructure

---

## Installation

### Add to Cargo.toml

```toml
[dependencies]
fastapi = { git = "https://github.com/Dicklesworthstone/fastapi_rust.git" }
asupersync = { git = "https://github.com/Dicklesworthstone/asupersync.git" }
serde = { version = "1", features = ["derive"] }
```

### From Source

```bash
git clone https://github.com/Dicklesworthstone/fastapi_rust.git
cd fastapi_rust
cargo build --release
```

### Requirements

- Rust 1.85+ (2024 edition)
- [asupersync](../asupersync) (co-developed runtime)

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         fastapi (facade)                        │
│   Re-exports all public types, prelude module                   │
└─────────────────────────────────────────────────────────────────┘
        │           │           │           │           │
        ▼           ▼           ▼           ▼           ▼
┌───────────┐ ┌───────────┐ ┌───────────┐ ┌───────────┐ ┌───────────┐
│   core    │ │   http    │ │  router   │ │  macros   │ │  openapi  │
│           │ │           │ │           │ │           │ │           │
│ • Request │ │ • Parser  │ │ • Trie    │ │ • #[get]  │ │ • Schema  │
│ • Response│ │ • Body    │ │ • Match   │ │ • #[post] │ │ • Builder │
│ • Context │ │ • Query   │ │ • Registry│ │ • Derive  │ │ • Spec    │
│ • Extract │ │ • Headers │ │           │ │           │ │           │
│ • Depends │ │ • Writer  │ │           │ │           │ │           │
│ • Error   │ │           │ │           │ │           │ │           │
│ • Middle. │ │           │ │           │ │           │ │           │
│ • Logging │ │           │ │           │ │           │ │           │
│ • Testing │ │           │ │           │ │           │ │           │
└───────────┘ └───────────┘ └───────────┘ └───────────┘ └───────────┘
        │
        ▼
┌─────────────────────────────────────────────────────────────────┐
│                         asupersync                              │
│   Structured concurrency • Cx • Regions • Budgets • Lab        │
└─────────────────────────────────────────────────────────────────┘
```

### Crate Overview

| Crate | Lines | Purpose |
|-------|-------|---------|
| `fastapi` | ~100 | Facade: re-exports, prelude |
| `fastapi-core` | ~2,000 | Request, Response, extractors, DI, middleware, logging, testing |
| `fastapi-http` | ~900 | Zero-copy HTTP/1.1 parser, body handling, query parsing |
| `fastapi-router` | ~300 | Trie-based routing, path matching, registry |
| `fastapi-macros` | ~150 | `#[get]`, `#[post]`, `#[derive(Validate)]`, `#[derive(JsonSchema)]` |
| `fastapi-openapi` | ~100 | OpenAPI 3.1 types, schema builder |

---

## Extractors

Extract typed data from requests declaratively:

```rust
use fastapi::prelude::*;

#[get("/search")]
async fn search(
    cx: &Cx,                           // Capability context
    q: Query<SearchParams>,            // ?q=...&limit=...
    auth: Header<Option<Bearer>>,      // Optional auth header
    accept: Header<Accept>,            // Required header
) -> Result<Json<Results>, HttpError> {
    // All extraction and validation happens automatically
    // Wrong types → compile error
    // Missing required → 422 response
    // Wrong content-type → 415 response
}

#[post("/items")]
async fn create(
    cx: &Cx,
    item: Json<CreateItem>,            // JSON body (415 if wrong type)
) -> Result<Response, HttpError> {
    // Payload too large → 413
    // Parse error → 422 with details
}
```

---

## Middleware

Composable middleware with onion model execution:

```rust
use fastapi::prelude::*;

// Built-in middleware
let stack = MiddlewareStack::new()
    .with(RequestResponseLogger::default())  // Logs all requests
    .with(Cors::permissive())                // CORS handling
    .with(RequireHeader::new("X-API-Key"))   // Require header
    .with(AddResponseHeader::new("X-Request-Id", generate_id));

// Custom middleware
struct Timing;

impl Middleware for Timing {
    async fn before(&self, req: &mut Request, cx: &Cx) -> ControlFlow {
        let start = cx.now();
        req.extensions_mut().insert(start);
        ControlFlow::Continue
    }

    async fn after(&self, req: &Request, resp: &mut Response, cx: &Cx) {
        if let Some(start) = req.extensions().get::<Instant>() {
            let elapsed = cx.now() - *start;
            resp.headers_mut().push(("X-Response-Time", elapsed.as_micros().to_string()));
        }
    }
}
```

---

## Dependency Injection

Request-scoped dependencies with caching:

```rust
use fastapi::prelude::*;

// Define a dependency
struct DatabasePool { /* ... */ }

impl FromDependency for DatabasePool {
    type Config = DefaultDependencyConfig;

    async fn from_dependency(cx: &Cx, cache: &DependencyCache) -> Result<Self, HttpError> {
        // Resolved once per request, cached for subsequent uses
        Ok(DatabasePool::connect().await?)
    }
}

// Use in handler
#[get("/users/{id}")]
async fn get_user(
    cx: &Cx,
    id: Path<i64>,
    db: Depends<DatabasePool>,  // Automatically resolved
) -> Result<Json<User>, HttpError> {
    let user = db.fetch_user(id.0).await?;
    Ok(Json(user))
}

// Override for testing
let overrides = DependencyOverrides::new()
    .with::<DatabasePool>(MockDatabase::new());
```

---

## Testing

In-process testing without network I/O:

```rust
use fastapi::testing::*;

#[test]
fn test_get_item() {
    // Deterministic testing with Lab runtime
    asupersync::Lab::new()
        .seed(12345)  // Reproducible
        .run(|| async {
            let client = TestClient::new(app);

            let resp = client.get("/items/42")
                .header("Authorization", "Bearer token")
                .send()
                .await;

            assert_eq!(resp.status(), 200);

            let item: Item = resp.json().await;
            assert_eq!(item.id, 42);
        });
}
```

---

## Limitations

### What fastapi_rust Doesn't Do (Yet)

| Feature | Status | Planned |
|---------|--------|---------|
| TCP server | Scaffolding | Phase 1 (waiting on asupersync I/O) |
| Path parameter extraction | Macros present | Phase 2 |
| WebSocket support | Not started | Phase 2 |
| File uploads / multipart | Not started | Phase 6 |
| Production deployment | Early dev | Post-v1.0 |

### Known Constraints

- **Requires asupersync**: Won't work with Tokio (by design)
- **Nightly Rust**: Uses 2024 edition features
- **Early development**: API will change before v1.0

---

## FAQ

### Why "fastapi_rust"?

It's a Rust web framework inspired by Python's [FastAPI](https://fastapi.tiangolo.com/), preserving the type-driven API design while achieving native performance and cancel-correctness.

### Why not use Tokio/Axum?

Tokio's spawn model makes cancel-correctness difficult — tasks can outlive their scope. asupersync's structured concurrency ensures all request-related work completes or cancels together.

### Can I use this in production?

Not yet. This is v0.1.0 in active development. The HTTP server implementation is pending asupersync's I/O support.

### How fast is it?

We haven't benchmarked yet (no TCP server), but the architecture is designed for:
- Zero allocations on the fast path
- Zero-copy request parsing
- No runtime reflection
- Pre-allocated buffers (4KB default)

### Does it support async/await?

Yes, fully. All handlers, middleware, and extractors are async-native, built on asupersync's structured concurrency model.

---

## Development Status

```
Phase 0: ✅ Foundation (core types, HTTP parser, extractors)
Phase 1: 🔄 TCP Server (asupersync I/O integration)
Phase 2: 🔜 Router + Path Parameters
Phase 3: 🔜 Validation + Error Handling
Phase 4: 🔜 Dependency Injection (partially complete)
Phase 5: 🔜 OpenAPI Generation
Phase 6: 🔜 Security + Advanced Features
```

---

## About Contributions

Please don't take this the wrong way, but I do not accept outside contributions for any of my projects. I simply don't have the mental bandwidth to review anything, and it's my name on the thing, so I'm responsible for any problems it causes; thus, the risk-reward is highly asymmetric from my perspective. I'd also have to worry about other "stakeholders," which seems unwise for tools I mostly make for myself for free. Feel free to submit issues, and even PRs if you want to illustrate a proposed fix, but know I won't merge them directly. Instead, I'll have Claude or Codex review submissions via `gh` and independently decide whether and how to address them. Bug reports in particular are welcome. Sorry if this offends, but I want to avoid wasted time and hurt feelings. I understand this isn't in sync with the prevailing open-source ethos that seeks community contributions, but it's the only way I can move at this velocity and keep my sanity.

---

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

---

## Related Projects

| Project | Description |
|---------|-------------|
| [asupersync](../asupersync) | Structured concurrency async runtime (co-developed) |
| [FastAPI](https://fastapi.tiangolo.com/) | The Python framework that inspired this project |
