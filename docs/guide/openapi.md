# OpenAPI Documentation

> **Status**: The OpenAPI crate exists with types defined, but automatic schema generation is not yet integrated with the App.

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

## Coming Soon

Automatic schema generation from types:

```rust
// Planned syntax (not yet implemented)
#[derive(JsonSchema)]
struct User {
    id: i64,
    name: String,
    email: String,
}

// Schema automatically derived
// GET /openapi.json returns full specification
```

### Planned Features

- **Automatic Route Discovery**: Routes added to spec automatically
- **Type-Driven Schemas**: Generate from Rust types
- **Request/Response Docs**: Document inputs and outputs
- **Interactive UI**: Swagger UI / ReDoc integration

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
