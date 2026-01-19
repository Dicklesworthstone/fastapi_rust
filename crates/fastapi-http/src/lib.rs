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
