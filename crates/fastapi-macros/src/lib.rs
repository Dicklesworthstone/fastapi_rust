//! Procedural macros for fastapi_rust.
//!
//! This crate provides the following macros:
//!
//! - Route macros: `#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]`, `#[head]`, `#[options]`
//! - `#[derive(Validate)]` for compile-time validation
//! - `#[derive(JsonSchema)]` for OpenAPI schema generation
//!
//! # Role In The System
//!
//! `fastapi-macros` is the compile-time glue that keeps the runtime minimal.
//! It analyzes handler signatures, generates route registration metadata, and
//! enforces validation/schema rules without any runtime reflection. The emitted
//! code targets types from `fastapi-core` and `fastapi-openapi`, and is re-exported
//! by the `fastapi` facade crate for user ergonomics.
//!
//! # Example
//!
//! ```ignore
//! use fastapi::prelude::*;
//!
//! #[get("/items/{id}")]
//! async fn get_item(cx: &Cx, id: Path<i64>) -> Json<Item> {
//!     // ...
//! }
//! ```

use proc_macro::TokenStream;

mod openapi;
mod param;
mod route;
mod validate;

/// Mark a function as a GET handler.
///
/// # Example
///
/// ```ignore
/// #[get("/items")]
/// async fn list_items() -> Json<Vec<Item>> {
///     // ...
/// }
/// ```
#[proc_macro_attribute]
pub fn get(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("Get", attr, item)
}

/// Mark a function as a POST handler.
#[proc_macro_attribute]
pub fn post(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("Post", attr, item)
}

/// Mark a function as a PUT handler.
#[proc_macro_attribute]
pub fn put(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("Put", attr, item)
}

/// Mark a function as a DELETE handler.
#[proc_macro_attribute]
pub fn delete(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("Delete", attr, item)
}

/// Mark a function as a PATCH handler.
#[proc_macro_attribute]
pub fn patch(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("Patch", attr, item)
}

/// Mark a function as a HEAD handler.
///
/// HEAD requests are identical to GET but return only headers, not body.
/// Useful for checking resource existence or metadata without full content.
///
/// # Example
///
/// ```ignore
/// #[head("/items/{id}")]
/// async fn head_item(id: Path<i64>) -> StatusCode {
///     StatusCode::OK
/// }
/// ```
#[proc_macro_attribute]
pub fn head(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("Head", attr, item)
}

/// Mark a function as an OPTIONS handler.
///
/// OPTIONS requests return allowed methods and CORS headers for a resource.
///
/// # Example
///
/// ```ignore
/// #[options("/items")]
/// async fn options_items() -> Response {
///     Response::builder()
///         .header("Allow", "GET, POST, OPTIONS")
///         .body(())
/// }
/// ```
#[proc_macro_attribute]
pub fn options(attr: TokenStream, item: TokenStream) -> TokenStream {
    route::route_impl("Options", attr, item)
}

/// Derive validation for a struct.
///
/// # Validation Attributes
///
/// - `#[validate(length(min = 1, max = 100))]` - String length
/// - `#[validate(range(min = 0.0, max = 1.0))]` - Numeric range
/// - `#[validate(email)]` - Email format
/// - `#[validate(regex = "pattern")]` - Regex pattern
///
/// # Example
///
/// ```ignore
/// #[derive(Validate)]
/// struct CreateUser {
///     #[validate(length(min = 1, max = 50))]
///     name: String,
///     #[validate(email)]
///     email: String,
/// }
/// ```
#[proc_macro_derive(Validate, attributes(validate))]
pub fn derive_validate(input: TokenStream) -> TokenStream {
    validate::derive_validate_impl(input)
}

/// Derive JSON Schema for OpenAPI.
///
/// # Example
///
/// ```ignore
/// #[derive(JsonSchema)]
/// struct Item {
///     id: i64,
///     name: String,
///     #[schema(nullable)]
///     description: Option<String>,
/// }
/// ```
#[proc_macro_derive(JsonSchema, attributes(schema))]
pub fn derive_json_schema(input: TokenStream) -> TokenStream {
    openapi::derive_json_schema_impl(input)
}
