//! OpenAPI 3.1 types and schema generation.
//!
//! This crate provides:
//!
//! - OpenAPI 3.1 document types
//! - JSON Schema types
//! - `JsonSchema` trait for compile-time schema generation
//!
//! # Example
//!
//! ```ignore
//! use fastapi_openapi::{OpenApiBuilder, JsonSchema};
//!
//! #[derive(JsonSchema)]
//! struct Item {
//!     id: i64,
//!     name: String,
//! }
//!
//! let spec = OpenApiBuilder::new("My API", "1.0.0")
//!     .route(&get_items_route())
//!     .build();
//! ```

#![forbid(unsafe_code)]
// Pedantic clippy lints allowed (style suggestions, not correctness issues)
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::single_match)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::len_zero)]
#![allow(clippy::single_match_else)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::trivially_copy_pass_by_ref)]

mod schema;
mod spec;

pub use schema::{
    ArraySchema, JsonSchema, ObjectSchema, PrimitiveSchema, RefSchema, Schema, SchemaType,
};
pub use spec::{
    Components, Example, HasParamMeta, Info, MediaType, OpenApi, OpenApiBuilder, Operation,
    ParamMeta, Parameter, ParameterLocation, PathItem, RequestBody, Response, Server, Tag,
};
