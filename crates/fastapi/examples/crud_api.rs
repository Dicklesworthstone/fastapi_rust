//! CRUD API Example - In-Memory User Management
//!
//! This example demonstrates a full CRUD (Create, Read, Update, Delete) API:
//! - `POST /users` - Create a new user
//! - `GET /users` - List all users
//! - `GET /users/{id}` - Get a user by ID
//! - `PUT /users/{id}` - Update a user by ID
//! - `DELETE /users/{id}` - Delete a user by ID
//!
//! Features demonstrated:
//! - In-memory storage with `Mutex<HashMap>`
//! - JSON request/response handling
//! - Path parameter extraction (manual)
//! - Proper HTTP status codes (200, 201, 204, 404, 400, 415)
//! - Input validation
//! - Error responses matching FastAPI style
//!
//! # Running This Example
//!
//! ```bash
//! cargo run --example crud_api -p fastapi-rust
//! ```
//!
//! # Equivalent curl Commands
//!
//! ```bash
//! # Create a user
//! curl -X POST http://localhost:8000/users \
//!   -H "Content-Type: application/json" \
//!   -d '{"name": "Alice", "email": "alice@example.com"}'
//!
//! # List all users
//! curl http://localhost:8000/users
//!
//! # Get a user by ID
//! curl http://localhost:8000/users/1
//!
//! # Update a user
//! curl -X PUT http://localhost:8000/users/1 \
//!   -H "Content-Type: application/json" \
//!   -d '{"name": "Alice Smith", "email": "alice.smith@example.com"}'
//!
//! # Delete a user
//! curl -X DELETE http://localhost:8000/users/1
//! ```

use fastapi_rust::core::{
    App, Body, Request, RequestContext, Response, ResponseBody, StatusCode, TestClient,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

// ============================================================================
// Models
// ============================================================================

#[derive(Debug, Clone, Deserialize)]
struct UserInput {
    name: String,
    email: String,
}

#[derive(Debug, Clone, Serialize)]
struct User {
    id: u64,
    name: String,
    email: String,
}

struct UserDb {
    users: HashMap<u64, User>,
    next_id: u64,
}

// Global in-memory store. In a real app, use State<T> with
// a database connection pool instead.
static STORE: Mutex<Option<UserDb>> = Mutex::new(None);

fn with_db<R>(f: impl FnOnce(&mut UserDb) -> R) -> R {
    let mut guard = STORE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let db = guard.get_or_insert_with(|| UserDb {
        users: HashMap::new(),
        next_id: 1,
    });
    f(db)
}

// ============================================================================
// Helpers
// ============================================================================

fn parse_json_body<T: serde::de::DeserializeOwned>(req: &mut Request) -> Result<T, Response> {
    let is_json = req
        .headers()
        .get("content-type")
        .is_some_and(|ct| ct.starts_with(b"application/json"));

    if !is_json {
        return Err(json_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        ));
    }

    let Body::Bytes(body) = req.take_body() else {
        return Err(json_error(StatusCode::BAD_REQUEST, "Missing request body"));
    };
    serde_json::from_slice(&body)
        .map_err(|e| json_error(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {e}")))
}

fn extract_user_id(req: &Request) -> Result<u64, Response> {
    let path = req.path();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    segments
        .get(1)
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| json_error(StatusCode::BAD_REQUEST, "Invalid user ID"))
}

fn json_error(status: StatusCode, detail: &str) -> Response {
    let body = serde_json::json!({ "detail": detail });
    Response::with_status(status)
        .header("content-type", b"application/json".to_vec())
        .body(ResponseBody::Bytes(body.to_string().into_bytes()))
}

fn json_response(status: StatusCode, value: &impl Serialize) -> Response {
    match serde_json::to_string(value) {
        Ok(text) => Response::with_status(status)
            .header("content-type", b"application/json".to_vec())
            .body(ResponseBody::Bytes(text.into_bytes())),
        Err(err) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("JSON serialization failed: {err}"),
        ),
    }
}

fn validate_input(input: &UserInput) -> Option<Response> {
    if input.name.trim().is_empty() {
        return Some(json_error(
            StatusCode::BAD_REQUEST,
            "name must not be empty",
        ));
    }
    if !input.email.contains('@') {
        return Some(json_error(
            StatusCode::BAD_REQUEST,
            "email must contain '@'",
        ));
    }
    None
}

// ============================================================================
// Handlers
// ============================================================================

fn create_user(_ctx: &RequestContext, req: &mut Request) -> std::future::Ready<Response> {
    let input = match parse_json_body::<UserInput>(req) {
        Ok(v) => v,
        Err(r) => return std::future::ready(r),
    };
    if let Some(r) = validate_input(&input) {
        return std::future::ready(r);
    }

    let user = with_db(|db| {
        let id = db.next_id;
        db.next_id += 1;
        let user = User {
            id,
            name: input.name,
            email: input.email,
        };
        db.users.insert(id, user.clone());
        user
    });

    std::future::ready(json_response(StatusCode::CREATED, &user))
}

fn list_users(_ctx: &RequestContext, _req: &mut Request) -> std::future::Ready<Response> {
    let users = with_db(|db| {
        let mut v: Vec<User> = db.users.values().cloned().collect();
        v.sort_by_key(|u| u.id);
        v
    });
    std::future::ready(json_response(StatusCode::OK, &users))
}

fn get_user(_ctx: &RequestContext, req: &mut Request) -> std::future::Ready<Response> {
    let id = match extract_user_id(req) {
        Ok(id) => id,
        Err(r) => return std::future::ready(r),
    };
    let result = with_db(|db| db.users.get(&id).cloned());
    match result {
        Some(user) => std::future::ready(json_response(StatusCode::OK, &user)),
        None => std::future::ready(json_error(
            StatusCode::NOT_FOUND,
            &format!("User {id} not found"),
        )),
    }
}

fn update_user(_ctx: &RequestContext, req: &mut Request) -> std::future::Ready<Response> {
    let id = match extract_user_id(req) {
        Ok(id) => id,
        Err(r) => return std::future::ready(r),
    };
    let input = match parse_json_body::<UserInput>(req) {
        Ok(v) => v,
        Err(r) => return std::future::ready(r),
    };
    if let Some(r) = validate_input(&input) {
        return std::future::ready(r);
    }

    let result = with_db(|db| {
        db.users.get_mut(&id).map(|user| {
            user.name = input.name;
            user.email = input.email;
            user.clone()
        })
    });
    match result {
        Some(user) => std::future::ready(json_response(StatusCode::OK, &user)),
        None => std::future::ready(json_error(
            StatusCode::NOT_FOUND,
            &format!("User {id} not found"),
        )),
    }
}

fn delete_user(_ctx: &RequestContext, req: &mut Request) -> std::future::Ready<Response> {
    let id = match extract_user_id(req) {
        Ok(id) => id,
        Err(r) => return std::future::ready(r),
    };
    let removed = with_db(|db| db.users.remove(&id).is_some());
    if removed {
        std::future::ready(Response::with_status(StatusCode::NO_CONTENT))
    } else {
        std::future::ready(json_error(
            StatusCode::NOT_FOUND,
            &format!("User {id} not found"),
        ))
    }
}

// ============================================================================
// Main
// ============================================================================

#[allow(clippy::needless_pass_by_value)]
fn check_eq<T: PartialEq + std::fmt::Debug>(left: T, right: T, message: &str) -> bool {
    if left == right {
        true
    } else {
        eprintln!("Check failed: {message}. left={left:?} right={right:?}");
        false
    }
}

#[allow(clippy::too_many_lines)]
fn main() {
    // Initialize the global store
    let mut guard = STORE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Some(UserDb {
        users: HashMap::new(),
        next_id: 1,
    });

    println!("fastapi_rust CRUD API Example");
    println!("=============================\n");

    let app = App::builder()
        .post("/users", create_user)
        .get("/users", list_users)
        .get("/users/{id}", get_user)
        .put("/users/{id}", update_user)
        .delete("/users/{id}", delete_user)
        .build();

    println!("App created with {} route(s)\n", app.route_count());
    let client = TestClient::new(app);

    // 1. Create users
    println!("1. Create users");
    let resp = client
        .post("/users")
        .header("content-type", "application/json")
        .body(r#"{"name": "Alice", "email": "alice@example.com"}"#)
        .send();
    println!(
        "   POST /users -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(resp.status().as_u16(), 201, "Create user should return 201") {
        return;
    }

    let resp = client
        .post("/users")
        .header("content-type", "application/json")
        .body(r#"{"name": "Bob", "email": "bob@example.com"}"#)
        .send();
    println!(
        "   POST /users -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(resp.status().as_u16(), 201, "Create user should return 201") {
        return;
    }

    // 2. List users
    println!("\n2. List all users");
    let resp = client.get("/users").send();
    println!(
        "   GET /users -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(resp.status().as_u16(), 200, "List users should return 200") {
        return;
    }

    // 3. Get user by ID
    println!("\n3. Get user by ID");
    let resp = client.get("/users/1").send();
    println!(
        "   GET /users/1 -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(resp.status().as_u16(), 200, "Get user should return 200") {
        return;
    }

    // 4. Get nonexistent user
    println!("\n4. Get nonexistent user");
    let resp = client.get("/users/999").send();
    println!(
        "   GET /users/999 -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(
        resp.status().as_u16(),
        404,
        "Missing user should return 404",
    ) {
        return;
    }

    // 5. Update user
    println!("\n5. Update user");
    let resp = client
        .put("/users/1")
        .header("content-type", "application/json")
        .body(r#"{"name": "Alice Smith", "email": "alice.smith@example.com"}"#)
        .send();
    println!(
        "   PUT /users/1 -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(resp.status().as_u16(), 200, "Update user should return 200") {
        return;
    }

    // 6. Validation: empty name
    println!("\n6. Validation error (empty name)");
    let resp = client
        .post("/users")
        .header("content-type", "application/json")
        .body(r#"{"name": "", "email": "bad@example.com"}"#)
        .send();
    println!(
        "   POST /users -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(resp.status().as_u16(), 400, "Empty name should return 400") {
        return;
    }

    // 7. Validation: invalid email
    println!("\n7. Validation error (invalid email)");
    let resp = client
        .post("/users")
        .header("content-type", "application/json")
        .body(r#"{"name": "Charlie", "email": "not-an-email"}"#)
        .send();
    println!(
        "   POST /users -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(
        resp.status().as_u16(),
        400,
        "Invalid email should return 400",
    ) {
        return;
    }

    // 8. Wrong Content-Type
    println!("\n8. Wrong Content-Type");
    let resp = client
        .post("/users")
        .header("content-type", "text/plain")
        .body(r#"{"name": "Dan", "email": "dan@example.com"}"#)
        .send();
    println!(
        "   POST /users -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(
        resp.status().as_u16(),
        415,
        "Wrong Content-Type should return 415",
    ) {
        return;
    }

    // 9. Delete user
    println!("\n9. Delete user");
    let resp = client.delete("/users/2").send();
    println!("   DELETE /users/2 -> {}", resp.status().as_u16());
    if !check_eq(resp.status().as_u16(), 204, "Delete user should return 204") {
        return;
    }

    // 10. Verify deletion
    println!("\n10. Verify deletion");
    let resp = client.get("/users/2").send();
    println!(
        "   GET /users/2 -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(
        resp.status().as_u16(),
        404,
        "Deleted user should return 404",
    ) {
        return;
    }

    let resp = client.get("/users").send();
    println!(
        "   GET /users -> {} {}",
        resp.status().as_u16(),
        resp.text()
    );
    if !check_eq(resp.status().as_u16(), 200, "List users should return 200") {
        return;
    }

    println!("\nAll CRUD operations passed!");
}
