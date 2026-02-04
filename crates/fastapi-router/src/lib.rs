//! Trie-based HTTP router.
//!
//! This crate provides a high-performance radix trie router optimized
//! for the fastapi_rust framework.
//!
//! # Features
//!
//! - Radix trie for fast lookups
//! - Path parameter extraction (`/items/{id}`)
//! - Type-safe path converters
//! - Static route optimization
//!
//! # Role In The System
//!
//! `fastapi-router` is responsible for matching incoming paths to handlers and
//! extracting typed parameters. `fastapi-core` uses it for request dispatch,
//! while `fastapi-openapi` reads its parameter metadata when generating specs.
//! Keeping the router isolated allows the rest of the framework to stay focused
//! on request semantics rather than path matching internals.

#![warn(unsafe_code)]
// Pedantic clippy lints allowed (style suggestions, not correctness issues)
#![allow(clippy::needless_lifetimes)]
#![allow(clippy::elidable_lifetime_names)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::cast_ptr_alignment)]
#![allow(clippy::ptr_as_ptr)]
#![allow(clippy::borrow_as_ptr)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::single_match_else)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_fields_in_debug)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::ref_as_ptr)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::match_wildcard_for_single_variants)]
#![allow(clippy::needless_borrows_for_generic_args)]

mod r#match;
mod registry;
mod trie;

pub use r#match::{AllowedMethods, RouteLookup, RouteMatch};
pub use registry::{RouteRegistration, registered_routes};
pub use trie::{
    ConversionError, Converter, InvalidRouteError, ParamInfo, ParamValue, Route, RouteAddError,
    RouteConflictError, RouteResponse, RouteSecurityRequirement, Router, extract_path_params,
};
