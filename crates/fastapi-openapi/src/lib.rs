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

mod schema;
mod spec;

pub use schema::{
    ArraySchema, JsonSchema, ObjectSchema, PrimitiveSchema, RefSchema, Schema, SchemaType,
};
pub use spec::{
    Components, Example, HasParamMeta, Info, MediaType, OpenApi, OpenApiBuilder, Operation,
    ParamMeta, Parameter, ParameterLocation, PathItem, RequestBody, Response, Server, Tag,
};
