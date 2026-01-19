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

#![warn(unsafe_code)]

mod r#match;
mod registry;
mod trie;

pub use r#match::{AllowedMethods, RouteLookup, RouteMatch};
pub use registry::{RouteRegistration, registered_routes};
pub use trie::{Converter, ParamInfo, Route, Router};
