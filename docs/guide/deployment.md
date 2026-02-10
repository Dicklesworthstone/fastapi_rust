# Deployment

> **Status (as of 2026-02-10)**: `fastapi_rust` includes an HTTP/1.1 TCP server built on **asupersync** (`fastapi_http::TcpServer` and `fastapi_rust::serve`). This chapter covers practical deployment guidance and the remaining hardening work.

## Current State

fastapi_rust provides:

- An application runtime (`App`) with routing, middleware, extraction, DI, and error formatting
- An HTTP/1.1 parser + TCP server (`serve(app, addr)`)

The server surface is usable, but production hardening (resource limits, metrics, signal integration, load/perf characterization) is ongoing.

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

## Roadmap Items (Hardening)

- Structured metrics export (Prometheus/OpenTelemetry style)
- Signal-driven graceful shutdown wiring (SIGTERM/SIGINT)
- Load testing, p95 latency characterization, and perf tuning
- More complete observability integration (spans/log sinks)

## Reverse Proxy Setup

If you deploy behind a reverse proxy (recommended), configure standard forwarded headers and TLS termination there:

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
