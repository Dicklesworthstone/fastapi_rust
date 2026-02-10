# OpenAPI Documentation

> **Status (as of 2026-02-10)**: OpenAPI schema/spec types exist (`fastapi-openapi`), `#[derive(JsonSchema)]` exists, and `App` can serve an OpenAPI JSON endpoint. Automatic, full-coverage generation from all handler/extractor types is still being expanded.

## Concept

OpenAPI (formerly Swagger) provides machine-readable API documentation that can generate interactive UIs.

## OpenAPI Types

The `fastapi-openapi` crate provides OpenAPI 3.1 types:

```rust
use fastapi::openapi::{OpenApi, OpenApiBuilder, Info, Server};

let spec = OpenApiBuilder::new()
    .info(Info {
        title: "My API".into(),
        version: "1.0.0".into(),
        description: Some("A sample API".into()),
        ..Default::default()
    })
    .server(Server {
        url: "https://api.example.com".into(),
        description: Some("Production".into()),
        ..Default::default()
    })
    .build();
```

## Not Implemented Yet (Or Still Expanding)

OpenAPI generation coverage is currently incomplete for the full framework surface (all extractors, responses, and security flows).

```rust
#[derive(JsonSchema)]
struct User {
    id: i64,
    name: String,
    email: String,
}

// JSON Schema is derived from Rust types and can be registered in OpenAPI components.
```

### Areas Being Expanded

- Route-to-operation mapping from registered handlers (params, request bodies, responses)
- Request/response schema coverage and examples
- Security scheme integration and per-route requirements

## Current Workarounds

Manually define OpenAPI spec and serve it:

```rust
fn openapi_spec(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    let spec = r#"{
        "openapi": "3.1.0",
        "info": { "title": "My API", "version": "1.0.0" },
        "paths": {
            "/": { "get": { "summary": "Home" } }
        }
    }"#;

    std::future::ready(
        Response::ok()
            .header("Content-Type", "application/json")
            .body(ResponseBody::Bytes(spec.as_bytes().to_vec()))
    )
}

let app = App::builder()
    .get("/openapi.json", openapi_spec)
    .build();
```

## Next Steps

- [Routing](routing.md) - Define API routes
- [Response Building](response-building.md) - Document response types
