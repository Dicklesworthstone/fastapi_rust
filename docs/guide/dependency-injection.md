# Dependency Injection

> **Status (as of 2026-02-10)**: Type-based dependency injection is implemented via `Depends<T>` where `T: FromDependency`. Overrides, caching, and scopes are supported.

## Concept

Dependency injection (DI) provides reusable components to handlers without tight coupling.

## Dependency Overrides

Override dependencies for testing:

```rust
use fastapi::core::{App, DependencyOverrides};

// Production dependency
struct RealDatabase;

// Test mock
struct MockDatabase;

// Create overrides
let mut overrides = DependencyOverrides::new();
overrides.set::<RealDatabase>(MockDatabase);

// Use in app for testing
let app = App::builder()
    .dependency_overrides(Arc::new(overrides))
    .get("/users", list_users)
    .build();
```

## Depends<T>

Dependencies are resolved from types, not functions. Implement `FromDependency` for any type you want to inject.

```rust
use fastapi::prelude::*;

#[derive(Clone)]
struct Database;

impl FromDependency for Database {
    type Error = HttpError;

    async fn from_dependency(_ctx: &RequestContext, _req: &mut Request) -> Result<Self, HttpError> {
        // Construct once per request by default and cache.
        Ok(Database)
    }
}

#[get("/profile")]
async fn profile(_cx: &Cx, db: Depends<Database>) -> StatusCode {
    let _db: &Database = &db;
    StatusCode::OK
}
```

### Scopes and Caching

By default, dependencies are request-scoped and cached (resolved once per request). You can opt out of caching:

```rust
use fastapi::prelude::*;

#[derive(Clone)]
struct CounterDep;

impl FromDependency for CounterDep {
    type Error = HttpError;
    async fn from_dependency(_ctx: &RequestContext, _req: &mut Request) -> Result<Self, HttpError> {
        Ok(CounterDep)
    }
}

async fn handler(_cx: &Cx, _dep: Depends<CounterDep, NoCache>) -> StatusCode {
    StatusCode::OK
}
```


## Dependencies with teardown

Implement `FromDependencyWithCleanup` and extract `DependsCleanup<T>` when a
dependency acquires a resource that must be released. Return the value and an
optional `CleanupFn` from `setup`. These types are exported by
`fastapi_rust::prelude::*`.

Request-scoped cached dependencies register their callback once. Callbacks run in
reverse registration order, after middleware, response body consumption, and
background tasks. The HTTP/1.1 and HTTP/2 servers, `TestClient`, and `TestServer`
perform this finalization. Completed WebSocket handlers finalize their own context.
HTTP response-write errors still attempt cleanup and preserve the write error;
background tasks run only after a successful write. HTTP/1.1 handler timeouts retain
and finalize callbacks that were registered before the handler was abandoned.

`TestClient` and `TestServer` collect finite streams before releasing dependencies.
Their synchronous calls wait for stream completion; use a live async client for
infinite streams. If you call `App::handle` directly, consume the response, take and
execute background tasks with `App::take_background_tasks`, then await
`ctx.cleanup_stack().run_cleanups()` yourself.

Cleanup callback creation and polling panics are caught so remaining callbacks are
attempted. Handler, body-stream, background-task, or destructor panics and dropping
the entire request or cleanup future can bypass teardown. Setup interrupted before
callback registration is not covered. Cleanup callbacks have no automatic timeout,
and request/function scopes share the same completion stack.

## Next Steps

- [Configuration](configuration.md) - Configure application state
- [Testing](testing.md) - Use overrides in tests
