# fastapi_rust User Guide

Welcome to the comprehensive user guide for fastapi_rust, an ultra-optimized Rust web framework inspired by Python's FastAPI.

## Chapters

### Core Concepts

1. **[Routing](routing.md)** - Route registration, HTTP methods, and URL patterns
2. **[Request Handling](request-handling.md)** - Working with requests, headers, and body
3. **[Response Building](response-building.md)** - Creating responses, status codes, and content types
4. **[Middleware](middleware.md)** - Built-in middleware and creating custom middleware

### Advanced Topics

5. **[Dependency Injection](dependency-injection.md)** - Depends, scopes, and overrides
6. **[Error Handling](error-handling.md)** - Exceptions and custom error handlers
7. **[Testing](testing.md)** - Using TestClient for comprehensive testing
8. **[Security](security.md)** - CORS, security headers, and authentication

### Reference

9. **[Configuration](configuration.md)** - Application configuration options
10. **[OpenAPI](openapi.md)** - Automatic API documentation generation
11. **[Deployment](deployment.md)** - Running in production

## Quick Reference

### Creating an App

```rust
use fastapi::core::{App, Request, RequestContext, Response, ResponseBody};

fn hello(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    std::future::ready(
        Response::ok().body(ResponseBody::Bytes(b"Hello!".to_vec()))
    )
}

let app = App::builder()
    .get("/", hello)
    .build();
```

### Common Patterns

| Pattern | Example |
|---------|---------|
| Add route | `.get("/path", handler)` |
| Add middleware | `.middleware(MyMiddleware)` |
| Add state | `.state(MyState { ... })` |
| Configure app | `.config(AppConfig::new().name("API"))` |

## Current Status

fastapi_rust is under active development. This guide covers features that are currently implemented and stable.

**Available Now:**
- Basic routing (GET, POST, PUT, DELETE, PATCH)
- Response building with various content types
- Middleware stack (before/after hooks)
- Built-in security middleware
- TestClient for testing
- Application configuration
- Startup/shutdown hooks

**Not Implemented Yet (or still expanding):**
- Fully automatic OpenAPI generation from handler/extractor types (schema/spec types exist; generation coverage is being expanded)

## Prerequisites

- Rust 1.85+ (2024 edition)
- Basic knowledge of async Rust
- Understanding of HTTP concepts

## Getting Help

- [Getting Started Guide](../getting-started.md) - Quick introduction
- [Examples](../../crates/fastapi/examples/) - Working code examples
- [API Documentation](https://docs.rs/fastapi) - Auto-generated docs

---

Select a chapter from the sidebar to begin learning about fastapi_rust.
