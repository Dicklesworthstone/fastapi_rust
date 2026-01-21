//! Zero-copy HTTP/1.1 parser.
//!
//! This crate provides a minimal, zero-copy HTTP parser optimized for
//! the fastapi_rust framework. It parses directly from byte buffers
//! without allocating for most operations.
//!
//! # Features
//!
//! - Zero-copy request parsing
//! - HTTP/1.1 compliance (subset)
//! - Response building with pre-allocated buffers
//! - Request body handling (Content-Length and chunked encoding)
//! - Query string parsing with percent-decoding
//! - Streaming response support
//!
//! # Example
//!
//! ```ignore
//! use fastapi_http::Parser;
//!
//! let bytes = b"GET /path HTTP/1.1\r\nHost: example.com\r\n\r\n";
//! let request = Parser::parse(bytes)?;
//! ```

#![deny(unsafe_code)]
// Pedantic clippy lints allowed (style suggestions, not correctness issues)
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::single_char_pattern)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::match_wildcard_for_single_variants)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::single_match_else)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::manual_strip)]
#![allow(clippy::items_after_statements)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::needless_borrow)]
#![allow(clippy::match_wild_err_arm)]
#![allow(clippy::format_push_string)]
#![allow(clippy::match_wildcard_for_single_variants)]
#![allow(clippy::manual_contains)]
#![allow(clippy::needless_borrows_for_generic_args)]
#![allow(clippy::iter_without_into_iter)]
#![allow(clippy::single_match)]
#![allow(clippy::len_zero)]
#![allow(clippy::len_without_is_empty)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::derivable_impls)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::needless_borrows_for_generic_args)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::unused_async)]
#![allow(clippy::used_underscore_binding)]
#![allow(clippy::duplicated_attributes)]

pub mod body;
mod parser;
mod query;
mod response;
mod server;
pub mod streaming;

pub use body::{
    BodyConfig, BodyError, ChunkedReader, ContentLengthReader, DEFAULT_MAX_BODY_SIZE, parse_body,
    parse_body_with_consumed, validate_content_length,
};
pub use parser::{
    BodyLength, Header, HeadersIter, HeadersParser, ParseError, ParseLimits, ParseStatus, Parser,
    RequestLine, StatefulParser,
};
pub use query::{QueryString, percent_decode};
pub use response::{ChunkedEncoder, ResponseWrite, ResponseWriter};
pub use server::Server;
pub use streaming::{
    CancelAwareStream, ChunkedBytes, DEFAULT_CHUNK_SIZE, DEFAULT_MAX_BUFFER_SIZE, FileStream,
    StreamConfig, StreamError, StreamingResponseExt,
};
