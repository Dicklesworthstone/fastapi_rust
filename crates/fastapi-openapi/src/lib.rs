//! OpenAPI 3.1 types and schema generation.
//!
//! This crate provides:
//!
//! - OpenAPI 3.1 document types
//! - JSON Schema types
//! - `JsonSchema` trait for compile-time schema generation
//!
//! # Role In The System
//!
//! `fastapi-openapi` owns the OpenAPI 3.1 and JSON Schema data model. It is
//! fed by metadata emitted from `fastapi-macros` and by runtime route info from
//! `fastapi-core`/`fastapi-router`. The resulting spec is then exposed through
//! the `fastapi` facade so applications can serve or export documentation.
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
    ArraySchema, ConstSchema, Discriminator, EnumSchema, JsonSchema, ObjectSchema, PrimitiveSchema,
    RefSchema, Schema, SchemaRegistry, SchemaType, StringEnumSchema,
};
pub use spec::{
    // Security types
    ApiKeyLocation,
    // Core document types
    Components,
    Example,
    HasParamMeta,
    Info,
    MediaType,
    OAuth2Flow,
    OAuth2Flows,
    OpenApi,
    OpenApiBuilder,
    Operation,
    ParamMeta,
    Parameter,
    ParameterLocation,
    PathItem,
    RequestBody,
    Response,
    SecurityRequirement,
    SecurityScheme,
    Server,
    Tag,
    // Path parameter generation
    converter_to_schema,
    param_info_to_parameter,
    path_params_to_parameters,
};
