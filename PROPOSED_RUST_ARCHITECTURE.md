# Proposed Rust Architecture

> **Design from spec, not translation.** This document describes how to implement FastAPI's behaviors in idiomatic Rust.

**Reference:** `EXISTING_FASTAPI_STRUCTURE.md` (THE SPEC)
**Runtime:** asupersync (co-developed at `/data/projects/asupersync`)
**Created:** 2026-01-17

---

## Table of Contents

0. [Parity Matrix (As Of 2026-02-11)](#0-parity-matrix-as-of-2026-02-11)
1. [Design Principles](#1-design-principles)
2. [Asupersync Integration](#2-asupersync-integration)
3. [Crate Structure](#3-crate-structure)
4. [HTTP Layer](#4-http-layer)
5. [Router Design](#5-router-design)
6. [Extractor System](#6-extractor-system)
7. [Validation](#7-validation)
8. [Dependency Injection](#8-dependency-injection)
9. [Security Extractors](#9-security-extractors)
10. [OpenAPI Generation](#10-openapi-generation)
11. [Error Handling](#11-error-handling)
12. [Application Builder](#12-application-builder)

---

## 0. Parity Matrix (As Of 2026-02-11)

This section is a living, high-level parity view against the legacy FastAPI behaviors described in `EXISTING_FASTAPI_STRUCTURE.md`. It is intended to answer two questions quickly:

- What is implemented today, and where is it in the Rust codebase?
- What are the highest-impact gaps to close next for FastAPI parity?

| Subsystem | Rust Location(s) | Status | Notes / Gaps |
|---|---|---|---|
| HTTP request parsing (HTTP/1.1) | `crates/fastapi-http/src/parser.rs` | Implemented | Focus: zero-copy parse; security hardening tests exist. |
| Request body (Content-Length, chunked) | `crates/fastapi-http/src/body.rs` | Implemented | Async chunked stream now consumes trailers and avoids keep-alive read-ahead. |
| TCP server + keep-alive | `crates/fastapi-http/src/server.rs` | Partial | Server exists and uses `asupersync::net`, but the end-to-end surface is still evolving and hardening. |
| Routing + conflict detection | `crates/fastapi-core/src/routing.rs`, `crates/fastapi-router/src/trie.rs` | Implemented | Path params + converters supported; 405/OPTIONS behaviors present. |
| App builder + request pipeline | `crates/fastapi-core/src/app.rs` | Implemented | Mounting, middleware execution, response mutations, background tasks integration. |
| Extractors: Path/Query/Header/Cookie/Auth | `crates/fastapi-core/src/extract.rs`, `crates/fastapi-core/src/dependency.rs` | Implemented | Large extractor surface; verify edge-case parity in spec as matrix expands. |
| Multipart/form-data + UploadFile semantics | `crates/fastapi-core/src/multipart.rs`, `crates/fastapi-core/src/extract.rs` | Partial | Parser/extractor exists, streamed request bodies are parsed incrementally (no full-body pre-buffer in extractor path), streamed part assembly flushes safe prefixes incrementally, and large streamed file parts can stay spool-backed through `MultipartForm` move-based file APIs (`take_file`/`into_files`) before `UploadFile` operations. Remaining gap: complete API-surface parity for fully streamed consumption and broader edge-case parity coverage. |
| Dependency injection | `crates/fastapi-core/src/dependency.rs` | Implemented | Type-based `Depends<T>` with caching/overrides/scopes; differs from Python callable-based dependency declaration. |
| Validation errors (422 format) | `crates/fastapi-core/src/error.rs` | Implemented | JSON shape is designed to be FastAPI-compatible; keep expanding exact rule coverage vs spec. |
| Validation derive | `crates/fastapi-core/src/validation.rs`, `crates/fastapi-macros/src/validate.rs` | Implemented | Current suite: length, range(gt/ge/lt/le), email, url, regex subset (anchors/classes/\\d/quantifiers), multiple_of, nested, phone, contains/starts_with/ends_with, custom paths. Still not full Pydantic parity. |
| Responses (JSON/HTML/files) | `crates/fastapi-core/src/response.rs` | Partial | Core response types exist; advanced streaming/file semantics may need more parity work. |
| Background tasks | `crates/fastapi-core/src/extract.rs` (BackgroundTasks) | Implemented | Server executes tasks after response in `crates/fastapi-http/src/server.rs`. |
| Security primitives | `crates/fastapi-core/src/extract.rs` | Partial | Credential extractors exist; token validation logic is app-specific. |
| OpenAPI schema/spec types | `crates/fastapi-openapi/src/*` | Implemented | OpenAPI 3.1 types and `JsonSchema` trait exist. |
| OpenAPI generation (from routes/handlers) | `crates/fastapi-core/src/app.rs` (`OpenApiConfig`) | Partial | `RouteEntry` now preserves `fastapi_router::Route` metadata for macro-generated routes, and OpenAPI generation uses that metadata when present; still missing full handler/type-to-schema mapping. |
| Docs pages (Swagger/ReDoc shells) | `crates/fastapi-core/src/docs.rs` | Implemented | HTML shells exist; assets expected via CDN/static hosting. |
| Docs endpoints wiring (routes) | `crates/fastapi-core/src/app.rs` (`enable_docs`) | Implemented | `.enable_docs(DocsConfig)` mounts `/docs`, `/redoc`, and `/docs/oauth2-redirect` (paths configurable). |
| Testing harness | `crates/fastapi-core/src/testing.rs` | Implemented | In-process TestClient + assertions. |
| WebSockets | `crates/fastapi-core/src/websocket.rs`, `crates/fastapi-http/tests/websocket.rs` | Partial | Upgrade + frame parsing + ping/pong + close-handshake hardening are implemented with E2E coverage. Remaining parity tracked in `bd-z09e` (full FastAPI/Starlette surface and edge-case semantics). |
| HTTP/2 | `crates/fastapi-http/src/http2.rs`, `crates/fastapi-http/src/server.rs`, `crates/fastapi-http/tests/http2.rs` | Partial | H2C prior-knowledge, HPACK encoding/decoding, bidirectional flow control (WINDOW_UPDATE + overflow detection), GOAWAY send/receive with payload validation, RST_STREAM, SETTINGS negotiation (all 6 RFC params), PUSH_PROMISE rejection, DATA/HEADERS stream-0 rejection, interleaved control frames during body reads, send-side flow control with h2_fc_clamp_send. 28 E2E tests, 405 unit tests. Remaining: concurrent streams (multiplexing), stream state machine. Tracked as `bd-2c9t`. |

**Highest-leverage gaps (parity):**

- OpenAPI generation: map handlers/extractors/types to operations/schemas (remove placeholder behaviors).
- Validation rules: expand `Validate` derive + runtime validation to match spec exactly.
- Security: flesh out auth flows and error semantics to match legacy FastAPI expectations.

## 1. Design Principles

### 1.1 Zero-Cost Abstractions

| Principle | Implementation |
|-----------|----------------|
| No runtime reflection | Proc macros analyze types at compile time |
| No trait objects on hot path | Monomorphization via generics |
| No allocations on fast path | Pre-allocated buffers, arena allocation |
| Inline everything hot | `#[inline(always)]` on critical paths |

### 1.2 Compile-Time Guarantees

```rust
// Route type safety - invalid routes fail at compile time
#[get("/items/{item_id}")]
async fn get_item(
    cx: &Cx,
    item_id: Path<i64>,      // Type checked
    q: Query<Option<String>>, // Optional query
) -> Json<Item> { ... }
```

### 1.3 Built on Asupersync

Every design decision leverages asupersync's structured concurrency model:

- **Cx** — Capability token for all effects
- **Outcome** — Four-valued result with severity lattice
- **Budget** — Request timeouts and resource limits
- **Two-Phase** — Cancel-safe resource acquisition
- **Lab Runtime** — Deterministic testing

---

## 2. Asupersync Integration

### 2.1 The Cx Capability Token

Every HTTP handler receives a `Cx` reference — the capability token that gates all effects:

```rust
use asupersync::Cx;

#[get("/items/{id}")]
async fn get_item(cx: &Cx, id: Path<i64>) -> Outcome<Json<Item>, HttpError> {
    // Cx provides:
    // - cx.region_id()        → Request's region identity
    // - cx.task_id()          → Handler's task identity
    // - cx.budget()           → Remaining time/poll quota
    // - cx.checkpoint()?      → Cancellation check point
    // - cx.is_cancel_requested() → Check without yielding
    // - cx.masked(|| ...)     → Critical section (can't be cancelled)
    // - cx.trace("msg")       → Deterministic logging

    cx.checkpoint()?;  // Early exit if client disconnected

    let item = db_query(cx, id.0).await?;
    Outcome::Ok(Json(item))
}
```

### 2.2 Request Lifecycle as Region

Each HTTP request becomes an asupersync **Region**:

```
Connection Accepted
    │
    ▼
┌─────────────────────────────────────────────┐
│  Request Region (owns all request tasks)     │
│  ┌─────────────────────────────────────────┐│
│  │  Handler Task (main request processing) ││
│  │  ├── Dependency Task (DB query)         ││
│  │  ├── Dependency Task (cache lookup)     ││
│  │  └── Background Task (logging)          ││
│  └─────────────────────────────────────────┘│
│                                              │
│  Region close waits for ALL tasks to finish │
└─────────────────────────────────────────────┘
    │
    ▼
Response Sent (only after region quiescent)
```

**Benefits:**
- No orphaned tasks — region close waits for children
- Client disconnect → cancel request region → all tasks cleaned up
- Request timeout → budget exhausted → graceful cancellation

### 2.3 Budget for Request Timeouts

Use asupersync's `Budget` for HTTP timeouts:

```rust
use asupersync::{Budget, Time};

impl App {
    async fn handle_request(&self, cx: &Cx, req: Request) -> Response {
        // Apply request timeout budget
        let timeout_budget = Budget::new()
            .with_deadline(Time::now().saturating_add_secs(30))
            .with_poll_quota(10_000);  // Prevent infinite loops

        let request_cx = cx.with_budget(timeout_budget);

        // Handler runs with combined budget (tightest wins)
        match self.router.dispatch(&request_cx, req).await {
            Outcome::Ok(resp) => resp,
            Outcome::Err(e) => e.into_response(),
            Outcome::Cancelled(reason) => {
                // Client disconnected or timeout
                Response::with_status(StatusCode::REQUEST_TIMEOUT)
            }
            Outcome::Panicked(payload) => {
                // Handler panicked - log and return 500
                Response::internal_error()
            }
        }
    }
}
```

### 2.4 Outcome for Handler Returns

Handlers return `Outcome<T, E>` instead of `Result<T, E>` to properly handle cancellation:

```rust
use asupersync::Outcome;

// Handler signature
async fn handler(cx: &Cx, ...) -> Outcome<Response, HttpError>;

// Outcome severity lattice:
// Ok(T)           → severity 0 (success)
// Err(E)          → severity 1 (application error)
// Cancelled(R)    → severity 2 (request cancelled)
// Panicked(P)     → severity 3 (handler crashed)

// When joining concurrent operations (e.g., fan-out):
// The outcome with HIGHEST severity wins
// Example: join(Ok, Cancelled) → Cancelled
```

### 2.5 Two-Phase Pattern for Resources

Use asupersync's two-phase pattern for cancel-safe resource acquisition:

```rust
use asupersync::channel::{Sender, Permit};

// Database connection pool example
impl DbPool {
    pub async fn acquire(&self, cx: &Cx) -> Outcome<DbConn, PoolError> {
        // Phase 1: Reserve connection (cancel-safe)
        let permit = self.permits.reserve(cx).await?;

        // Phase 2: Get actual connection (infallible)
        let conn = permit.take_connection();

        Outcome::Ok(DbConn { conn, permit })
    }
}

// If cancelled during Phase 1: no cleanup needed
// If cancelled during Phase 2: permit drop returns connection to pool
```

### 2.6 Structured Concurrency Combinators

Use asupersync combinators for concurrent HTTP patterns:

```rust
use asupersync::combinator::{join_all, race, timeout, retry};

// Fan-out to multiple services
#[get("/aggregate")]
async fn aggregate(cx: &Cx) -> Outcome<Json<Combined>, Error> {
    // Run all in parallel, wait for all
    let results = join_all(cx, [
        fetch_service_a(cx),
        fetch_service_b(cx),
        fetch_service_c(cx),
    ]).await;

    // Aggregate outcomes (worst severity wins)
    Outcome::Ok(Json(Combined::from(results)))
}

// First successful response wins
#[get("/fastest")]
async fn fastest(cx: &Cx) -> Outcome<Json<Data>, Error> {
    // Race returns (winner_index, outcome), cancels losers
    let (_, outcome) = race(cx, [
        fetch_primary(cx),
        fetch_replica(cx),
    ]).await;

    outcome.map(Json)
}

// Retry with exponential backoff
#[post("/reliable")]
async fn reliable(cx: &Cx, body: Json<Payload>) -> Outcome<(), Error> {
    retry(cx,
        || send_to_queue(cx, &body.0),
        RetryPolicy::exponential(base: 100ms, max: 10s, attempts: 5)
    ).await
}
```

### 2.7 Lab Runtime for Testing

Use asupersync's `LabRuntime` for deterministic HTTP tests:

```rust
use asupersync::lab::{LabRuntime, LabConfig};

#[test]
fn concurrent_requests_deterministic() {
    let config = LabConfig::new().with_seed(12345);
    let lab = LabRuntime::new(config);

    lab.run(|| async {
        let app = App::new();

        // Spawn concurrent requests
        let results = join_all(&lab.cx(), [
            app.handle(Request::get("/a")),
            app.handle(Request::get("/b")),
            app.handle(Request::get("/c")),
        ]).await;

        // Results are deterministic for this seed
        assert_eq!(results[0].status(), 200);
    });

    // Same seed → same execution → reproducible bugs
}

#[test]
fn request_timeout_cleanup() {
    let lab = LabRuntime::new(LabConfig::default());

    lab.run(|| async {
        let app = App::new();

        // Request that will timeout
        let cx = lab.cx().with_budget(Budget::new()
            .with_deadline(Time::from_millis(100)));

        let result = app.handle(&cx, slow_request()).await;

        // Verify cancelled, not stuck
        assert!(result.is_cancelled());

        // Verify no task leaks
        assert!(!lab.has_leaked_tasks());
    });
}
```

### 2.8 Cancellation Checkpoints

Insert cancellation checkpoints in long operations:

```rust
async fn process_large_batch(cx: &Cx, items: Vec<Item>) -> Outcome<(), Error> {
    for (i, item) in items.iter().enumerate() {
        // Check for cancellation every N items
        if i % 100 == 0 {
            cx.checkpoint()?;  // Returns Cancelled if request aborted
        }

        process_item(cx, item).await?;
    }
    Outcome::Ok(())
}

// For critical cleanup that must complete:
async fn finalize_transaction(cx: &Cx, txn: Transaction) -> Outcome<(), Error> {
    cx.masked(|| async {
        // Inside masked: cx.checkpoint() always returns Ok
        // This section WILL complete even if cancelled
        txn.commit().await
    }).await
}
```

---

## 3. Crate Structure

```
fastapi_rust/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── fastapi/                  # Main library (re-exports)
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   ├── fastapi-core/             # Core types, traits
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── request.rs        # Request type
│   │       ├── response.rs       # Response types
│   │       ├── error.rs          # Error types
│   │       └── extract.rs        # FromRequest trait
│   ├── fastapi-http/             # HTTP/1.1 parser
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── parser.rs         # Zero-copy parser
│   │       ├── response.rs       # Response builder
│   │       └── server.rs         # Accept loop
│   ├── fastapi-router/           # Trie-based router
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── trie.rs           # Radix trie
│   │       └── match.rs          # Path matching
│   ├── fastapi-macros/           # Proc macros
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── route.rs          # #[get], #[post], etc.
│   │       ├── validate.rs       # #[validate]
│   │       └── openapi.rs        # Schema generation
│   └── fastapi-openapi/          # OpenAPI 3.1 types
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── schema.rs         # JSON Schema types
│           └── spec.rs           # OpenAPI document
```

### 3.1 Dependency Graph

```
fastapi (facade)
├── fastapi-core
├── fastapi-http
│   └── fastapi-core
├── fastapi-router
│   └── fastapi-core
├── fastapi-macros
│   └── fastapi-core (types only)
└── fastapi-openapi
    └── fastapi-core
```

### 3.2 External Dependencies

```toml
[workspace.dependencies]
# Runtime (our own)
asupersync = { path = "../asupersync" }

# Serialization (only these)
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

---

## 4. HTTP Layer

### 4.1 Request Type

```rust
/// Zero-copy HTTP request
pub struct Request<'a> {
    method: Method,
    path: &'a str,
    query: Option<&'a str>,
    headers: Headers<'a>,
    body: Body<'a>,

    // Extensions for extracted values
    extensions: Extensions,
}

pub enum Method {
    Get, Post, Put, Delete, Patch, Options, Head, Trace,
}

pub struct Headers<'a> {
    // Flat array of (name, value) pairs
    pairs: &'a [(HeaderName<'a>, &'a [u8])],
}

pub enum Body<'a> {
    Empty,
    Bytes(&'a [u8]),
    Stream(BodyStream),
}
```

### 4.2 Response Type

```rust
pub struct Response {
    status: StatusCode,
    headers: Vec<(HeaderName<'static>, HeaderValue)>,
    body: ResponseBody,
}

pub enum ResponseBody {
    Empty,
    Bytes(Vec<u8>),
    Json(Box<RawValue>),  // Pre-serialized JSON
    Stream(Box<dyn AsyncRead>),
}

impl Response {
    pub fn ok() -> Self { Self::with_status(StatusCode::OK) }
    pub fn created() -> Self { Self::with_status(StatusCode::CREATED) }
    pub fn no_content() -> Self { Self::with_status(StatusCode::NO_CONTENT) }

    pub fn json<T: Serialize>(value: T) -> Result<Self, Error> { ... }

    pub fn header(mut self, name: HeaderName<'static>, value: impl Into<HeaderValue>) -> Self { ... }
}
```

### 4.3 Status Codes

```rust
pub struct StatusCode(u16);

impl StatusCode {
    // Informational
    pub const CONTINUE: Self = Self(100);
    pub const SWITCHING_PROTOCOLS: Self = Self(101);

    // Success
    pub const OK: Self = Self(200);
    pub const CREATED: Self = Self(201);
    pub const ACCEPTED: Self = Self(202);
    pub const NO_CONTENT: Self = Self(204);

    // Redirection
    pub const MOVED_PERMANENTLY: Self = Self(301);
    pub const FOUND: Self = Self(302);
    pub const NOT_MODIFIED: Self = Self(304);
    pub const TEMPORARY_REDIRECT: Self = Self(307);
    pub const PERMANENT_REDIRECT: Self = Self(308);

    // Client Error
    pub const BAD_REQUEST: Self = Self(400);
    pub const UNAUTHORIZED: Self = Self(401);
    pub const FORBIDDEN: Self = Self(403);
    pub const NOT_FOUND: Self = Self(404);
    pub const METHOD_NOT_ALLOWED: Self = Self(405);
    pub const UNPROCESSABLE_ENTITY: Self = Self(422);

    // Server Error
    pub const INTERNAL_SERVER_ERROR: Self = Self(500);

    /// Status codes that must not have a body
    pub fn allows_body(self) -> bool {
        !matches!(self.0, 100..=103 | 204 | 304)
    }
}
```

---

## 5. Router Design

### 5.1 Radix Trie Router

```rust
/// Compile-time route table
pub struct Router {
    root: Node,
    routes: Vec<Route>,
}

struct Node {
    // Static segment
    segment: &'static str,

    // Children (sorted by segment for binary search)
    children: Vec<Node>,

    // Parameter capture
    param: Option<ParamInfo>,

    // Route index if this is a terminal node
    route_idx: Option<usize>,
}

struct ParamInfo {
    name: &'static str,
    converter: Converter,
}

enum Converter {
    Str,      // Default
    Int,      // :int
    Float,    // :float
    Uuid,     // :uuid
    Path,     // :path (matches /)
}
```

### 5.2 Route Matching

```rust
pub struct RouteMatch<'a> {
    route: &'a Route,
    params: SmallVec<[(&'static str, &'a str); 4]>,
}

impl Router {
    pub fn match_path<'a>(&'a self, path: &'a str) -> Option<RouteMatch<'a>> {
        let mut node = &self.root;
        let mut params = SmallVec::new();

        for segment in path.split('/').filter(|s| !s.is_empty()) {
            // Try static match first (binary search)
            if let Some(child) = node.find_static(segment) {
                node = child;
                continue;
            }

            // Try parameter match
            if let Some(child) = node.find_param() {
                if child.param.as_ref()?.converter.matches(segment) {
                    params.push((child.param.as_ref()?.name, segment));
                    node = child;
                    continue;
                }
            }

            return None;
        }

        node.route_idx.map(|idx| RouteMatch {
            route: &self.routes[idx],
            params,
        })
    }
}
```

### 5.3 Route Registration Macro

```rust
/// Generated at compile time
#[proc_macro_attribute]
pub fn get(attr: TokenStream, item: TokenStream) -> TokenStream {
    route_impl(Method::Get, attr, item)
}

// Usage:
#[get("/items/{item_id}")]
async fn get_item(cx: &Cx, item_id: Path<i64>) -> Json<Item> {
    // ...
}

// Expands to:
fn __route_get_item() -> Route {
    Route {
        path: "/items/{item_id}",
        method: Method::Get,
        handler: |cx, req| Box::pin(async move {
            let item_id = Path::<i64>::from_request(cx, &req).await?;
            get_item(cx, item_id).await.into_response()
        }),
        // OpenAPI metadata
        operation_id: "get_item",
        summary: None,
        // ...
    }
}
```

---

## 6. Extractor System

### 6.1 FromRequest Trait

```rust
/// Extract a value from an HTTP request
pub trait FromRequest: Sized {
    /// Error type when extraction fails
    type Error: IntoResponse;

    /// Extract from request
    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error>;

    /// OpenAPI parameter info (for schema generation)
    fn openapi_param() -> Option<Parameter> { None }

    /// OpenAPI request body info
    fn openapi_body() -> Option<RequestBody> { None }
}
```

### 6.2 Path Extractor

```rust
/// Extract path parameters
pub struct Path<T>(pub T);

impl<T: DeserializeOwned> FromRequest for Path<T> {
    type Error = PathError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        let params = req.extensions().get::<PathParams>()
            .ok_or(PathError::NoParams)?;

        // For single value: deserialize directly
        // For struct: build map and deserialize
        let value = if TypeId::of::<T>() == TypeId::of::<String>() {
            // Single string param
            serde_json::from_str(&format!("\"{}\"", params.get(0)?.1))?
        } else {
            // Struct from params
            let map: HashMap<_, _> = params.iter().collect();
            serde_json::from_value(serde_json::to_value(&map)?)?
        };

        Ok(Path(value))
    }
}
```

### 6.3 Query Extractor

```rust
/// Extract query parameters
pub struct Query<T>(pub T);

impl<T: DeserializeOwned> FromRequest for Query<T> {
    type Error = QueryError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        let query = req.query().unwrap_or("");
        let value = serde_urlencoded::from_str(query)?;
        Ok(Query(value))
    }
}

// Usage with Option for optional query params
#[get("/items")]
async fn list_items(
    cx: &Cx,
    Query(params): Query<ListParams>,
) -> Json<Vec<Item>> { ... }

#[derive(Deserialize)]
struct ListParams {
    #[serde(default)]
    skip: usize,
    #[serde(default = "default_limit")]
    limit: usize,
    q: Option<String>,
}
```

### 6.4 Header Extractor

```rust
/// Extract a single header
pub struct Header<T>(pub T);

impl<T: FromStr> FromRequest for Header<T>
where
    T::Err: std::error::Error,
{
    type Error = HeaderError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        // Header name from type annotation via macro
        let name = T::HEADER_NAME; // Set by derive macro
        let value = req.headers()
            .get(name)
            .ok_or(HeaderError::Missing(name))?;
        let parsed = std::str::from_utf8(value)?
            .parse()
            .map_err(|e| HeaderError::Parse(name, e))?;
        Ok(Header(parsed))
    }
}

// Convert underscores to hyphens (like FastAPI)
// user_agent -> User-Agent
fn convert_header_name(rust_name: &str) -> String {
    rust_name
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().chain(chars).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}
```

### 6.5 Cookie Extractor

```rust
/// Extract a cookie value
pub struct Cookie<T>(pub T);

impl<T: DeserializeOwned> FromRequest for Cookie<T> {
    type Error = CookieError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        let cookie_header = req.headers()
            .get("cookie")
            .ok_or(CookieError::NoCookies)?;

        // Parse cookie header
        let cookies = parse_cookies(cookie_header)?;
        let value = serde_json::from_value(serde_json::to_value(&cookies)?)?;
        Ok(Cookie(value))
    }
}
```

### 6.6 JSON Body Extractor

```rust
/// Extract JSON body
pub struct Json<T>(pub T);

impl<T: DeserializeOwned> FromRequest for Json<T> {
    type Error = JsonError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        // Check content-type
        let content_type = req.headers().get("content-type");
        if !is_json_content_type(content_type) {
            return Err(JsonError::WrongContentType);
        }

        // Read body
        let body = req.body().bytes().await?;

        // Parse JSON
        let value = serde_json::from_slice(&body)?;
        Ok(Json(value))
    }
}

// IntoResponse for Json
impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        match serde_json::to_vec(&self.0) {
            Ok(bytes) => Response::ok()
                .header("content-type", "application/json")
                .body(ResponseBody::Bytes(bytes)),
            Err(e) => Response::internal_error()
                .body(ResponseBody::Bytes(e.to_string().into_bytes())),
        }
    }
}
```

---

## 7. Validation

### 7.1 Validate Derive Macro

```rust
/// Compile-time validation code generation
#[derive(Validate)]
pub struct CreateItem {
    #[validate(length(min = 1, max = 100))]
    pub name: String,

    #[validate(range(min = 0.0))]
    pub price: f64,

    #[validate(email)]
    pub owner_email: String,

    #[validate(regex = "^[A-Z]{3}$")]
    pub code: String,
}
```

### 7.2 Validation Constraints

```rust
/// Validation constraint trait
pub trait Constraint<T> {
    fn validate(&self, value: &T) -> Result<(), ValidationError>;
}

// String constraints
pub struct Length { pub min: Option<usize>, pub max: Option<usize> }
pub struct Pattern { pub regex: &'static str }

// Numeric constraints
pub struct Range<T> { pub min: Option<T>, pub max: Option<T> }
pub struct MultipleOf<T> { pub value: T }

// Generated validation
impl Validate for CreateItem {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errors = Vec::new();

        // name: length(min = 1, max = 100)
        if self.name.len() < 1 {
            errors.push(ValidationError {
                loc: vec!["name".into()],
                msg: "String too short".into(),
                error_type: "string_too_short",
                ctx: json!({"min_length": 1}),
            });
        }
        if self.name.len() > 100 {
            errors.push(ValidationError {
                loc: vec!["name".into()],
                msg: "String too long".into(),
                error_type: "string_too_long",
                ctx: json!({"max_length": 100}),
            });
        }

        // price: range(min = 0.0)
        if self.price < 0.0 {
            errors.push(ValidationError {
                loc: vec!["price".into()],
                msg: "Value too small".into(),
                error_type: "greater_than_equal",
                ctx: json!({"ge": 0.0}),
            });
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationErrors(errors))
        }
    }
}
```

### 7.3 Validating Extractor

```rust
/// JSON body with validation
pub struct ValidJson<T>(pub T);

impl<T: DeserializeOwned + Validate> FromRequest for ValidJson<T> {
    type Error = ValidationError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        let Json(value) = Json::<T>::from_request(cx, req).await?;
        value.validate()?;
        Ok(ValidJson(value))
    }
}
```

---

## 8. Dependency Injection

### 8.1 Depends Pattern via Cx

```rust
/// Request-scoped dependency storage in Cx
impl Cx {
    /// Get or compute a dependency
    pub async fn get_or_insert<T: Send + Sync + 'static>(
        &self,
        key: TypeId,
        f: impl FnOnce() -> impl Future<Output = T>,
    ) -> &T {
        // Check cache first
        if let Some(value) = self.extensions().get::<T>() {
            return value;
        }

        // Compute and cache
        let value = f().await;
        self.extensions_mut().insert(value);
        self.extensions().get::<T>().unwrap()
    }
}
```

### 8.2 Dependency Extractor

```rust
/// Dependency injection extractor
pub struct Depends<T>(pub T);

impl<T: FromDependency> FromRequest for Depends<T> {
    type Error = T::Error;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        // Check for dependency override (testing)
        if let Some(override_fn) = cx.get_override::<T>() {
            return Ok(Depends(override_fn().await));
        }

        // Use cached or compute
        let value = cx.get_or_insert(
            TypeId::of::<T>(),
            || T::from_dependency(cx, req)
        ).await;

        Ok(Depends(value.clone()))
    }
}

/// Trait for types that can be injected as dependencies
pub trait FromDependency: Clone + Send + Sync + 'static {
    type Error: IntoResponse;

    async fn from_dependency(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error>;
}
```

### 8.3 Database Connection Example

```rust
/// Database connection pool (app-level state)
#[derive(Clone)]
pub struct DbPool(/* ... */);

/// Request-scoped database connection
#[derive(Clone)]
pub struct DbConn(/* ... */);

impl FromDependency for DbConn {
    type Error = DbError;

    async fn from_dependency(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        let pool = cx.state::<DbPool>()?;
        let conn = pool.get().await?;
        Ok(DbConn(conn))
    }
}

// Usage in handler
#[get("/users/{id}")]
async fn get_user(
    cx: &Cx,
    id: Path<i64>,
    Depends(db): Depends<DbConn>,
) -> Result<Json<User>, Error> {
    let user = db.query_one("SELECT * FROM users WHERE id = $1", &[&id.0]).await?;
    Ok(Json(user))
}
```

### 8.4 Chained Dependencies

```rust
/// Current user (depends on auth token)
#[derive(Clone)]
pub struct CurrentUser(pub User);

impl FromDependency for CurrentUser {
    type Error = AuthError;

    async fn from_dependency(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        // Get auth token (another dependency)
        let Depends(token) = Depends::<AuthToken>::from_request(cx, req).await?;

        // Get db connection (another dependency)
        let Depends(db) = Depends::<DbConn>::from_request(cx, req).await?;

        // Look up user
        let user = db.query_one(
            "SELECT * FROM users WHERE token = $1",
            &[&token.0]
        ).await?;

        Ok(CurrentUser(user))
    }
}
```

---

## 9. Security Extractors

### 9.1 Bearer Token

```rust
/// Extract bearer token from Authorization header
pub struct BearerToken(pub String);

impl FromRequest for BearerToken {
    type Error = AuthError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        let auth = req.headers()
            .get("authorization")
            .ok_or(AuthError::Missing)?;

        let auth_str = std::str::from_utf8(auth)?;
        let (scheme, token) = auth_str.split_once(' ')
            .ok_or(AuthError::InvalidFormat)?;

        if !scheme.eq_ignore_ascii_case("bearer") {
            return Err(AuthError::WrongScheme);
        }

        Ok(BearerToken(token.to_string()))
    }
}
```

### 9.2 HTTP Basic Auth

```rust
/// HTTP Basic authentication credentials
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

impl FromRequest for BasicAuth {
    type Error = AuthError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        let auth = req.headers()
            .get("authorization")
            .ok_or_else(|| AuthError::missing_with_challenge("Basic"))?;

        let auth_str = std::str::from_utf8(auth)?;
        let (scheme, encoded) = auth_str.split_once(' ')
            .ok_or(AuthError::InvalidFormat)?;

        if !scheme.eq_ignore_ascii_case("basic") {
            return Err(AuthError::wrong_scheme_with_challenge("Basic"));
        }

        let decoded = base64::decode(encoded)?;
        let decoded_str = String::from_utf8(decoded)?;
        let (username, password) = decoded_str.split_once(':')
            .ok_or(AuthError::InvalidFormat)?;

        Ok(BasicAuth {
            username: username.to_string(),
            password: password.to_string(),
        })
    }
}
```

### 9.3 API Key

```rust
/// API key from query parameter
pub struct ApiKeyQuery<const NAME: &'static str>(pub String);

impl<const NAME: &'static str> FromRequest for ApiKeyQuery<NAME> {
    type Error = AuthError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        let query = req.query().unwrap_or("");
        let params: HashMap<&str, &str> = serde_urlencoded::from_str(query)?;

        let key = params.get(NAME)
            .ok_or(AuthError::MissingApiKey)?;

        Ok(ApiKeyQuery(key.to_string()))
    }
}

/// API key from header
pub struct ApiKeyHeader<const NAME: &'static str>(pub String);

impl<const NAME: &'static str> FromRequest for ApiKeyHeader<NAME> {
    type Error = AuthError;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        let key = req.headers()
            .get(NAME)
            .ok_or(AuthError::MissingApiKey)?;

        Ok(ApiKeyHeader(std::str::from_utf8(key)?.to_string()))
    }
}

// Usage
#[get("/protected")]
async fn protected(
    cx: &Cx,
    ApiKeyHeader::<"X-API-Key">(key): ApiKeyHeader<"X-API-Key">,
) -> &'static str {
    "secret data"
}
```

### 9.4 Optional Authentication

```rust
/// Make any auth extractor optional
impl<T: FromRequest> FromRequest for Option<T> {
    type Error = Infallible;

    async fn from_request(cx: &Cx, req: &Request<'_>) -> Result<Self, Self::Error> {
        Ok(T::from_request(cx, req).await.ok())
    }
}

// Usage - optional bearer token
#[get("/maybe-auth")]
async fn maybe_auth(
    cx: &Cx,
    token: Option<BearerToken>,
) -> Json<Response> {
    match token {
        Some(t) => Json(Response::authenticated(&t.0)),
        None => Json(Response::anonymous()),
    }
}
```

---

## 10. OpenAPI Generation

### 10.1 Schema Trait

```rust
/// Generate JSON Schema for a type
pub trait JsonSchema {
    fn schema() -> Schema;
    fn schema_name() -> Option<&'static str> { None }
}

// Derive macro generates implementation
#[derive(JsonSchema)]
pub struct Item {
    pub id: i64,
    pub name: String,
    #[schema(nullable)]
    pub description: Option<String>,
}

// Generates:
impl JsonSchema for Item {
    fn schema() -> Schema {
        Schema::Object(ObjectSchema {
            properties: vec![
                ("id".into(), Schema::Integer),
                ("name".into(), Schema::String),
                ("description".into(), Schema::Nullable(Box::new(Schema::String))),
            ].into_iter().collect(),
            required: vec!["id".into(), "name".into()],
        })
    }

    fn schema_name() -> Option<&'static str> {
        Some("Item")
    }
}
```

### 10.2 OpenAPI Document Builder

```rust
pub struct OpenApiBuilder {
    info: Info,
    servers: Vec<Server>,
    paths: IndexMap<String, PathItem>,
    components: Components,
    tags: Vec<Tag>,
}

impl OpenApiBuilder {
    pub fn new(title: &str, version: &str) -> Self { ... }

    pub fn server(mut self, url: &str, description: Option<&str>) -> Self { ... }

    pub fn tag(mut self, name: &str, description: Option<&str>) -> Self { ... }

    /// Add route from macro-generated metadata
    pub fn route(mut self, route: &Route) -> Self {
        let path_item = self.paths
            .entry(route.path.to_string())
            .or_default();

        let operation = Operation {
            operation_id: Some(route.operation_id.to_string()),
            summary: route.summary.map(|s| s.to_string()),
            description: route.description.map(|s| s.to_string()),
            tags: route.tags.iter().map(|t| t.to_string()).collect(),
            parameters: route.collect_parameters(),
            request_body: route.request_body(),
            responses: route.responses(),
            deprecated: route.deprecated,
            security: route.security_requirements(),
        };

        match route.method {
            Method::Get => path_item.get = Some(operation),
            Method::Post => path_item.post = Some(operation),
            Method::Put => path_item.put = Some(operation),
            Method::Delete => path_item.delete = Some(operation),
            Method::Patch => path_item.patch = Some(operation),
            // ...
        }

        self
    }

    pub fn build(self) -> OpenApi { ... }
}
```

### 10.3 Route Metadata from Macros

```rust
/// Metadata extracted at compile time
pub struct Route {
    pub path: &'static str,
    pub method: Method,
    pub handler: Handler,

    // OpenAPI metadata
    pub operation_id: &'static str,
    pub summary: Option<&'static str>,
    pub description: Option<&'static str>,
    pub tags: &'static [&'static str],
    pub deprecated: bool,

    // Parameter schemas (generated from extractors)
    pub path_params: &'static [ParamSchema],
    pub query_params: &'static [ParamSchema],
    pub header_params: &'static [ParamSchema],

    // Body schema
    pub request_body: Option<BodySchema>,

    // Response schemas
    pub responses: &'static [ResponseSchema],

    // Security requirements
    pub security: &'static [SecurityRequirement],
}
```

---

## 11. Error Handling

### 11.1 Error Types

```rust
/// HTTP error that produces a response
pub struct HttpError {
    pub status: StatusCode,
    pub detail: Option<String>,
    pub headers: Vec<(HeaderName<'static>, HeaderValue)>,
}

impl HttpError {
    pub fn new(status: StatusCode) -> Self {
        Self { status, detail: None, headers: Vec::new() }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_header(mut self, name: HeaderName<'static>, value: HeaderValue) -> Self {
        self.headers.push((name, value));
        self
    }

    // Convenience constructors
    pub fn bad_request() -> Self { Self::new(StatusCode::BAD_REQUEST) }
    pub fn unauthorized() -> Self { Self::new(StatusCode::UNAUTHORIZED) }
    pub fn forbidden() -> Self { Self::new(StatusCode::FORBIDDEN) }
    pub fn not_found() -> Self { Self::new(StatusCode::NOT_FOUND) }
    pub fn internal() -> Self { Self::new(StatusCode::INTERNAL_SERVER_ERROR) }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let body = match self.detail {
            Some(detail) => serde_json::json!({ "detail": detail }),
            None => serde_json::json!({ "detail": self.status.canonical_reason() }),
        };

        let mut response = Response::with_status(self.status)
            .header("content-type", "application/json")
            .body(ResponseBody::Bytes(serde_json::to_vec(&body).unwrap()));

        for (name, value) in self.headers {
            response = response.header(name, value);
        }

        response
    }
}
```

### 11.2 Validation Error Response

```rust
/// Validation error (422 Unprocessable Entity)
pub struct ValidationErrors(pub Vec<ValidationError>);

#[derive(Serialize)]
pub struct ValidationError {
    #[serde(rename = "type")]
    pub error_type: &'static str,
    pub loc: Vec<String>,
    pub msg: String,
    pub input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctx: Option<serde_json::Value>,
}

impl IntoResponse for ValidationErrors {
    fn into_response(self) -> Response {
        Response::with_status(StatusCode::UNPROCESSABLE_ENTITY)
            .header("content-type", "application/json")
            .body(ResponseBody::Bytes(
                serde_json::to_vec(&json!({ "detail": self.0 })).unwrap()
            ))
    }
}
```

### 11.3 Result Return Type

```rust
/// Handler can return Result<T, E> where both T and E implement IntoResponse
impl<T: IntoResponse, E: IntoResponse> IntoResponse for Result<T, E> {
    fn into_response(self) -> Response {
        match self {
            Ok(v) => v.into_response(),
            Err(e) => e.into_response(),
        }
    }
}

// Usage
#[get("/items/{id}")]
async fn get_item(
    cx: &Cx,
    Path(id): Path<i64>,
    Depends(db): Depends<DbConn>,
) -> Result<Json<Item>, HttpError> {
    let item = db.find_item(id).await
        .map_err(|_| HttpError::not_found().with_detail("Item not found"))?;
    Ok(Json(item))
}
```

---

## 12. Application Builder

### 12.1 App Configuration

```rust
pub struct App {
    // Routing
    router: Router,

    // OpenAPI
    title: String,
    version: String,
    description: Option<String>,
    openapi_url: Option<String>,

    // State
    state: Extensions,

    // Middleware
    middleware: Vec<Box<dyn Middleware>>,

    // Error handlers
    exception_handlers: HashMap<TypeId, Box<dyn ExceptionHandler>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            router: Router::new(),
            title: "FastAPI".to_string(),
            version: "0.1.0".to_string(),
            description: None,
            openapi_url: Some("/openapi.json".to_string()),
            state: Extensions::new(),
            middleware: Vec::new(),
            exception_handlers: HashMap::new(),
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn state<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.state.insert(state);
        self
    }

    pub fn route(mut self, route: Route) -> Self {
        self.router.add(route);
        self
    }

    pub fn include_router(mut self, router: impl IntoIterator<Item = Route>, prefix: &str) -> Self {
        for mut route in router {
            route.path = format!("{}{}", prefix, route.path).leak();
            self.router.add(route);
        }
        self
    }

    /// Serve the application
    pub async fn serve(self, addr: &str) -> Result<(), Error> {
        let listener = asupersync::net::TcpListener::bind(addr).await?;

        loop {
            let (stream, _addr) = listener.accept().await?;
            let app = self.clone();

            asupersync::spawn(async move {
                if let Err(e) = app.handle_connection(stream).await {
                    eprintln!("Connection error: {}", e);
                }
            });
        }
    }
}
```

### 12.2 Router Builder

```rust
pub struct RouterBuilder {
    prefix: String,
    tags: Vec<String>,
    dependencies: Vec<Box<dyn Dependency>>,
    routes: Vec<Route>,
}

impl RouterBuilder {
    pub fn new() -> Self {
        Self {
            prefix: String::new(),
            tags: Vec::new(),
            dependencies: Vec::new(),
            routes: Vec::new(),
        }
    }

    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    pub fn route(mut self, route: Route) -> Self {
        self.routes.push(route);
        self
    }

    pub fn build(self) -> Vec<Route> {
        self.routes.into_iter().map(|mut route| {
            route.path = format!("{}{}", self.prefix, route.path).leak();
            for tag in &self.tags {
                route.tags = Box::leak(
                    [route.tags, &[tag.as_str()]].concat().into_boxed_slice()
                );
            }
            route
        }).collect()
    }
}
```

---

## Implementation Phases

### Phase 0: Asupersync Foundation ✅ COMPLETE
- [x] Add asupersync workspace dependency (`fastapi_rust-i5m`)
- [x] Design RequestContext wrapping Cx (`fastapi_rust-qhc`)
- [x] Implement RequestContext with Cx integration (`fastapi_rust-dsb`)

**Implemented in `crates/fastapi-core/src/context.rs`:**
```rust
pub struct RequestContext {
    cx: Cx,
    request_id: u64,
}

impl RequestContext {
    pub fn checkpoint(&self) -> Result<(), CancelledError>;
    pub fn masked<F, R>(&self, f: F) -> R;
    pub fn budget(&self) -> Budget;
    pub fn is_cancelled(&self) -> bool;
    pub fn cx(&self) -> &Cx;
}
```

**Re-exports in `lib.rs`:**
```rust
pub use asupersync::{Budget, Cx, Outcome, RegionId, TaskId};
```

### Phase 1: Core Infrastructure
- [x] fastapi-core types (Request, Response, Error) - **implemented**
- [ ] Zero-copy HTTP request line parser (`fastapi_rust-5be`)
- [ ] Zero-copy HTTP header parser (`fastapi_rust-d6d`)
- [ ] HTTP response builder (`fastapi_rust-0qv`)
- [ ] TCP server with asupersync (`fastapi_rust-9ik`)
- [ ] Error handling with Outcome (`fastapi_rust-3f1`)
- [ ] Request timeout via Budget (`fastapi_rust-k9h`)

### Phase 2: Routing & Extractors
- [ ] Design trie-based router (`fastapi_rust-9ll`)
- [ ] Implement route trie (`fastapi_rust-hfk`)
- [ ] Route macros #[get], #[post] (`fastapi_rust-o6k`)
- [ ] FromRequest trait and extractors (`fastapi_rust-940`)

### Phase 3: Validation
- [ ] Validate derive macro
- [ ] ValidJson extractor
- [ ] Error formatting (422 responses)

### Phase 4: Dependency Injection
- [ ] Cx-based dependency storage
- [ ] Depends extractor
- [ ] Dependency overrides for testing

### Phase 5: OpenAPI
- [ ] JsonSchema derive macro
- [ ] OpenAPI document builder
- [ ] /openapi.json endpoint

### Phase 6: Security
- [ ] Auth extractors (Bearer, Basic, ApiKey)
- [ ] Security scheme metadata for OpenAPI

---

## Asupersync Co-Development Coordination

### What fastapi_rust Needs from Asupersync

| Feature | Asupersync Status | fastapi_rust Dependency |
|---------|-------------------|-------------------------|
| Cx (capability context) | ✅ Implemented | Phase 0 - Used in RequestContext |
| Outcome (4-valued result) | ✅ Implemented | Phase 1 - Error handling |
| Budget (time/poll quota) | ✅ Implemented | Phase 1 - Request timeouts |
| Combinators (join, race) | ✅ Implemented | Phase 2+ - Concurrent handlers |
| Lab runtime | ✅ Implemented | Phase 2 - Deterministic testing |
| TcpListener/TcpStream | 🔜 Phase 2 | Phase 1 - HTTP server |
| Graceful shutdown | 🔜 | Phase 6 - Server shutdown |

### Cross-Project Beads

When asupersync implements TCP I/O, create corresponding fastapi_rust beads:
- `fastapi_rust-tcp-server` depends on asupersync TCP implementation
- Test utilities depend on asupersync Lab runtime being complete

---

*Document version: 1.1*
*Created: 2026-01-17*
*Updated: 2026-01-17 - Added Phase 0 completion, bead references*
