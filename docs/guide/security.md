# Security

Secure your fastapi_rust application with built-in middleware and best practices.

## Security Headers

Add security headers to all responses:

```rust
use fastapi::core::{SecurityHeaders, SecurityHeadersConfig, XFrameOptions, ReferrerPolicy};

// Default security headers
let app = App::builder()
    .middleware(SecurityHeaders::new())
    .build();

// Strict configuration
let app = App::builder()
    .middleware(SecurityHeaders::strict())
    .build();

// Custom configuration
let config = SecurityHeadersConfig::new()
    .x_frame_options(XFrameOptions::Deny)
    .referrer_policy(ReferrerPolicy::StrictOrigin)
    .content_security_policy("default-src 'self'")
    .hsts(31536000, true, false);  // 1 year, includeSubDomains, no preload

let app = App::builder()
    .middleware(SecurityHeaders::with_config(config))
    .build();
```

### Default Headers

| Header | Default Value |
|--------|--------------|
| X-Content-Type-Options | nosniff |
| X-Frame-Options | DENY |
| X-XSS-Protection | 0 |
| Referrer-Policy | strict-origin-when-cross-origin |

### Optional Headers

Configure these for production:

| Header | Purpose |
|--------|---------|
| Content-Security-Policy | Control resource loading |
| Strict-Transport-Security | Force HTTPS |
| Permissions-Policy | Control browser features |

## CORS

Configure Cross-Origin Resource Sharing:

```rust
use fastapi::core::{Cors, CorsConfig};

// Development: Allow all
let app = App::builder()
    .middleware(Cors::permissive())
    .build();

// Production: Specific origins
let config = CorsConfig::new()
    .allow_origins(vec![
        "https://example.com".into(),
        "https://app.example.com".into(),
    ])
    .allow_methods(vec![
        "GET".into(),
        "POST".into(),
        "PUT".into(),
        "DELETE".into(),
    ])
    .allow_headers(vec![
        "Content-Type".into(),
        "Authorization".into(),
    ])
    .allow_credentials(true)
    .max_age(3600);

let app = App::builder()
    .middleware(Cors::new(config))
    .build();
```

## Authentication Middleware

Create an authentication middleware:

```rust
struct ApiKeyAuth {
    expected_key: String,
}

impl Middleware for ApiKeyAuth {
    fn name(&self) -> &'static str { "ApiKeyAuth" }

    fn before<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> BoxFuture<'a, ControlFlow<Response>> {
        let expected = self.expected_key.clone();
        Box::pin(async move {
            match req.header("X-API-Key") {
                Some(key) if key == expected.as_bytes() => {
                    ControlFlow::Continue
                }
                _ => ControlFlow::Stop(
                    Response::with_status(StatusCode::UNAUTHORIZED)
                        .header("WWW-Authenticate", "API-Key")
                ),
            }
        })
    }
}

let app = App::builder()
    .middleware(ApiKeyAuth { expected_key: "secret".into() })
    .build();
```

## Best Practices

### 1. Always Use HTTPS in Production

```rust
// Add HSTS header
let config = SecurityHeadersConfig::new()
    .hsts(31536000, true, true);  // 1 year, subdomains, preload
```

### 2. Validate All Input

```rust
fn handler(ctx: &RequestContext, req: &mut Request) -> ... {
    // Validate before processing
    let body = req.body();
    if body.len() > MAX_BODY_SIZE {
        return Response::with_status(StatusCode::PAYLOAD_TOO_LARGE);
    }
    // Continue processing
}
```

### 3. Use Specific CORS Origins

```rust
// Good: Specific origins
.allow_origins(vec!["https://myapp.com".into()])

// Avoid in production: Allow all
.allow_origins(vec!["*".into()])
```

### 4. Don't Expose Internal Errors

```rust
// Good: Generic message
HttpError::internal("An error occurred")

// Bad: Expose details
HttpError::internal(&format!("Database error: {}", db_error))
```

### 5. Use Request IDs for Tracing

```rust
let app = App::builder()
    .middleware(RequestIdMiddleware::new())
    .middleware(AuthMiddleware::new())
    .build();

// Auth middleware can log request IDs for audit trails
```

## Not Built In Yet (Or App-Specific)

- OAuth2/JWT validation logic is application-specific (extractors provide credentials, apps validate)
- CSRF protection primitives are not provided as a first-class built-in yet
- Rate limiting support depends on which middleware you enable/configure (and is still being expanded)

## Next Steps

- [Middleware](middleware.md) - Build security middleware
- [Error Handling](error-handling.md) - Secure error responses
