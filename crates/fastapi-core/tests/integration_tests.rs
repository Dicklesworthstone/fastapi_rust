//! Integration tests demonstrating the multi-component test framework.
//!
//! These tests verify that the full application stack works together:
//! - Parser -> Router -> Handler -> Response
//! - Middleware chain execution
//! - State management across requests
//! - Error propagation through layers

#![allow(unused_must_use, dead_code)]

use std::sync::Arc;

use fastapi_core::{
    App, BoxFuture, FixtureGuard, Handler, IntegrationTest, Method, Request, RequestContext,
    Response, ResponseBody, StatusCode, TestClient, TestFixture,
};

// =============================================================================
// Test Fixtures
// =============================================================================

/// A simple fixture that tracks setup/teardown calls.
struct CounterFixture {
    setup_count: u32,
    request_count: u32,
}

impl TestFixture for CounterFixture {
    fn setup() -> Self {
        CounterFixture {
            setup_count: 1,
            request_count: 0,
        }
    }

    fn teardown(&mut self) {
        // In a real test, this would clean up database records, etc.
        assert!(self.setup_count > 0, "Teardown called without setup");
    }
}

/// Fixture for tracking test user data.
struct UserFixture {
    user_id: i64,
    username: String,
}

impl TestFixture for UserFixture {
    fn setup() -> Self {
        // In a real test, this would create a user in a test database
        UserFixture {
            user_id: 42,
            username: "test_user".to_string(),
        }
    }

    fn teardown(&mut self) {
        // In a real test, this would delete the test user
    }
}

// =============================================================================
// Test Handlers
// =============================================================================

/// Handler wrapper that creates a response from a closure.
struct FnHandler<F: Fn() -> Response + Send + Sync>(F);

impl<F: Fn() -> Response + Send + Sync> Handler for FnHandler<F> {
    fn call<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a mut Request,
    ) -> BoxFuture<'a, Response> {
        let resp = (self.0)();
        Box::pin(async move { resp })
    }
}

// =============================================================================
// Integration Tests
// =============================================================================

#[test]
fn test_fixture_setup_and_teardown() {
    // Verify that fixtures are properly set up and torn down
    let guard = FixtureGuard::<CounterFixture>::new();
    assert_eq!(guard.setup_count, 1);
    assert_eq!(guard.request_count, 0);
    // Teardown happens on drop
}

#[test]
fn test_app_test_client_integration() {
    // Build a simple app with closures
    let app = App::builder()
        .route(
            "/hello",
            Method::Get,
            |_ctx: &RequestContext, _req: &mut Request| async {
                Response::ok().body(ResponseBody::Bytes(b"Hello, World!".to_vec()))
            },
        )
        .route(
            "/user",
            Method::Get,
            |_ctx: &RequestContext, _req: &mut Request| async {
                Response::json(&serde_json::json!({
                    "id": 42,
                    "name": "Test User"
                }))
                .unwrap()
            },
        )
        .build();

    // Create test client from Arc<App>
    let app = Arc::new(app);
    let client = app.clone().test_client();

    // Test GET request
    let response = client.get("/hello").send();
    response.assert_status(StatusCode::OK);
    assert_eq!(response.text(), "Hello, World!");

    // Test JSON response
    let response = client.get("/user").send();
    response.assert_status(StatusCode::OK);
    let json: serde_json::Value = response.json().unwrap();
    assert_eq!(json["id"], 42);
    assert_eq!(json["name"], "Test User");
}

#[test]
fn test_integration_test_with_fixtures() {
    let app = Arc::new(
        App::builder()
            .route(
                "/user",
                Method::Get,
                |_ctx: &RequestContext, _req: &mut Request| async {
                    Response::json(&serde_json::json!({
                        "id": 42,
                        "name": "Test User"
                    }))
                    .unwrap()
                },
            )
            .build(),
    );

    IntegrationTest::new("User API Test", app)
        .with_fixture::<UserFixture>()
        .run(|ctx| {
            // Access fixture data
            let user_fixture = ctx.fixture::<UserFixture>().unwrap();
            assert_eq!(user_fixture.user_id, 42);
            assert_eq!(user_fixture.username, "test_user");

            // Make request
            let response = ctx.get("/user").send();
            response.assert_success();

            let json: serde_json::Value = response.json().unwrap();
            assert_eq!(json["id"], user_fixture.user_id);
        });
}

#[test]
fn test_full_request_lifecycle() {
    // Tests the complete flow: request -> routing -> handler -> response
    let app = Arc::new(
        App::builder()
            .route(
                "/hello",
                Method::Get,
                |_ctx: &RequestContext, _req: &mut Request| async {
                    Response::ok().body(ResponseBody::Bytes(b"Hello!".to_vec()))
                },
            )
            .route(
                "/error",
                Method::Get,
                |_ctx: &RequestContext, _req: &mut Request| async {
                    Response::with_status(StatusCode::BAD_REQUEST)
                        .body(ResponseBody::Bytes(b"Bad Request".to_vec()))
                },
            )
            .build(),
    );

    let client = TestClient::new(app);

    // Successful request
    let response = client.get("/hello").send();
    assert!(response.is_success());
    assert_eq!(response.status().as_u16(), 200);

    // Error response
    let response = client.get("/error").send();
    assert!(response.is_client_error());
    assert_eq!(response.status().as_u16(), 400);

    // Not found
    let response = client.get("/nonexistent").send();
    assert_eq!(response.status().as_u16(), 404);

    // Method not allowed
    let response = client.post("/hello").send();
    assert_eq!(response.status().as_u16(), 405);
}

#[test]
fn test_options_auto_handling() {
    let app = Arc::new(
        App::builder()
            .route(
                "/resource",
                Method::Get,
                |_ctx: &RequestContext, _req: &mut Request| async {
                    Response::ok().body(ResponseBody::Bytes(b"GET".to_vec()))
                },
            )
            .route(
                "/resource",
                Method::Post,
                |_ctx: &RequestContext, _req: &mut Request| async {
                    Response::ok().body(ResponseBody::Empty)
                },
            )
            .build(),
    );

    let client = TestClient::new(app);

    // OPTIONS should return 204 with Allow header
    let response = client.options("/resource").send();
    assert_eq!(response.status().as_u16(), 204);

    let allow = response.header_str("Allow").unwrap();
    assert!(allow.contains("GET"));
    assert!(allow.contains("POST"));
    assert!(allow.contains("OPTIONS"));
}

#[test]
fn test_parallel_test_isolation() {
    // Verify that tests with seeds are deterministic
    let app = Arc::new(
        App::builder()
            .route(
                "/hello",
                Method::Get,
                |_ctx: &RequestContext, _req: &mut Request| async {
                    Response::ok().body(ResponseBody::Bytes(b"Hello!".to_vec()))
                },
            )
            .build(),
    );

    // Create two clients with same seed
    let client1 = app.clone().test_client_with_seed(42);
    let client2 = app.clone().test_client_with_seed(42);

    // Both should have the same seed
    assert_eq!(client1.seed(), Some(42));
    assert_eq!(client2.seed(), Some(42));

    // Both should work correctly
    let r1 = client1.get("/hello").send();
    let r2 = client2.get("/hello").send();

    assert_eq!(r1.status().as_u16(), r2.status().as_u16());
    assert_eq!(r1.text(), r2.text());
}

#[test]
fn test_state_reset_between_tests() {
    let app = Arc::new(
        App::builder()
            .route(
                "/hello",
                Method::Get,
                |_ctx: &RequestContext, _req: &mut Request| async {
                    Response::ok().body(ResponseBody::Bytes(b"Hello!".to_vec()))
                },
            )
            .build(),
    );

    let client = TestClient::new(app);

    // Set a cookie
    let response = client.get("/hello").cookie("session", "test123").send();
    response.assert_success();

    // Clear state
    client.clear_cookies();
    client.clear_dependency_overrides();

    // Cookies should be cleared
    assert!(client.cookies().is_empty());
}

#[test]
fn test_integration_test_reset_hooks() {
    use std::sync::atomic::{AtomicBool, Ordering};

    static RESET_CALLED: AtomicBool = AtomicBool::new(false);

    let app = Arc::new(
        App::builder()
            .route(
                "/hello",
                Method::Get,
                |_ctx: &RequestContext, _req: &mut Request| async {
                    Response::ok().body(ResponseBody::Bytes(b"Hello!".to_vec()))
                },
            )
            .build(),
    );

    IntegrationTest::new("Reset Hook Test", app)
        .on_reset(|| {
            RESET_CALLED.store(true, Ordering::SeqCst);
        })
        .run(|ctx| {
            let response = ctx.get("/hello").send();
            response.assert_success();
        });

    // Verify reset hook was called
    assert!(RESET_CALLED.load(Ordering::SeqCst));
}
