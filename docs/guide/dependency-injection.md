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

## Next Steps

- [Configuration](configuration.md) - Configure application state
- [Testing](testing.md) - Use overrides in tests
