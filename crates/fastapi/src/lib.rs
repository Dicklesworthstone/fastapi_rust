//! Ultra-optimized Rust web framework inspired by FastAPI.
//!
//! fastapi_rust provides a type-safe, high-performance web framework with:
//!
//! - **Type-driven API design** — Route handlers declare types, framework extracts/validates automatically
//! - **Dependency injection** — Composable, testable request handling
//! - **Automatic OpenAPI** — Schema generation from type definitions
//! - **First-class async** — Built on asupersync for structured concurrency
//! - **Minimal dependencies** — Only asupersync + serde
//!
//! # Quick Start
//!
//! ```ignore
//! use fastapi::prelude::*;
//!
//! #[derive(Serialize, Deserialize, JsonSchema)]
//! struct Item {
//!     id: i64,
//!     name: String,
//! }
//!
//! #[get("/items/{id}")]
//! async fn get_item(cx: &Cx, id: Path<i64>) -> Json<Item> {
//!     Json(Item { id: id.0, name: "Example".into() })
//! }
//!
//! fn main() {
//!     let app = App::new()
//!         .title("My API")
//!         .route(get_item);
//!
//!     // Run with asupersync
//!     // asupersync::block_on(app.serve("0.0.0.0:8000"));
//! }
//! ```
//!
//! # Design Philosophy
//!
//! This framework is built with the following principles:
//!
//! 1. **Zero-cost abstractions** — No runtime reflection, everything at compile time
//! 2. **Cancel-correct** — Leverages asupersync's structured concurrency
//! 3. **Minimal allocations** — Zero-copy parsing where possible
//! 4. **Familiar API** — FastAPI users will recognize the patterns
//!
//! # Crate Structure
//!
//! - [`fastapi_core`] — Core types (Request, Response, Error)
//! - [`fastapi_http`] — Zero-copy HTTP/1.1 parser
//! - [`fastapi_router`] — Trie-based router
//! - [`fastapi_macros`] — Procedural macros (`#[get]`, `#[derive(Validate)]`)
//! - [`fastapi_openapi`] — OpenAPI 3.1 types and generation

#![forbid(unsafe_code)]
// Design doc at PROPOSED_RUST_ARCHITECTURE.md (not embedded - too many conceptual code examples)

// Re-export crates
pub use fastapi_core as core;
pub use fastapi_http as http;
pub use fastapi_macros as macros;
pub use fastapi_openapi as openapi;
pub use fastapi_router as router;

// Re-export commonly used types
pub use fastapi_core::{
    App, AppBuilder, AppConfig, Cors, CorsConfig, DefaultConfig, DefaultDependencyConfig,
    DependencyOverrides, DependencyScope, Depends, DependsConfig, FromDependency, FromRequest,
    HttpError, IntoResponse, Method, NoCache, Request, RequestId, RequestIdConfig,
    RequestIdMiddleware, Response, ResponseBody, StateContainer, StatusCode, ValidationError,
    ValidationErrors,
};

// Re-export testing utilities
pub use fastapi_core::{CookieJar, RequestBuilder, TestClient, TestResponse};
pub use fastapi_macros::{JsonSchema, Validate, delete, get, patch, post, put};
pub use fastapi_openapi::{OpenApi, OpenApiBuilder};
pub use fastapi_router::{Route, Router};

/// Prelude module for convenient imports.
pub mod prelude {
    pub use crate::{
        App, AppBuilder, AppConfig, Cors, CorsConfig, DefaultConfig, DefaultDependencyConfig,
        DependencyOverrides, DependencyScope, Depends, DependsConfig, FromDependency, FromRequest,
        HttpError, IntoResponse, JsonSchema, Method, NoCache, OpenApi, OpenApiBuilder, Request,
        RequestId, RequestIdMiddleware, Response, Route, Router, StatusCode, Validate,
        ValidationError, ValidationErrors, delete, get, patch, post, put,
    };
    pub use serde::{Deserialize, Serialize};
}

/// Testing utilities module.
pub mod testing {
    pub use fastapi_core::testing::{CookieJar, RequestBuilder, TestClient, TestResponse};
}

/// Extractors module for request data extraction.
pub mod extract {
    pub use fastapi_core::{
        Accept, AppState, Authorization, ContentType, FromHeaderValue, Header, HeaderExtractError,
        HeaderName, HeaderValues, Host, Json, JsonConfig, JsonExtractError, NamedHeader,
        OAuth2BearerError, OAuth2BearerErrorKind, OAuth2PasswordBearer, OAuth2PasswordBearerConfig,
        Path, PathExtractError, PathParams, Query, QueryExtractError, QueryParams, State,
        StateExtractError, UserAgent, XRequestId,
    };
}

/// Extension trait for generating OpenAPI specifications from applications.
pub trait OpenApiExt {
    /// Generate an OpenAPI specification from the application.
    ///
    /// This creates an OpenAPI 3.1 document based on the application's
    /// configuration and registered routes.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi::prelude::*;
    /// use fastapi::OpenApiExt;
    ///
    /// let app = App::builder()
    ///     .config(AppConfig::new().name("My API").version("1.0.0"))
    ///     .build();
    ///
    /// let spec = app.openapi();
    /// println!("{}", serde_json::to_string_pretty(&spec).unwrap());
    /// ```
    fn openapi(&self) -> OpenApi;

    /// Generate an OpenAPI specification with custom configuration.
    fn openapi_with<F>(&self, configure: F) -> OpenApi
    where
        F: FnOnce(OpenApiBuilder) -> OpenApiBuilder;
}

impl OpenApiExt for App {
    fn openapi(&self) -> OpenApi {
        self.openapi_with(|b| b)
    }

    fn openapi_with<F>(&self, configure: F) -> OpenApi
    where
        F: FnOnce(OpenApiBuilder) -> OpenApiBuilder,
    {
        // Start with app config
        let mut builder = OpenApiBuilder::new(&self.config().name, &self.config().version);

        // Add routes from the application
        for (method, path) in self.routes() {
            let operation_id = generate_operation_id(method, path);
            let method_str = method_to_str(method);
            builder = builder.operation(
                method_str,
                path,
                fastapi_openapi::Operation {
                    operation_id: Some(operation_id),
                    ..Default::default()
                },
            );
        }

        // Apply custom configuration
        builder = configure(builder);

        builder.build()
    }
}

/// Convert a Method to its string representation.
fn method_to_str(method: Method) -> &'static str {
    match method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        Method::Patch => "PATCH",
        Method::Head => "HEAD",
        Method::Options => "OPTIONS",
        Method::Trace => "TRACE",
    }
}

/// Generate an operation ID from method and path.
fn generate_operation_id(method: Method, path: &str) -> String {
    let method_lower = method_to_str(method).to_lowercase();
    let path_part = path
        .trim_start_matches('/')
        .replace('/', "_")
        .replace(['{', '}'], "");
    if path_part.is_empty() {
        method_lower
    } else {
        format!("{method_lower}_{path_part}")
    }
}
