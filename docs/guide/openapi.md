# OpenAPI Documentation

`App` serves an OpenAPI 3.1 document from registered route metadata. Route macros
connect JSON models and responses, named query/header fields, typed path
parameters, and built-in authentication metadata to this document. Full coverage of every extractor, serde representation
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

## Authentication Metadata

`BearerToken`, `BasicAuth`, and `OAuth2PasswordBearer` publish matching
`components.securitySchemes` and operation `security` requirements when used
in macro handlers. Qualified types and aliases work through
`FromRequest::security_metadata()`. Default names are the extractor names;
Bearer tokens have no assumed token format. OAuth2 password metadata uses
`/token` with an empty scope map. Applications provide their token endpoint.

Multiple required extractors appear together in one requirement object (AND).
Optional-only extractors add an empty-object alternative for anonymous access.
An optional extractor cannot weaken a required extractor or an explicitly
declared route requirement. Existing optional extraction still turns every
extraction error into `None`, including malformed credentials.

Manual router requirements are alternatives (OR), with their scopes preserved.
Register their definitions on the runtime route entry using
`.security_scheme(name, fastapi_rust::openapi::SecurityScheme)`. An explicit
definition overrides that entry's inferred definition of the same name; all
entries sharing a name must agree. `OpenApiBuilder::security_scheme` supports
standalone document construction. Identical definitions deduplicate; conflicting
definitions and unregistered requirement names fail during document construction.

Security declarations describe authentication; handlers still validate opaque
tokens, passwords and authorization scopes. Custom token URLs, refresh URLs and
scope descriptions can be supplied with an explicit `SecurityScheme::OAuth2`
definition. `OAuth2PasswordBearerConfig` is not automatically inspected. Other
OAuth flows, middleware policies and dependency security are not inferred.

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
- Security metadata beyond the built-in extractors and OAuth2 password flow

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
