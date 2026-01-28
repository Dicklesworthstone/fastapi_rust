//! Authentication Example - Bearer Token Authentication with Protected Routes
//!
//! This example demonstrates authentication patterns in fastapi_rust:
//! - Bearer token authentication
//! - Protected routes that return 401 without valid token
//! - A simulated login endpoint
//! - Public and private endpoints
//! - Secure token comparison to prevent timing attacks
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example auth_example
//! ```
//!
//! # Expected Output
//!
//! ```text
//! fastapi_rust Authentication Example
//! ====================================
//!
//! 1. Public endpoint - no auth required
//!    GET /public -> 200 OK
//!
//! 2. Protected endpoint - without token
//!    GET /protected -> 401 Unauthorized
//!
//! 3. Login endpoint - get a token
//!    POST /login -> 200 OK
//!    Token: demo_secret_token_12345
//!
//! 4. Protected endpoint - with valid token
//!    GET /protected (Authorization: Bearer demo_secret_token_12345) -> 200 OK
//!
//! 5. Protected endpoint - with invalid token
//!    GET /protected (Authorization: Bearer wrong_token) -> 403 Forbidden
//!
//! 6. Protected endpoint - with wrong auth scheme
//!    GET /protected (Authorization: Basic ...) -> 401 Unauthorized
//!
//! 7. Login with wrong Content-Type
//!    POST /login (Content-Type: text/plain) -> 415 Unsupported Media Type
//!
//! 8. Token case sensitivity (lowercase 'bearer')
//!    GET /protected (Authorization: bearer demo_secret_token_12345) -> 200 OK
//!
//! All authentication tests passed!
//! ```
//!
//! # Security Notes
//!
//! This example uses a hardcoded secret token for demonstration purposes.
//! In a production application:
//! - Use cryptographically secure random tokens (e.g., UUID v4 or JWT)
//! - Store tokens securely (hashed in database)
//! - Implement token expiration
//! - Use HTTPS to protect tokens in transit
//! - Consider using OAuth2 or JWT for more complex scenarios

use fastapi::core::{
    App, BearerToken, Request, RequestContext, Response, ResponseBody, SecureCompare, StatusCode,
    TestClient,
};
use serde::Serialize;

/// The secret token used for authentication in this demo.
/// In production, this would be generated per-user and stored securely.
const SECRET_TOKEN: &str = "demo_secret_token_12345";

/// Login response body.
#[derive(Debug, Serialize)]
struct LoginResponse {
    access_token: String,
    token_type: &'static str,
}

/// User info returned from protected endpoints.
#[derive(Debug, Serialize)]
struct UserInfo {
    username: String,
    message: String,
}

/// Handler for public endpoint - accessible without authentication.
///
/// This endpoint demonstrates a route that anyone can access.
fn public_handler(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    let body = serde_json::json!({
        "message": "This is a public endpoint - no authentication required!"
    });
    std::future::ready(
        Response::ok()
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(body.to_string().into_bytes())),
    )
}

/// Handler for the login endpoint.
///
/// In a real application, this would:
/// 1. Validate username/password against a database
/// 2. Generate a unique token (JWT or random)
/// 3. Store the token with associated user info
/// 4. Return the token to the client
///
/// For this demo, we accept any credentials and return a fixed token.
fn login_handler(_ctx: &RequestContext, req: &mut Request) -> std::future::Ready<Response> {
    // In a real app, we would parse the JSON body and validate credentials.
    // For this demo, we just check that it's a POST with some body.

    // Check Content-Type
    let is_json = req
        .headers()
        .get("content-type")
        .is_some_and(|ct| ct.starts_with(b"application/json"));

    if !is_json {
        let error = serde_json::json!({
            "detail": "Content-Type must be application/json"
        });
        return std::future::ready(
            Response::with_status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
                .header("content-type", b"application/json".to_vec())
                .body(ResponseBody::Bytes(error.to_string().into_bytes())),
        );
    }

    // For demo purposes, we don't validate credentials - just return the token
    // In production, you would:
    // 1. Parse the request body as LoginRequest
    // 2. Verify username/password against your database
    // 3. Generate a unique, cryptographically secure token
    // 4. Store token -> user_id mapping (with expiration)

    let response = LoginResponse {
        access_token: SECRET_TOKEN.to_string(),
        token_type: "bearer",
    };

    std::future::ready(
        Response::ok()
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(
                serde_json::to_string(&response).unwrap().into_bytes(),
            )),
    )
}

/// Handler for protected endpoint - requires valid bearer token.
///
/// This handler manually extracts and validates the bearer token:
/// 1. Gets the Authorization header
/// 2. Verifies it uses the Bearer scheme
/// 3. Validates the token against our secret using constant-time comparison
///
/// Returns appropriate error responses for each failure mode.
fn protected_handler(_ctx: &RequestContext, req: &mut Request) -> std::future::Ready<Response> {
    // Step 1: Get the Authorization header
    let Some(auth_header) = req.headers().get("authorization") else {
        // Missing header -> 401 Unauthorized
        let body = serde_json::json!({
            "detail": "Not authenticated"
        });
        return std::future::ready(
            Response::with_status(StatusCode::UNAUTHORIZED)
                .header("www-authenticate", b"Bearer".to_vec())
                .header("content-type", b"application/json".to_vec())
                .body(ResponseBody::Bytes(body.to_string().into_bytes())),
        );
    };

    // Step 2: Parse the Authorization header
    let Ok(auth_str) = std::str::from_utf8(auth_header) else {
        // Invalid UTF-8 -> 401 Unauthorized
        let body = serde_json::json!({
            "detail": "Invalid authentication credentials"
        });
        return std::future::ready(
            Response::with_status(StatusCode::UNAUTHORIZED)
                .header("www-authenticate", b"Bearer".to_vec())
                .header("content-type", b"application/json".to_vec())
                .body(ResponseBody::Bytes(body.to_string().into_bytes())),
        );
    };

    // Step 3: Check for "Bearer " prefix (case-insensitive for the scheme)
    let Some(token) = auth_str
        .strip_prefix("Bearer ")
        .or_else(|| auth_str.strip_prefix("bearer "))
    else {
        // Wrong scheme -> 401 Unauthorized
        let body = serde_json::json!({
            "detail": "Invalid authentication credentials"
        });
        return std::future::ready(
            Response::with_status(StatusCode::UNAUTHORIZED)
                .header("www-authenticate", b"Bearer".to_vec())
                .header("content-type", b"application/json".to_vec())
                .body(ResponseBody::Bytes(body.to_string().into_bytes())),
        );
    };

    let token = token.trim();
    if token.is_empty() {
        // Empty token -> 401 Unauthorized
        let body = serde_json::json!({
            "detail": "Invalid authentication credentials"
        });
        return std::future::ready(
            Response::with_status(StatusCode::UNAUTHORIZED)
                .header("www-authenticate", b"Bearer".to_vec())
                .header("content-type", b"application/json".to_vec())
                .body(ResponseBody::Bytes(body.to_string().into_bytes())),
        );
    }

    // Step 4: Validate the token using constant-time comparison
    // Create a BearerToken for secure comparison
    let bearer_token = BearerToken::new(token);
    if !bearer_token.secure_eq(SECRET_TOKEN) {
        // Invalid token -> 403 Forbidden
        let body = serde_json::json!({
            "detail": "Invalid token"
        });
        return std::future::ready(
            Response::with_status(StatusCode::FORBIDDEN)
                .header("content-type", b"application/json".to_vec())
                .body(ResponseBody::Bytes(body.to_string().into_bytes())),
        );
    }

    // Token is valid - return protected data
    let user_info = UserInfo {
        username: "demo_user".to_string(),
        message: "You have accessed a protected resource!".to_string(),
    };

    std::future::ready(
        Response::ok()
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(
                serde_json::to_string(&user_info).unwrap().into_bytes(),
            )),
    )
}

#[allow(clippy::too_many_lines)]
fn main() {
    println!("fastapi_rust Authentication Example");
    println!("====================================\n");

    // Build the application with public and protected routes
    let app = App::builder()
        // Public endpoints - accessible to everyone
        .get("/public", public_handler)
        // Login endpoint - returns a token
        .post("/login", login_handler)
        // Protected endpoint - requires valid bearer token
        .get("/protected", protected_handler)
        .build();

    println!("App created with {} route(s)\n", app.route_count());

    // Create a test client
    let client = TestClient::new(app);

    // =========================================================================
    // Test 1: Public endpoint - no auth required
    // =========================================================================
    println!("1. Public endpoint - no auth required");
    let response = client.get("/public").send();
    println!(
        "   GET /public -> {} {}",
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    assert_eq!(response.status().as_u16(), 200);
    assert!(response.text().contains("public endpoint"));

    // =========================================================================
    // Test 2: Protected endpoint - without token (should get 401)
    // =========================================================================
    println!("\n2. Protected endpoint - without token");
    let response = client.get("/protected").send();
    println!(
        "   GET /protected -> {} {}",
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    assert_eq!(
        response.status().as_u16(),
        401,
        "Protected endpoint should return 401 without token"
    );

    // Check for WWW-Authenticate header
    let has_www_auth = response
        .headers()
        .iter()
        .any(|(name, value)| name == "www-authenticate" && value == b"Bearer");
    assert!(
        has_www_auth,
        "401 response should include WWW-Authenticate: Bearer header"
    );

    // =========================================================================
    // Test 3: Login endpoint - get a token
    // =========================================================================
    println!("\n3. Login endpoint - get a token");
    let response = client
        .post("/login")
        .header("content-type", "application/json")
        .body(r#"{"username":"test","password":"test123"}"#)
        .send();
    println!(
        "   POST /login -> {} {}",
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    assert_eq!(response.status().as_u16(), 200);

    // Parse the response to get the token
    let body: serde_json::Value = serde_json::from_str(response.text()).unwrap();
    let token = body["access_token"].as_str().unwrap();
    println!("   Token: {token}");
    assert_eq!(token, SECRET_TOKEN);

    // =========================================================================
    // Test 4: Protected endpoint - with valid token (should get 200)
    // =========================================================================
    println!("\n4. Protected endpoint - with valid token");
    let response = client
        .get("/protected")
        .header("authorization", format!("Bearer {SECRET_TOKEN}"))
        .send();
    println!(
        "   GET /protected (Authorization: Bearer {}) -> {} {}",
        SECRET_TOKEN,
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    assert_eq!(
        response.status().as_u16(),
        200,
        "Protected endpoint should return 200 with valid token"
    );
    assert!(response.text().contains("protected resource"));

    // =========================================================================
    // Test 5: Protected endpoint - with invalid token (should get 403)
    // =========================================================================
    println!("\n5. Protected endpoint - with invalid token");
    let response = client
        .get("/protected")
        .header("authorization", "Bearer wrong_token")
        .send();
    println!(
        "   GET /protected (Authorization: Bearer wrong_token) -> {} {}",
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    assert_eq!(
        response.status().as_u16(),
        403,
        "Protected endpoint should return 403 with invalid token"
    );

    // =========================================================================
    // Test 6: Protected endpoint - with wrong auth scheme (should get 401)
    // =========================================================================
    println!("\n6. Protected endpoint - with wrong auth scheme");
    let response = client
        .get("/protected")
        .header("authorization", "Basic dXNlcjpwYXNz")
        .send();
    println!(
        "   GET /protected (Authorization: Basic ...) -> {} {}",
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    assert_eq!(
        response.status().as_u16(),
        401,
        "Protected endpoint should return 401 with wrong auth scheme"
    );

    // =========================================================================
    // Test 7: Login with wrong Content-Type (should get 415)
    // =========================================================================
    println!("\n7. Login with wrong Content-Type");
    let response = client
        .post("/login")
        .header("content-type", "text/plain")
        .body("username=test&password=test123")
        .send();
    println!(
        "   POST /login (Content-Type: text/plain) -> {} {}",
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    assert_eq!(
        response.status().as_u16(),
        415,
        "Login should return 415 with wrong Content-Type"
    );

    // =========================================================================
    // Test 8: Token case sensitivity (lowercase 'bearer')
    // =========================================================================
    println!("\n8. Token case sensitivity (lowercase 'bearer')");
    let response = client
        .get("/protected")
        .header("authorization", format!("bearer {SECRET_TOKEN}"))
        .send();
    println!(
        "   GET /protected (Authorization: bearer {}) -> {} {}",
        SECRET_TOKEN,
        response.status().as_u16(),
        response.status().canonical_reason()
    );
    assert_eq!(
        response.status().as_u16(),
        200,
        "Bearer scheme should be case-insensitive (lowercase accepted)"
    );

    println!("\nAll authentication tests passed!");
}
