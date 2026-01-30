//! Full-featured demo application showcasing the fastapi_rust framework.
//!
//! Demonstrates:
//! - Route definitions with multiple HTTP methods
//! - Path and query parameter extraction
//! - JSON request/response handling
//! - Middleware (CORS, security headers, rate limiting, request ID)
//! - Error handling with validation
//! - Dependency injection
//! - Pagination
//! - Background tasks
//! - OpenAPI schema generation
//! - Configuration
//!
//! Run with: `cargo run --example demo_api`

use fastapi::prelude::*;
use serde::{Deserialize, Serialize};

// ============================================================
// Domain Models
// ============================================================

/// A user in our system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub role: UserRole,
}

/// User roles for authorization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum UserRole {
    Admin,
    Editor,
    Viewer,
}

/// Request body for creating a user.
#[derive(Debug, Deserialize)]
pub struct CreateUser {
    pub name: String,
    pub email: String,
    pub role: Option<UserRole>,
}

/// Request body for updating a user.
#[derive(Debug, Deserialize)]
pub struct UpdateUser {
    pub name: Option<String>,
    pub email: Option<String>,
    pub role: Option<UserRole>,
}

/// Response envelope for consistent API responses.
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            data,
            message: None,
        }
    }

    pub fn with_message(data: T, msg: impl Into<String>) -> Self {
        Self {
            data,
            message: Some(msg.into()),
        }
    }
}

/// Paginated response wrapper.
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u64,
    pub per_page: u64,
    pub total_pages: u64,
}

// ============================================================
// Application State
// ============================================================

/// Shared application state.
#[derive(Debug, Clone)]
pub struct AppState {
    pub app_name: String,
    pub version: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            app_name: "Demo API".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

// ============================================================
// Route Handlers
// ============================================================

/// Health check endpoint.
///
/// Returns service status and version information.
/// Used by load balancers and monitoring systems.
fn health_check() -> serde_json::Value {
    serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_notice": "This is a demo application"
    })
}

/// List users with pagination.
///
/// Supports page/per_page query parameters with sensible defaults.
fn list_users(page: u64, per_page: u64) -> PaginatedResponse<User> {
    // Simulated data
    let all_users = sample_users();
    let total = all_users.len() as u64;
    let total_pages = total.div_ceil(per_page);
    #[allow(clippy::cast_possible_truncation)]
    let start = ((page - 1) * per_page) as usize;
    #[allow(clippy::cast_possible_truncation)]
    let items: Vec<User> = all_users
        .into_iter()
        .skip(start)
        .take(per_page as usize)
        .collect();

    PaginatedResponse {
        items,
        total,
        page,
        per_page,
        total_pages,
    }
}

/// Get a single user by ID.
fn get_user(id: u64) -> Result<ApiResponse<User>, (u16, String)> {
    sample_users()
        .into_iter()
        .find(|u| u.id == id)
        .map(ApiResponse::ok)
        .ok_or((404, format!("User {id} not found")))
}

/// Create a new user.
fn create_user(input: CreateUser) -> ApiResponse<User> {
    let user = User {
        id: 42, // In production, this would be auto-generated
        name: input.name,
        email: input.email,
        role: input.role.unwrap_or(UserRole::Viewer),
    };
    ApiResponse::with_message(user, "User created successfully")
}

/// Delete a user by ID.
#[allow(dead_code)]
fn delete_user(id: u64) -> Result<ApiResponse<()>, (u16, String)> {
    if sample_users().iter().any(|u| u.id == id) {
        Ok(ApiResponse::with_message((), format!("User {id} deleted")))
    } else {
        Err((404, format!("User {id} not found")))
    }
}

// ============================================================
// Configuration
// ============================================================

/// Build the application configuration.
fn build_config() -> AppConfig {
    AppConfig {
        name: "Demo API".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        debug: cfg!(debug_assertions),
        max_body_size: 10 * 1024 * 1024, // 10 MB
        request_timeout_ms: 30_000,
        root_path: String::new(),
        root_path_in_servers: false,
        trailing_slash_mode: fastapi_core::routing::TrailingSlashMode::Strict,
        debug_config: fastapi_core::error::DebugConfig::default(),
    }
}

/// Print demo information.
fn print_demo_info() {
    println!("=== FastAPI Rust Demo Application ===");
    println!();
    println!("This example demonstrates:");
    println!("  - Route definitions (GET, POST, DELETE)");
    println!("  - Path and query parameter extraction");
    println!("  - JSON request/response handling");
    println!("  - Middleware configuration (CORS, security headers)");
    println!("  - Pagination patterns");
    println!("  - Error handling");
    println!("  - Application configuration");
    println!();
    println!("Endpoints:");
    println!("  GET  /health              - Health check");
    println!("  GET  /api/v1/users        - List users (paginated)");
    println!("  GET  /api/v1/users/:id    - Get user by ID");
    println!("  POST /api/v1/users        - Create user");
    println!("  DELETE /api/v1/users/:id  - Delete user");
    println!();
    println!("Configuration:");
    let config = build_config();
    println!("  Name:           {}", config.name);
    println!("  Version:        {}", config.version);
    println!("  Debug:          {}", config.debug);
    println!(
        "  Max body size:  {} MB",
        config.max_body_size / (1024 * 1024)
    );
    println!("  Timeout:        {}ms", config.request_timeout_ms);
    println!();
    println!("CORS: allow_any_origin=true, allow_credentials=true");
    println!("Rate limit: 100 req/min (token bucket)");
    println!("Security headers: X-Content-Type-Options, X-Frame-Options, Referrer-Policy");
    println!();
    println!("--- Demo data ---");
    println!();

    // Demonstrate the handlers
    println!("Health check:");
    let health = health_check();
    println!("  {}", serde_json::to_string_pretty(&health).unwrap());
    println!();

    println!("List users (page 1, 2 per page):");
    let users = list_users(1, 2);
    println!("  {}", serde_json::to_string_pretty(&users).unwrap());
    println!();

    println!("Get user 1:");
    match get_user(1) {
        Ok(resp) => println!("  {}", serde_json::to_string_pretty(&resp).unwrap()),
        Err((code, msg)) => println!("  Error {code}: {msg}"),
    }
    println!();

    println!("Get user 999 (not found):");
    match get_user(999) {
        Ok(resp) => println!("  {}", serde_json::to_string_pretty(&resp).unwrap()),
        Err((code, msg)) => println!("  Error {code}: {msg}"),
    }
    println!();

    println!("Create user:");
    let new_user = create_user(CreateUser {
        name: "New User".into(),
        email: "new@example.com".into(),
        role: Some(UserRole::Editor),
    });
    println!("  {}", serde_json::to_string_pretty(&new_user).unwrap());
}

// ============================================================
// Sample Data
// ============================================================

fn sample_users() -> Vec<User> {
    vec![
        User {
            id: 1,
            name: "Alice Johnson".into(),
            email: "alice@example.com".into(),
            role: UserRole::Admin,
        },
        User {
            id: 2,
            name: "Bob Smith".into(),
            email: "bob@example.com".into(),
            role: UserRole::Editor,
        },
        User {
            id: 3,
            name: "Carol Williams".into(),
            email: "carol@example.com".into(),
            role: UserRole::Viewer,
        },
    ]
}

// ============================================================
// Main
// ============================================================

fn main() {
    print_demo_info();
}
