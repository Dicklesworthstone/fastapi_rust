# fastapi_rust Examples

This directory contains example applications demonstrating fastapi_rust features.

## hello_world

The minimal "Hello, World!" example showing basic setup and usage.

### What it demonstrates

- Creating an application with `App::builder()`
- Defining a simple request handler
- Registering routes with `.get()`
- Testing with `TestClient`

### Running

```bash
cargo run --example hello_world -p fastapi
```

## getting_started

Validates all code snippets from the [Getting Started Guide](../../../docs/getting-started.md).

### What it demonstrates

- Basic app with multiple routes
- Adding middleware (RequestIdMiddleware, SecurityHeaders)
- Application configuration
- 404 handling for unknown routes

### Running

```bash
cargo run --example getting_started -p fastapi
```

## More Examples

Additional examples coming soon:

- `json_api` - JSON request/response handling
- `middleware` - Custom middleware
- `state` - Application state management
- `validation` - Input validation with derive macros
