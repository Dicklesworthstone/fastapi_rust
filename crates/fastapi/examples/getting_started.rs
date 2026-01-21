//! Getting Started Example
//!
//! This example validates all code snippets from docs/getting-started.md work correctly.
//!
//! Run with: cargo run --example getting_started -p fastapi

use fastapi::core::{
    App,
    AppConfig,
    Request,
    RequestContext,
    Response,
    ResponseBody,
    RequestIdMiddleware,
    SecurityHeaders,
    TestClient,
};

/// Handler for GET /
fn hello(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    std::future::ready(
        Response::ok().body(ResponseBody::Bytes(b"Hello, World!".to_vec()))
    )
}

/// Handler for GET /health
fn health(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    std::future::ready(
        Response::ok().body(ResponseBody::Bytes(b"{\"status\":\"healthy\"}".to_vec()))
    )
}


fn main() {
    println!("Getting Started Guide - Code Validation\n");

    // === Basic App Example ===
    println!("1. Basic app with two routes:");
    let app = App::builder()
        .get("/", hello)
        .get("/health", health)
        .build();

    println!("   Routes: {}", app.route_count());
    let client = TestClient::new(app);

    let response = client.get("/").send();
    println!("   GET / -> {} ({})", response.status().as_u16(), response.text());
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.text(), "Hello, World!");

    let response = client.get("/health").send();
    println!("   GET /health -> {} ({})", response.status().as_u16(), response.text());
    assert_eq!(response.status().as_u16(), 200);

    // === App with Middleware ===
    println!("\n2. App with middleware:");
    let app = App::builder()
        .middleware(RequestIdMiddleware::new())
        .middleware(SecurityHeaders::new())
        .get("/", hello)
        .build();

    let client = TestClient::new(app);
    let response = client.get("/").send();
    println!("   GET / -> {}", response.status().as_u16());
    assert_eq!(response.status().as_u16(), 200);

    // === App with Configuration ===
    println!("\n3. App with configuration:");
    let config = AppConfig::new()
        .name("My API")
        .version("1.0.0")
        .debug(true)
        .max_body_size(10 * 1024 * 1024)
        .request_timeout_ms(30_000);

    let app = App::builder()
        .config(config)
        .get("/", hello)
        .build();

    println!("   App name: {}", app.config().name);
    println!("   Version: {}", app.config().version);
    assert_eq!(app.config().name, "My API");
    assert_eq!(app.config().version, "1.0.0");

    // === 404 for unknown routes ===
    println!("\n4. 404 for unknown routes:");
    let app = App::builder()
        .get("/", hello)
        .build();

    let client = TestClient::new(app);
    let response = client.get("/nonexistent").send();
    println!("   GET /nonexistent -> {}", response.status().as_u16());
    assert_eq!(response.status().as_u16(), 404);

    println!("\nAll getting started examples validated successfully!");
}
