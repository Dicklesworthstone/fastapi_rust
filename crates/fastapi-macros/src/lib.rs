//! Procedural macros for fastapi_rust.
//!
//! This crate provides the following macros:
//!
//! - Route macros: `#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]`
//! - `#[derive(Validate)]` for compile-time validation
//! - `#[derive(JsonSchema)]` for OpenAPI schema generation
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
