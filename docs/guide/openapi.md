# OpenAPI Documentation

`App` serves an OpenAPI 3.1 document from registered route metadata. Route macros
connect JSON models and responses, named query/header fields, and typed path
parameters to this document. Full coverage of every extractor, serde representation
and security flow remains incomplete; see the parity matrix and Beads.

## Concept

OpenAPI (formerly Swagger) provides machine-readable API documentation that can generate interactive UIs.

## OpenAPI Types

The `fastapi-openapi` crate provides OpenAPI 3.1 types:

```rust
use fastapi_rust::openapi::OpenApiBuilder;

let spec = OpenApiBuilder::new("My API", "1.0.0")
    .description("A sample API")
    .server("https://api.example.com", Some("Production".into()))
    .build();
```

## Typed Path Parameters

```rust
use fastapi_rust::prelude::*;

#[get("/users/{id}")]
async fn get_user(_cx: &Cx, id: Path<i64>) -> Json<i64> {
    Json(id.0)
}

let app = App::builder()
    .openapi(fastapi_rust::OpenApiConfig::new())
    .route_entry(get_user_route())
    .build();
```

The served `/openapi.json` describes `id` as a required integer with `int64`
format. For multiple placeholders, use one `Path<(String, i64)>` whose elements
follow route order, or a named model deriving both `Deserialize` and `JsonSchema`.
Named fields match serialized names, including symmetric serde renames and raw
Rust identifiers such as `r#type`. Separate scalar `Path` arguments are rejected.

Optional path extractors still describe required URL placeholders. Existing
`Option<Path<T>>` extraction returns `None` for malformed values; it does not
promise a 422 response. Optional named models omit JSON-null unions from path
parameter schemas because URL values are text.

Explicit `:int`, `:float` and `:uuid` converter schemas remain authoritative.
Handler-specific narrowing on those converters is not fully represented. Literal
tuples are supported; tuple aliases, recursive/custom schema references,
directional serde attributes and arbitrary extractor metadata remain limits.
Manually created `RouteEntry` values can use `.path_schema::<T>(&["id"])` while
retaining parameter descriptions and examples from their router metadata.

## Missing / In Progress

OpenAPI generation coverage is currently incomplete for the full framework surface (all extractors, responses, and security flows). The concrete gap list lives under `bd-uz2s`.

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
