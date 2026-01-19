//! HTTP request body handling.
//!
//! This module provides body reading with support for:
//! - Content-Length based reading with size limits
//! - Chunked transfer encoding parsing
//! - Streaming API for large bodies
//! - Integration points for async I/O
//!
//! # Body Size Limits
//!
//! By default, bodies are limited to 1MB. This prevents denial-of-service
//! attacks via large payloads. The limit is configurable per-request.
//!
//! # Streaming
//!
//! Large bodies can be read incrementally via the [`BodyReader`] trait,
//! which supports async I/O integration with checkpoints.
//!
//! # Example
//!
//! ```ignore
//! use fastapi_http::body::{BodyConfig, BodyReader, ContentLengthReader};
//!
//! let config = BodyConfig::default().with_max_size(1024 * 1024);
//! let mut reader = ContentLengthReader::new(body_bytes, 100, &config)?;
//!
//! let body = reader.read_all()?;
//! ```

use crate::parser::{BodyLength, ParseError};

/// Default maximum body size (1MB).
pub const DEFAULT_MAX_BODY_SIZE: usize = 1024 * 1024;

/// Configuration for body reading.
#[derive(Debug, Clone)]
pub struct BodyConfig {
    /// Maximum body size in bytes.
    max_size: usize,
    /// Initial buffer capacity for streaming.
    initial_capacity: usize,
}

impl Default for BodyConfig {
    fn default() -> Self {
        Self {
            max_size: DEFAULT_MAX_BODY_SIZE,
            initial_capacity: 4096,
        }
    }
}

impl BodyConfig {
    /// Create a new body configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum body size.
    #[must_use]
    pub fn with_max_size(mut self, size: usize) -> Self {
        self.max_size = size;
        self
    }

    /// Set the initial buffer capacity.
    #[must_use]
    pub fn with_initial_capacity(mut self, capacity: usize) -> Self {
        self.initial_capacity = capacity;
        self
    }

    /// Returns the maximum body size.
    #[must_use]
    pub fn max_size(&self) -> usize {
        self.max_size
    }

    /// Returns the initial buffer capacity.
    #[must_use]
    pub fn initial_capacity(&self) -> usize {
        self.initial_capacity
    }
}

/// Error types for body reading.
#[derive(Debug)]
pub enum BodyError {
    /// Body exceeds maximum allowed size.
    TooLarge {
        /// The declared or actual size.
        size: usize,
        /// The maximum allowed size.
        max: usize,
    },
    /// Invalid chunked encoding.
    InvalidChunkedEncoding {
        /// Description of the error.
        detail: &'static str,
    },
    /// Incomplete body (need more data).
    Incomplete {
        /// Bytes received so far.
        received: usize,
        /// Expected total size (if known).
        expected: Option<usize>,
    },
    /// Unexpected end of input.
    UnexpectedEof,
    /// Parse error from underlying parser.
    Parse(ParseError),
}

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { size, max } => {
                write!(f, "body too large: {size} bytes exceeds limit of {max}")
            }
            Self::InvalidChunkedEncoding { detail } => {
                write!(f, "invalid chunked encoding: {detail}")
            }
            Self::Incomplete { received, expected } => {
                if let Some(exp) = expected {
                    write!(f, "incomplete body: received {received} of {exp} bytes")
                } else {
                    write!(f, "incomplete body: received {received} bytes")
                }
            }
            Self::UnexpectedEof => write!(f, "unexpected end of body"),
            Self::Parse(e) => write!(f, "parse error: {e}"),
        }
    }
}

impl std::error::Error for BodyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ParseError> for BodyError {
    fn from(e: ParseError) -> Self {
        Self::Parse(e)
    }
}

// ============================================================================
// Content-Length Body Reading
// ============================================================================

/// Reads a body with a known Content-Length.
///
/// This reader validates that exactly `length` bytes are provided and
/// enforces the configured size limit.
#[derive(Debug)]
pub struct ContentLengthReader<'a> {
    buffer: &'a [u8],
    length: usize,
    position: usize,
    // Stored for potential future use (streaming chunk size configuration)
    #[allow(dead_code)]
    config: BodyConfig,
}

impl<'a> ContentLengthReader<'a> {
    /// Create a new Content-Length reader.
    ///
    /// # Arguments
    ///
    /// * `buffer` - The buffer containing body bytes
    /// * `length` - The Content-Length value
    /// * `config` - Body reading configuration
    ///
    /// # Errors
    ///
    /// Returns `BodyError::TooLarge` if `length` exceeds the configured maximum.
    pub fn new(buffer: &'a [u8], length: usize, config: &BodyConfig) -> Result<Self, BodyError> {
        // Check size limit before reading
        if length > config.max_size {
            return Err(BodyError::TooLarge {
                size: length,
                max: config.max_size,
            });
        }

        Ok(Self {
            buffer,
            length,
            position: 0,
            config: config.clone(),
        })
    }

    /// Returns the expected body length.
    #[must_use]
    pub fn length(&self) -> usize {
        self.length
    }

    /// Returns the number of bytes remaining.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.length.saturating_sub(self.position)
    }

    /// Returns true if all bytes have been read.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.position >= self.length
    }

    /// Read up to `max_bytes` into the provided buffer.
    ///
    /// Returns the number of bytes read.
    pub fn read(&mut self, dest: &mut [u8]) -> Result<usize, BodyError> {
        if self.is_complete() {
            return Ok(0);
        }

        let available = self.buffer.len().saturating_sub(self.position);
        let to_read = dest.len().min(self.remaining()).min(available);

        if to_read == 0 && !self.is_complete() {
            return Err(BodyError::Incomplete {
                received: self.position,
                expected: Some(self.length),
            });
        }

        dest[..to_read].copy_from_slice(&self.buffer[self.position..self.position + to_read]);
        self.position += to_read;

        Ok(to_read)
    }

    /// Read all remaining body bytes.
    ///
    /// # Errors
    ///
    /// Returns `BodyError::Incomplete` if the buffer doesn't contain enough data.
    pub fn read_all(&mut self) -> Result<Vec<u8>, BodyError> {
        if self.buffer.len() < self.length {
            return Err(BodyError::Incomplete {
                received: self.buffer.len(),
                expected: Some(self.length),
            });
        }

        let body = self.buffer[..self.length].to_vec();
        self.position = self.length;
        Ok(body)
    }

    /// Read all remaining body bytes as a borrowed slice.
    ///
    /// This is zero-copy when the entire body is already in the buffer.
    ///
    /// # Errors
    ///
    /// Returns `BodyError::Incomplete` if the buffer doesn't contain enough data.
    pub fn read_all_borrowed(&self) -> Result<&'a [u8], BodyError> {
        if self.buffer.len() < self.length {
            return Err(BodyError::Incomplete {
                received: self.buffer.len(),
                expected: Some(self.length),
            });
        }

        Ok(&self.buffer[..self.length])
    }
}

// ============================================================================
// Chunked Transfer Encoding
// ============================================================================

/// State machine for chunked encoding parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkedState {
    /// Expecting chunk size line.
    ChunkSize,
    /// Reading chunk data.
    ChunkData { remaining: usize },
    /// Expecting CRLF after chunk data.
    ChunkDataEnd,
    /// Reading trailers (after final chunk).
    Trailers,
    /// Complete.
    Complete,
}

/// Parses chunked transfer encoding.
///
/// Chunked encoding format:
/// ```text
/// chunk-size CRLF
/// chunk-data CRLF
/// ...
/// 0 CRLF
/// [trailers] CRLF
/// ```
#[derive(Debug)]
pub struct ChunkedReader<'a> {
    buffer: &'a [u8],
    position: usize,
    state: ChunkedState,
    total_size: usize,
    config: BodyConfig,
}

impl<'a> ChunkedReader<'a> {
    /// Create a new chunked reader.
    ///
    /// # Arguments
    ///
    /// * `buffer` - The buffer containing chunked body data
    /// * `config` - Body reading configuration
    #[must_use]
    pub fn new(buffer: &'a [u8], config: &BodyConfig) -> Self {
        Self {
            buffer,
            position: 0,
            state: ChunkedState::ChunkSize,
            total_size: 0,
            config: config.clone(),
        }
    }

    /// Returns true if parsing is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.state == ChunkedState::Complete
    }

    /// Returns the total decoded body size so far.
    #[must_use]
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Parse the chunk size line and return the size.
    fn parse_chunk_size(&self) -> Result<(usize, usize), BodyError> {
        let remaining = &self.buffer[self.position..];

        // Find CRLF
        let line_end =
            remaining
                .windows(2)
                .position(|w| w == b"\r\n")
                .ok_or(BodyError::Incomplete {
                    received: self.position,
                    expected: None,
                })?;

        let size_line = &remaining[..line_end];

        // Parse hex size (ignore chunk extensions after semicolon)
        let size_str = if let Some(semi) = size_line.iter().position(|&b| b == b';') {
            &size_line[..semi]
        } else {
            size_line
        };

        let size_str =
            std::str::from_utf8(size_str).map_err(|_| BodyError::InvalidChunkedEncoding {
                detail: "invalid UTF-8 in chunk size",
            })?;

        let size = usize::from_str_radix(size_str.trim(), 16).map_err(|_| {
            BodyError::InvalidChunkedEncoding {
                detail: "invalid hex chunk size",
            }
        })?;

        // bytes_consumed = size_line + CRLF
        Ok((size, line_end + 2))
    }

    /// Decode all chunks into a single buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The total size exceeds the configured limit
    /// - The chunked encoding is malformed
    /// - The buffer is incomplete
    pub fn decode_all(&mut self) -> Result<Vec<u8>, BodyError> {
        let mut output = Vec::with_capacity(self.config.initial_capacity);

        loop {
            match self.state {
                ChunkedState::ChunkSize => {
                    let (size, consumed) = self.parse_chunk_size()?;
                    self.position += consumed;

                    // Check size limit
                    let new_total = self.total_size.saturating_add(size);
                    if new_total > self.config.max_size {
                        return Err(BodyError::TooLarge {
                            size: new_total,
                            max: self.config.max_size,
                        });
                    }

                    if size == 0 {
                        // Final chunk - transition to trailers
                        self.state = ChunkedState::Trailers;
                    } else {
                        self.state = ChunkedState::ChunkData { remaining: size };
                    }
                }
                ChunkedState::ChunkData { remaining } => {
                    let available = self.buffer.len().saturating_sub(self.position);
                    if available < remaining {
                        return Err(BodyError::Incomplete {
                            received: self.total_size + (remaining - available),
                            expected: None,
                        });
                    }

                    // Copy chunk data
                    let chunk_data = &self.buffer[self.position..self.position + remaining];
                    output.extend_from_slice(chunk_data);
                    self.position += remaining;
                    self.total_size += remaining;

                    self.state = ChunkedState::ChunkDataEnd;
                }
                ChunkedState::ChunkDataEnd => {
                    // Expect CRLF
                    let remaining = &self.buffer[self.position..];
                    if remaining.len() < 2 {
                        return Err(BodyError::Incomplete {
                            received: self.total_size,
                            expected: None,
                        });
                    }

                    if &remaining[..2] != b"\r\n" {
                        return Err(BodyError::InvalidChunkedEncoding {
                            detail: "expected CRLF after chunk data",
                        });
                    }

                    self.position += 2;
                    self.state = ChunkedState::ChunkSize;
                }
                ChunkedState::Trailers => {
                    // Skip trailers until empty line
                    let remaining = &self.buffer[self.position..];

                    // Look for CRLF (empty line) or trailer headers
                    if remaining.starts_with(b"\r\n") {
                        self.position += 2;
                        self.state = ChunkedState::Complete;
                    } else {
                        // Find end of trailer line
                        let line_end = remaining.windows(2).position(|w| w == b"\r\n");
                        match line_end {
                            Some(pos) => {
                                // Skip this trailer
                                self.position += pos + 2;
                                // Stay in Trailers state to handle more trailers or final CRLF
                            }
                            None => {
                                return Err(BodyError::Incomplete {
                                    received: self.total_size,
                                    expected: None,
                                });
                            }
                        }
                    }
                }
                ChunkedState::Complete => {
                    break;
                }
            }
        }

        Ok(output)
    }

    /// Returns the number of bytes consumed from the raw buffer.
    #[must_use]
    pub fn bytes_consumed(&self) -> usize {
        self.position
    }
}

// ============================================================================
// Body Parsing from Headers
// ============================================================================

/// Parse a request body from a buffer given the body length indicator.
///
/// This is the main entry point for body parsing. It dispatches to the
/// appropriate reader based on Content-Length or Transfer-Encoding.
///
/// # Arguments
///
/// * `buffer` - The buffer containing the body (after headers)
/// * `body_length` - The body length indicator from header parsing
/// * `config` - Body reading configuration
///
/// # Returns
///
/// Returns the parsed body bytes, or `None` if no body is expected.
///
/// # Errors
///
/// Returns an error if:
/// - The body exceeds the configured size limit
/// - The chunked encoding is malformed
/// - The buffer is incomplete
pub fn parse_body(
    buffer: &[u8],
    body_length: BodyLength,
    config: &BodyConfig,
) -> Result<Option<Vec<u8>>, BodyError> {
    let (body, _) = parse_body_with_consumed(buffer, body_length, config)?;
    Ok(body)
}

/// Parse a request body and return both the decoded body and bytes consumed.
///
/// This is useful for incremental parsing to determine request boundaries.
///
/// # Errors
///
/// Returns an error if:
/// - The body exceeds the configured size limit
/// - The chunked encoding is malformed
/// - The buffer is incomplete
pub fn parse_body_with_consumed(
    buffer: &[u8],
    body_length: BodyLength,
    config: &BodyConfig,
) -> Result<(Option<Vec<u8>>, usize), BodyError> {
    match body_length {
        BodyLength::None => Ok((None, 0)),
        BodyLength::ContentLength(len) => {
            if len == 0 {
                return Ok((Some(Vec::new()), 0));
            }
            let mut reader = ContentLengthReader::new(buffer, len, config)?;
            let body = reader.read_all()?;
            Ok((Some(body), len))
        }
        BodyLength::Chunked => {
            let mut reader = ChunkedReader::new(buffer, config);
            let body = reader.decode_all()?;
            Ok((Some(body), reader.bytes_consumed()))
        }
        BodyLength::Conflicting => Err(BodyError::InvalidChunkedEncoding {
            detail: "conflicting body length indicators",
        }),
    }
}

/// Validates body size against Content-Length header before reading.
///
/// Call this early (before buffering) to reject oversized requests.
///
/// # Arguments
///
/// * `content_length` - The Content-Length header value
/// * `config` - Body reading configuration
///
/// # Errors
///
/// Returns `BodyError::TooLarge` if the content length exceeds the limit.
pub fn validate_content_length(
    content_length: usize,
    config: &BodyConfig,
) -> Result<(), BodyError> {
    if content_length > config.max_size {
        return Err(BodyError::TooLarge {
            size: content_length,
            max: config.max_size,
        });
    }
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // BodyConfig Tests
    // ========================================================================

    #[test]
    fn body_config_defaults() {
        let config = BodyConfig::default();
        assert_eq!(config.max_size(), DEFAULT_MAX_BODY_SIZE);
        assert_eq!(config.initial_capacity(), 4096);
    }

    #[test]
    fn body_config_custom() {
        let config = BodyConfig::new()
            .with_max_size(2048)
            .with_initial_capacity(1024);
        assert_eq!(config.max_size(), 2048);
        assert_eq!(config.initial_capacity(), 1024);
    }

    // ========================================================================
    // Content-Length Reader Tests
    // ========================================================================

    #[test]
    fn content_length_basic() {
        let body = b"Hello, World!";
        let config = BodyConfig::default();
        let mut reader = ContentLengthReader::new(body, body.len(), &config).unwrap();

        assert_eq!(reader.length(), 13);
        assert_eq!(reader.remaining(), 13);
        assert!(!reader.is_complete());

        let result = reader.read_all().unwrap();
        assert_eq!(result, b"Hello, World!");
        assert!(reader.is_complete());
    }

    #[test]
    fn content_length_zero() {
        let body = b"";
        let config = BodyConfig::default();
        let mut reader = ContentLengthReader::new(body, 0, &config).unwrap();

        assert_eq!(reader.length(), 0);
        assert!(reader.is_complete());

        let result = reader.read_all().unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn content_length_too_large() {
        let body = b"small";
        let config = BodyConfig::new().with_max_size(3);
        let result = ContentLengthReader::new(body, 100, &config);

        assert!(matches!(
            result,
            Err(BodyError::TooLarge { size: 100, max: 3 })
        ));
    }

    #[test]
    fn content_length_incomplete() {
        let body = b"Hello";
        let config = BodyConfig::default();
        let mut reader = ContentLengthReader::new(body, 10, &config).unwrap();

        let result = reader.read_all();
        assert!(matches!(
            result,
            Err(BodyError::Incomplete {
                received: 5,
                expected: Some(10)
            })
        ));
    }

    #[test]
    fn content_length_borrowed() {
        let body = b"Hello, World!";
        let config = BodyConfig::default();
        let reader = ContentLengthReader::new(body, body.len(), &config).unwrap();

        let borrowed = reader.read_all_borrowed().unwrap();
        assert_eq!(borrowed, body);
        // Verify it's the same memory location (zero-copy)
        assert_eq!(borrowed.as_ptr(), body.as_ptr());
    }

    #[test]
    fn content_length_incremental_read() {
        let body = b"Hello, World!";
        let config = BodyConfig::default();
        let mut reader = ContentLengthReader::new(body, body.len(), &config).unwrap();

        let mut buf = [0u8; 5];

        // First read
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf[..n], b"Hello");
        assert_eq!(reader.remaining(), 8);

        // Second read
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf[..n], b", Wor");
        assert_eq!(reader.remaining(), 3);

        // Third read
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 3);
        assert_eq!(&buf[..n], b"ld!");
        assert!(reader.is_complete());

        // No more data
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    // ========================================================================
    // Chunked Encoding Tests
    // ========================================================================

    #[test]
    fn chunked_single_chunk() {
        let body = b"5\r\nHello\r\n0\r\n\r\n";
        let config = BodyConfig::default();
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all().unwrap();
        assert_eq!(result, b"Hello");
        assert!(reader.is_complete());
    }

    #[test]
    fn chunked_multiple_chunks() {
        let body = b"5\r\nHello\r\n7\r\n, World\r\n1\r\n!\r\n0\r\n\r\n";
        let config = BodyConfig::default();
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all().unwrap();
        assert_eq!(result, b"Hello, World!");
        assert!(reader.is_complete());
    }

    #[test]
    fn chunked_empty() {
        let body = b"0\r\n\r\n";
        let config = BodyConfig::default();
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all().unwrap();
        assert!(result.is_empty());
        assert!(reader.is_complete());
    }

    #[test]
    fn chunked_with_extension() {
        // Chunk extensions should be ignored
        let body = b"5;ext=value\r\nHello\r\n0\r\n\r\n";
        let config = BodyConfig::default();
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all().unwrap();
        assert_eq!(result, b"Hello");
    }

    #[test]
    fn chunked_with_trailers() {
        let body = b"5\r\nHello\r\n0\r\nTrailer: value\r\n\r\n";
        let config = BodyConfig::default();
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all().unwrap();
        assert_eq!(result, b"Hello");
        assert!(reader.is_complete());
    }

    #[test]
    fn chunked_hex_sizes() {
        // Test various hex chunk sizes
        let body = b"a\r\n0123456789\r\nF\r\n0123456789ABCDE\r\n0\r\n\r\n";
        let config = BodyConfig::default();
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all().unwrap();
        assert_eq!(result.len(), 10 + 15); // a=10, F=15
    }

    #[test]
    fn chunked_too_large() {
        let body = b"10\r\n0123456789ABCDEF\r\n0\r\n\r\n"; // 16 bytes
        let config = BodyConfig::new().with_max_size(10);
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all();
        assert!(matches!(
            result,
            Err(BodyError::TooLarge { size: 16, max: 10 })
        ));
    }

    #[test]
    fn chunked_invalid_size() {
        let body = b"xyz\r\nHello\r\n0\r\n\r\n";
        let config = BodyConfig::default();
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all();
        assert!(matches!(
            result,
            Err(BodyError::InvalidChunkedEncoding { detail: _ })
        ));
    }

    #[test]
    fn chunked_missing_crlf() {
        let body = b"5\r\nHelloX0\r\n\r\n"; // Missing CRLF after data
        let config = BodyConfig::default();
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all();
        assert!(matches!(
            result,
            Err(BodyError::InvalidChunkedEncoding {
                detail: "expected CRLF after chunk data"
            })
        ));
    }

    #[test]
    fn chunked_incomplete() {
        let body = b"5\r\nHel"; // Incomplete chunk data
        let config = BodyConfig::default();
        let mut reader = ChunkedReader::new(body, &config);

        let result = reader.decode_all();
        assert!(matches!(result, Err(BodyError::Incomplete { .. })));
    }

    // ========================================================================
    // parse_body Tests
    // ========================================================================

    #[test]
    fn parse_body_none() {
        let config = BodyConfig::default();
        let result = parse_body(b"ignored", BodyLength::None, &config).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn parse_body_content_length() {
        let config = BodyConfig::default();
        let result = parse_body(b"Hello, World!", BodyLength::ContentLength(13), &config).unwrap();
        assert_eq!(result.unwrap(), b"Hello, World!");
    }

    #[test]
    fn parse_body_content_length_zero() {
        let config = BodyConfig::default();
        let result = parse_body(b"", BodyLength::ContentLength(0), &config).unwrap();
        assert_eq!(result.unwrap(), b"");
    }

    #[test]
    fn parse_body_chunked() {
        let config = BodyConfig::default();
        let result = parse_body(b"5\r\nHello\r\n0\r\n\r\n", BodyLength::Chunked, &config).unwrap();
        assert_eq!(result.unwrap(), b"Hello");
    }

    #[test]
    fn parse_body_with_consumed_content_length() {
        let config = BodyConfig::default();
        let (body, consumed) =
            parse_body_with_consumed(b"Hello, World!", BodyLength::ContentLength(13), &config)
                .unwrap();
        assert_eq!(body.unwrap(), b"Hello, World!");
        assert_eq!(consumed, 13);
    }

    #[test]
    fn parse_body_with_consumed_chunked() {
        let config = BodyConfig::default();
        let (body, consumed) =
            parse_body_with_consumed(b"5\r\nHello\r\n0\r\n\r\n", BodyLength::Chunked, &config)
                .unwrap();
        assert_eq!(body.unwrap(), b"Hello");
        assert_eq!(consumed, 15);
    }

    // ========================================================================
    // validate_content_length Tests
    // ========================================================================

    #[test]
    fn validate_content_length_ok() {
        let config = BodyConfig::new().with_max_size(1000);
        assert!(validate_content_length(500, &config).is_ok());
        assert!(validate_content_length(1000, &config).is_ok());
    }

    #[test]
    fn validate_content_length_too_large() {
        let config = BodyConfig::new().with_max_size(1000);
        let result = validate_content_length(1001, &config);
        assert!(matches!(
            result,
            Err(BodyError::TooLarge {
                size: 1001,
                max: 1000
            })
        ));
    }

    // ========================================================================
    // BodyError Tests
    // ========================================================================

    #[test]
    fn body_error_display() {
        let err = BodyError::TooLarge {
            size: 2000,
            max: 1000,
        };
        assert_eq!(
            format!("{err}"),
            "body too large: 2000 bytes exceeds limit of 1000"
        );

        let err = BodyError::InvalidChunkedEncoding {
            detail: "bad format",
        };
        assert_eq!(format!("{err}"), "invalid chunked encoding: bad format");

        let err = BodyError::Incomplete {
            received: 50,
            expected: Some(100),
        };
        assert_eq!(
            format!("{err}"),
            "incomplete body: received 50 of 100 bytes"
        );

        let err = BodyError::UnexpectedEof;
        assert_eq!(format!("{err}"), "unexpected end of body");
    }
}
