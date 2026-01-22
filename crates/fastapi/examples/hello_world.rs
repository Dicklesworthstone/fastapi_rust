//! Hello World Example - Minimal fastapi_rust Application
//!
//! This example demonstrates the most basic fastapi_rust setup:
//! - Creating an application with a single GET endpoint
//! - Defining a simple request handler
//! - Testing with the built-in TestClient
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example hello_world
//! ```
//!
//! # Expected Output
//!
//! ```text
//! GET / -> 200 OK
//! Response: Hello, World!
//! ```

// Import the core types from fastapi
use fastapi::core::{
    App,            // The application builder and container
    Request,        // Incoming HTTP request
    RequestContext, // Request context (contains Cx, request ID, etc.)
    Response,       // HTTP response to send back
    ResponseBody,   // Response body types
    TestClient,     // Built-in test client for making requests
};

/// A simple request handler that returns "Hello, World!"
///
/// # Parameters
///
/// - `_ctx`: The request context (unused in this simple example)
/// - `_req`: The incoming request (unused - we return the same response for any request)
///
/// # Returns
///
/// An HTTP 200 OK response with "Hello, World!" as the body.
///
/// # Note
///
/// Handlers in fastapi_rust are functions that take a RequestContext
/// and mutable Request, and return a Response. They can be sync or async.
fn hello_handler(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    // Create a 200 OK response with a plain text body
    std::future::ready(Response::ok().body(ResponseBody::Bytes(b"Hello, World!".to_vec())))
}

fn main() {
    println!("fastapi_rust Hello World Example");
    println!("================================\n");

    // Build the application
    //
    // App::builder() creates a new application builder that lets you:
    // - Add routes for different HTTP methods
    // - Configure middleware
    // - Set application state
    // - Define exception handlers
    let app = App::builder()
        // Register a GET handler for the root path "/"
        //
        // This is equivalent to:
        //   @app.get("/")
        //   def hello():
        //       return "Hello, World!"
        // in Python FastAPI
        .get("/", hello_handler)
        // Build the final immutable App
        .build();

    println!("App created with {} route(s)\n", app.route_count());

    // Create a test client to make requests to our app
    //
    // TestClient wraps any Handler (including App) and provides
    // a convenient API for making HTTP requests in tests.
    let client = TestClient::new(app);

    // Make a GET request to "/"
    println!("Making request: GET /");
    let response = client.get("/").send();

    // Check the response
    println!(
        "GET / -> {} {}",
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    println!("Response: {}\n", response.text());

    // Verify success
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(response.text(), "Hello, World!");

    // Try a path that doesn't exist - should get 404
    println!("Making request: GET /not-found");
    let response = client.get("/not-found").send();
    println!(
        "GET /not-found -> {} {}",
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    assert_eq!(response.status().as_u16(), 404);

    println!("\nAll assertions passed!");
}
