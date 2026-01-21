# Middleware

Middleware intercepts requests and responses, enabling cross-cutting concerns like logging, authentication, and security headers.

## Concept

Middleware wraps your handlers, running code before and/or after each request:

```
Request → [Before Hooks] → Handler → [After Hooks] → Response
```

## Adding Middleware

Add middleware to your application with `.middleware()`:

```rust
use fastapi::core::{App, RequestIdMiddleware, SecurityHeaders};

let app = App::builder()
    .middleware(RequestIdMiddleware::new())  // First middleware
    .middleware(SecurityHeaders::new())      // Second middleware
    .get("/", handler)
    .build();
```

### Execution Order

Middleware runs in registration order:
1. **Before hooks**: First registered runs first
2. **After hooks**: Last registered runs first (LIFO)

## Built-in Middleware

### RequestIdMiddleware

Assigns a unique ID to each request:

```rust
use fastapi::core::{RequestIdMiddleware, RequestIdConfig};

// Default configuration
let mw = RequestIdMiddleware::new();

// Custom configuration
let config = RequestIdConfig::new()
    .header_name("X-Trace-ID")
    .prefix("trace-");

let mw = RequestIdMiddleware::with_config(config);
```

The request ID is available in handlers via `ctx.request_id()`.

### SecurityHeaders

Adds security headers to responses:

```rust
use fastapi::core::{SecurityHeaders, SecurityHeadersConfig};

// Default headers (X-Content-Type-Options, X-Frame-Options, etc.)
let mw = SecurityHeaders::new();

// Strict configuration with HSTS and CSP
let mw = SecurityHeaders::strict();

// Custom configuration
let config = SecurityHeadersConfig::new()
    .content_security_policy("default-src 'self'")
    .hsts(31536000, true, false)  // max-age, includeSubDomains, preload
    .referrer_policy(ReferrerPolicy::StrictOrigin);

let mw = SecurityHeaders::with_config(config);
```

Default headers added:
- `X-Content-Type-Options: nosniff`
- `X-Frame-Options: DENY`
- `X-XSS-Protection: 0`
- `Referrer-Policy: strict-origin-when-cross-origin`

### CORS Middleware

Enable Cross-Origin Resource Sharing:

```rust
use fastapi::core::{Cors, CorsConfig};

// Allow all origins (development only!)
let mw = Cors::permissive();

// Production configuration
let config = CorsConfig::new()
    .allow_origins(vec!["https://example.com".into()])
    .allow_methods(vec!["GET".into(), "POST".into()])
    .allow_headers(vec!["Content-Type".into()])
    .max_age(3600);

let mw = Cors::new(config);
```

## Creating Custom Middleware

Implement the `Middleware` trait:

```rust
use fastapi::core::{
    BoxFuture, ControlFlow, Middleware, Request, RequestContext, Response
};

struct LoggingMiddleware;

impl Middleware for LoggingMiddleware {
    fn name(&self) -> &'static str {
        "LoggingMiddleware"
    }

    fn before<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow<Response>> {
        Box::pin(async move {
            println!("[{}] {} {}", ctx.request_id(), req.method(), req.path());
            ControlFlow::Continue
        })
    }

    fn after<'a>(
        &'a self,
        ctx: &'a RequestContext,
        _req: &'a Request,
        response: Response,
    ) -> BoxFuture<'a, Response> {
        Box::pin(async move {
            println!("[{}] Response: {}", ctx.request_id(), response.status());
            response
        })
    }
}
```

### Before Hook

The `before` hook runs before the handler. Return:
- `ControlFlow::Continue` - Proceed to handler
- `ControlFlow::Stop(response)` - Short-circuit with response

```rust
fn before<'a>(
    &'a self,
    ctx: &'a RequestContext,
    req: &'a mut Request,
) -> BoxFuture<'a, ControlFlow<Response>> {
    Box::pin(async move {
        // Check authentication
        if !is_authenticated(req) {
            return ControlFlow::Stop(Response::unauthorized());
        }
        ControlFlow::Continue
    })
}
```

### After Hook

The `after` hook runs after the handler (or after a short-circuit):

```rust
fn after<'a>(
    &'a self,
    _ctx: &'a RequestContext,
    _req: &'a Request,
    mut response: Response,
) -> BoxFuture<'a, Response> {
    Box::pin(async move {
        // Add a custom header
        response.add_header("X-Processed-By", "my-middleware");
        response
    })
}
```

## Middleware Patterns

### Authentication Guard

```rust
struct AuthMiddleware {
    api_key: String,
}

impl Middleware for AuthMiddleware {
    fn name(&self) -> &'static str { "AuthMiddleware" }

    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow<Response>> {
        let expected_key = self.api_key.clone();
        Box::pin(async move {
            match req.header("X-API-Key") {
                Some(key) if key == expected_key.as_bytes() => {
                    ControlFlow::Continue
                }
                _ => ControlFlow::Stop(
                    Response::with_status(StatusCode::UNAUTHORIZED)
                ),
            }
        })
    }
}
```

### Response Timing

```rust
use std::time::Instant;
use std::sync::atomic::{AtomicU64, Ordering};

struct TimingMiddleware {
    // Store start time per request (simplified)
}

impl Middleware for TimingMiddleware {
    fn name(&self) -> &'static str { "TimingMiddleware" }

    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow<Response>> {
        Box::pin(async move {
            // In practice, store Instant::now() in request extensions
            ControlFlow::Continue
        })
    }

    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a Request,
        mut response: Response,
    ) -> BoxFuture<'a, Response> {
        Box::pin(async move {
            // Calculate duration and add header
            response.add_header("X-Response-Time", "42ms");
            response
        })
    }
}
```

### Conditional Middleware

Apply middleware only to certain paths:

```rust
use fastapi::core::PathPrefixFilter;

// Only apply to /api/* paths
let api_logging = PathPrefixFilter::new("/api", LoggingMiddleware);
```

## Middleware Stack

Multiple middleware form a stack:

```rust
let app = App::builder()
    .middleware(RequestIdMiddleware::new())  // 1st before, last after
    .middleware(LoggingMiddleware)           // 2nd before, 2nd-last after
    .middleware(AuthMiddleware::new(key))    // 3rd before, 1st after
    .get("/", handler)
    .build();
```

Execution for a request:
1. RequestIdMiddleware.before()
2. LoggingMiddleware.before()
3. AuthMiddleware.before()
4. handler()
5. AuthMiddleware.after()
6. LoggingMiddleware.after()
7. RequestIdMiddleware.after()

If `AuthMiddleware.before()` returns `Stop(response)`:
1. RequestIdMiddleware.before()
2. LoggingMiddleware.before()
3. AuthMiddleware.before() → Returns Stop
4. LoggingMiddleware.after() (on short-circuit response)
5. RequestIdMiddleware.after()

## Pitfalls to Avoid

### Heavy Computation in Middleware

Keep middleware lightweight:

```rust
// Bad: Expensive operation in middleware
fn before(...) {
    let result = expensive_database_query();  // Blocks all requests!
    // ...
}

// Good: Defer expensive operations or use async properly
fn before(...) {
    // Quick validation only
    // Heavy lifting in the handler
}
```

### Order-Dependent Bugs

Be aware of middleware order:

```rust
// Bug: Auth runs before RequestId is set
let app = App::builder()
    .middleware(AuthMiddleware::new())       // Can't log request ID!
    .middleware(RequestIdMiddleware::new())
    .build();

// Fixed: RequestId first
let app = App::builder()
    .middleware(RequestIdMiddleware::new())  // Request ID available
    .middleware(AuthMiddleware::new())       // Can use request ID
    .build();
```

## Next Steps

- [Testing](testing.md) - Test your middleware
- [Security](security.md) - Security-focused middleware patterns
