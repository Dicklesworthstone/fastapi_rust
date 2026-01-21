# Dependency Injection

> **Status**: Dependency injection is partially implemented. Basic overrides work, with full `Depends` support coming soon.

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

## Coming Soon

The full `Depends` system is planned:

```rust
// Planned syntax (not yet implemented)
async fn get_db() -> Database {
    Database::connect().await
}

async fn get_current_user(db: Depends<Database>) -> User {
    // Use db to fetch user
}

#[get("/profile")]
async fn profile(
    user: Depends<get_current_user>,
    db: Depends<get_db>,
) -> Json<Profile> {
    // Both dependencies injected
}
```

### Planned Features

- **Automatic Resolution**: Dependencies resolved from type signatures
- **Scopes**: Request, application, or custom scopes
- **Caching**: Avoid duplicate resolution within scope
- **Nested Dependencies**: Dependencies can have dependencies

## Current Workarounds

Until full DI is available, use application state:

```rust
struct Services {
    db: DatabasePool,
    cache: CacheClient,
}

let app = App::builder()
    .state(Services {
        db: DatabasePool::new(),
        cache: CacheClient::new(),
    })
    .get("/users", list_users)
    .build();

// In handler, access via state
fn list_users(ctx: &RequestContext, req: &mut Request) -> ... {
    // Access services from app state
}
```

## Next Steps

- [Configuration](configuration.md) - Configure application state
- [Testing](testing.md) - Use overrides in tests
