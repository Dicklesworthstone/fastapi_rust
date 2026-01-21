# Deployment

> **Status**: Full HTTP server integration is coming soon. This chapter covers preparation for production.

## Current State

fastapi_rust currently provides application building and testing capabilities. Full HTTP server integration is in development.

## Production Checklist

### Security

- [ ] Enable HTTPS (via reverse proxy or when server is ready)
- [ ] Configure security headers
- [ ] Set specific CORS origins
- [ ] Use environment variables for secrets

### Configuration

```rust
// Production configuration
let config = AppConfig::new()
    .name("Production API")
    .debug(false)  // Disable debug in production
    .max_body_size(1024 * 1024)  // 1 MB limit
    .request_timeout_ms(30_000);
```

### Error Handling

```rust
// Don't expose internal errors
let app = App::builder()
    .exception_handler::<std::io::Error>(|_err| {
        // Log error internally
        // eprintln!("IO error: {:?}", err);

        // Return generic response
        Response::with_status(StatusCode::INTERNAL_SERVER_ERROR)
            .body("Internal error".into())
    })
    .build();
```

## Build Optimization

### Release Build

```bash
cargo build --release
```

### Profile Settings

In `Cargo.toml`:

```toml
[profile.release]
lto = true
codegen-units = 1
panic = 'abort'
```

## Monitoring

### Request IDs

Always include request IDs for tracing:

```rust
let app = App::builder()
    .middleware(RequestIdMiddleware::new())
    .build();
```

### Logging

Log requests and errors:

```rust
impl Middleware for LoggingMiddleware {
    fn before(&self, ctx: &RequestContext, req: &mut Request) -> ... {
        println!("[{}] {} {}", ctx.request_id(), req.method(), req.path());
        ControlFlow::Continue
    }

    fn after(&self, ctx: &RequestContext, _req: &Request, response: Response) -> ... {
        println!("[{}] {}", ctx.request_id(), response.status());
        response
    }
}
```

## Coming Soon

- **HTTP Server**: Native server binding
- **Graceful Shutdown**: Handle SIGTERM cleanly
- **Health Checks**: Kubernetes-ready endpoints
- **Metrics**: Prometheus integration

## Reverse Proxy Setup

While waiting for native server support, you can test with a reverse proxy:

### nginx Example

```nginx
server {
    listen 80;
    server_name api.example.com;

    location / {
        # When server is ready, proxy to fastapi_rust
        proxy_pass http://127.0.0.1:8000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

## Container Deployment

### Dockerfile

```dockerfile
FROM rust:1.85 as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/my-api /usr/local/bin/
CMD ["my-api"]
```

## Next Steps

- [Configuration](configuration.md) - Production settings
- [Security](security.md) - Secure your deployment
