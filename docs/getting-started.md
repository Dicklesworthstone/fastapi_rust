# Getting Started with fastapi_rust

This guide walks you through creating your first fastapi_rust application in under 5 minutes.

## Prerequisites

- **Rust 1.85+** (2024 edition)
- **Cargo** (comes with Rust)

Check your Rust version:

```bash
rustc --version
# Should show: rustc 1.85.0 or higher
```

If you need to update Rust:

```bash
rustup update stable
```

## Create a New Project

Create a new Rust project:

```bash
cargo new my_api
cd my_api
```

## Add Dependencies

Add fastapi_rust to your `Cargo.toml`:

```toml
[package]
name = "my_api"
version = "0.1.0"
edition = "2024"

[dependencies]
fastapi = { git = "https://github.com/Dicklesworthstone/fastapi_rust.git" }
```

> **Note**: Once published to crates.io, you'll be able to use:
> ```toml
> fastapi = "0.1"
> ```

## Your First Handler

Replace the contents of `src/main.rs`:

```rust
//! My First fastapi_rust API

use fastapi::core::{
    App,
    Request,
    RequestContext,
    Response,
    ResponseBody,
    TestClient,
};

/// Handler for GET /
///
/// Returns a friendly greeting.
fn hello(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    std::future::ready(
        Response::ok().body(ResponseBody::Bytes(b"Hello, World!".to_vec()))
    )
}

/// Handler for GET /health
///
/// Returns API health status.
fn health(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    std::future::ready(
        Response::ok().body(ResponseBody::Bytes(b"{\"status\":\"healthy\"}".to_vec()))
    )
}

fn main() {
    // Build the application with routes
    let app = App::builder()
        .get("/", hello)
        .get("/health", health)
        .build();

    println!("API configured with {} routes", app.route_count());

    // Test the API using the built-in TestClient
    let client = TestClient::new(app);

    // Test the hello endpoint
    let response = client.get("/").send();
    println!("GET / -> {}", response.status().as_u16());
    println!("Body: {}", response.text());

    // Test the health endpoint
    let response = client.get("/health").send();
    println!("\nGET /health -> {}", response.status().as_u16());
    println!("Body: {}", response.text());
}
```

## Run Your API

```bash
cargo run
```

Expected output:

```
API configured with 2 routes
GET / -> 200
Body: Hello, World!

GET /health -> 200
Body: {"status":"healthy"}
```

## Adding More Routes

fastapi_rust supports all HTTP methods:

```rust
let app = App::builder()
    .get("/users", list_users)      // GET /users
    .post("/users", create_user)    // POST /users
    .put("/users", update_users)    // PUT /users
    .delete("/users", delete_users) // DELETE /users
    .patch("/users", patch_users)   // PATCH /users
    .build();
```

If you use the route attribute macros (e.g., `#[get("/users/{id}")]`), register the generated route entry:

```rust
let app = App::builder()
    .route_entry(get_user_route())
    .build();
```

## Adding Middleware

Apply middleware to your application:

```rust
use fastapi::core::{App, RequestIdMiddleware, SecurityHeaders};

let app = App::builder()
    // Add request ID to every request
    .middleware(RequestIdMiddleware::new())
    // Add security headers to responses
    .middleware(SecurityHeaders::new())
    .get("/", hello)
    .build();
```

## Application Configuration

Configure your application:

```rust
use fastapi::core::{App, AppConfig};

let config = AppConfig::new()
    .name("My API")
    .version("1.0.0")
    .debug(true)
    .max_body_size(10 * 1024 * 1024)  // 10MB
    .request_timeout_ms(30_000);       // 30 seconds

let app = App::builder()
    .config(config)
    .get("/", hello)
    .build();
```

## Testing Your API

fastapi_rust includes a powerful `TestClient` for testing:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use fastapi::core::TestClient;

    #[test]
    fn test_hello_endpoint() {
        let app = App::builder()
            .get("/", hello)
            .build();

        let client = TestClient::new(app);
        let response = client.get("/").send();

        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.text(), "Hello, World!");
    }

    #[test]
    fn test_not_found() {
        let app = App::builder()
            .get("/", hello)
            .build();

        let client = TestClient::new(app);
        let response = client.get("/nonexistent").send();

        assert_eq!(response.status().as_u16(), 404);
    }
}
```

Run tests:

```bash
cargo test
```

## Project Structure

Recommended project structure for larger applications:

```
my_api/
├── Cargo.toml
├── src/
│   ├── main.rs           # Entry point
│   ├── lib.rs            # Library exports
│   ├── routes/           # Route handlers
│   │   ├── mod.rs
│   │   ├── users.rs
│   │   └── health.rs
│   ├── middleware/       # Custom middleware
│   │   └── mod.rs
│   └── models/           # Data models
│       └── mod.rs
└── tests/
    └── integration.rs    # Integration tests
```

## Next Steps

- [Hello World Example](../crates/fastapi/examples/hello_world.rs) - See a working example
- [Routing Guide](./guide/routing.md) - Organize routes into modules
- [Middleware Guide](./guide/middleware.md) - Build custom middleware
- [Testing Guide](./guide/testing.md) - Advanced testing patterns

## Getting Help

- GitHub Issues: Report bugs and request features
- API Documentation: `cargo doc --open`

## Current Status

fastapi_rust is under active development. The current version provides:

- Application builder with fluent API
- Route registration for all HTTP methods
- Middleware stack with before/after hooks
- Built-in security headers middleware
- Request ID middleware
- CORS middleware
- Comprehensive TestClient for testing
- Path parameter extraction
- JSON extraction (`Json<T>`) and validation error formatting (422)
- Query string extraction (`Query<T>`)
- Header and cookie extractors
- Dependency injection (`Depends<T>`, overrides, caching, scopes)
- HTTP/1.1 parser + TCP server (`serve(app, addr)`) built on asupersync
- OpenAPI: schema/spec types and route-aware stubs (generation is still being expanded)
