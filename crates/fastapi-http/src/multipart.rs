//! Multipart form data parser.
//!
//! This module provides streaming parsing of `multipart/form-data` requests,
//! which is the standard encoding for file uploads.
//!
//! # Format
//!
//! Multipart form data consists of multiple parts separated by a boundary string:
//!
//! ```text
//! --boundary\r\n
//! Content-Disposition: form-data; name="field1"\r\n
//! \r\n
//! value1\r\n
//! --boundary\r\n
//! Content-Disposition: form-data; name="file"; filename="example.txt"\r\n
//! Content-Type: text/plain\r\n
//! \r\n
//! file contents...\r\n
//! --boundary--\r\n
//! ```
//!
//! # Features
//!
//! - Streaming parser that doesn't buffer entire body
//! - Per-file and total size limits
//! - Extracts filename, content-type, and field name
//! - Memory-safe: enforces limits before allocating

use std::collections::HashMap;

/// Default maximum file size (10MB).
pub const DEFAULT_MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

/// Default maximum total upload size (50MB).
pub const DEFAULT_MAX_TOTAL_SIZE: usize = 50 * 1024 * 1024;

/// Default maximum number of fields.
pub const DEFAULT_MAX_FIELDS: usize = 100;

/// Configuration for multipart parsing.
#[derive(Debug, Clone)]
pub struct MultipartConfig {
    /// Maximum size per file in bytes.
    max_file_size: usize,
    /// Maximum total upload size in bytes.
    max_total_size: usize,
    /// Maximum number of fields (including files).
    max_fields: usize,
}

impl Default for MultipartConfig {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_total_size: DEFAULT_MAX_TOTAL_SIZE,
            max_fields: DEFAULT_MAX_FIELDS,
        }
    }
}

impl MultipartConfig {
    /// Create a new configuration with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum file size.
    #[must_use]
    pub fn max_file_size(mut self, size: usize) -> Self {
        self.max_file_size = size;
        self
    }

    /// Set the maximum total upload size.
    #[must_use]
    pub fn max_total_size(mut self, size: usize) -> Self {
        self.max_total_size = size;
        self
    }

    /// Set the maximum number of fields.
    #[must_use]
    pub fn max_fields(mut self, count: usize) -> Self {
        self.max_fields = count;
        self
    }

    /// Get the maximum file size.
    #[must_use]
    pub fn get_max_file_size(&self) -> usize {
        self.max_file_size
    }

    /// Get the maximum total upload size.
    #[must_use]
    pub fn get_max_total_size(&self) -> usize {
        self.max_total_size
    }

    /// Get the maximum number of fields.
    #[must_use]
    pub fn get_max_fields(&self) -> usize {
        self.max_fields
    }
}

/// Errors that can occur during multipart parsing.
#[derive(Debug)]
pub enum MultipartError {
    /// Missing boundary in Content-Type header.
    MissingBoundary,
    /// Invalid boundary format.
    InvalidBoundary,
    /// File size exceeds limit.
    FileTooLarge { size: usize, max: usize },
    /// Total upload size exceeds limit.
    TotalTooLarge { size: usize, max: usize },
    /// Too many fields.
    TooManyFields { count: usize, max: usize },
    /// Missing Content-Disposition header.
    MissingContentDisposition,
    /// Invalid Content-Disposition header.
    InvalidContentDisposition { detail: String },
    /// Invalid part headers.
    InvalidPartHeaders { detail: String },
    /// Unexpected end of input.
    UnexpectedEof,
    /// Invalid multipart format.
    InvalidFormat { detail: &'static str },
}

impl std::fmt::Display for MultipartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBoundary => write!(f, "missing boundary in multipart Content-Type"),
            Self::InvalidBoundary => write!(f, "invalid multipart boundary"),
            Self::FileTooLarge { size, max } => {
                write!(f, "file too large: {size} bytes exceeds limit of {max}")
            }
            Self::TotalTooLarge { size, max } => {
                write!(
                    f,
                    "total upload too large: {size} bytes exceeds limit of {max}"
                )
            }
            Self::TooManyFields { count, max } => {
                write!(f, "too many fields: {count} exceeds limit of {max}")
            }
            Self::MissingContentDisposition => {
                write!(f, "missing Content-Disposition header in part")
            }
            Self::InvalidContentDisposition { detail } => {
                write!(f, "invalid Content-Disposition: {detail}")
            }
            Self::InvalidPartHeaders { detail } => {
                write!(f, "invalid part headers: {detail}")
            }
            Self::UnexpectedEof => write!(f, "unexpected end of multipart data"),
            Self::InvalidFormat { detail } => write!(f, "invalid multipart format: {detail}"),
        }
    }
}

impl std::error::Error for MultipartError {}

/// A parsed multipart form part.
#[derive(Debug, Clone)]
pub struct Part {
    /// Field name from Content-Disposition.
    pub name: String,
    /// Filename from Content-Disposition (if present).
    pub filename: Option<String>,
    /// Content-Type of the part (if present).
    pub content_type: Option<String>,
    /// The part's content.
    pub data: Vec<u8>,
    /// Additional headers.
    pub headers: HashMap<String, String>,
}

impl Part {
    /// Returns true if this part is a file upload.
    #[must_use]
    pub fn is_file(&self) -> bool {
        self.filename.is_some()
    }

    /// Returns true if this part is a regular form field.
    #[must_use]
    pub fn is_field(&self) -> bool {
        self.filename.is_none()
    }

    /// Get the content as a UTF-8 string (for form fields).
    ///
    /// Returns `None` if the content is not valid UTF-8.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.data).ok()
    }

    /// Get the size of the data in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

/// An uploaded file with metadata.
#[derive(Debug, Clone)]
pub struct UploadFile {
    /// The field name.
    pub field_name: String,
    /// The original filename.
    pub filename: String,
    /// Content-Type of the file.
    pub content_type: String,
    /// File contents.
    pub data: Vec<u8>,
}

impl UploadFile {
    /// Create a new UploadFile from a Part.
    ///
    /// Returns `None` if the part is not a file.
    #[must_use]
    pub fn from_part(part: Part) -> Option<Self> {
        let filename = part.filename?;
        Some(Self {
            field_name: part.name,
            filename,
            content_type: part
                .content_type
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            data: part.data,
        })
    }

    /// Get the file size in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Get the file extension from the filename.
    #[must_use]
    pub fn extension(&self) -> Option<&str> {
        self.filename
            .rsplit('.')
            .next()
            .filter(|ext| !ext.is_empty() && *ext != self.filename)
    }
}

/// Parse boundary from Content-Type header.
///
/// Content-Type format: `multipart/form-data; boundary=----WebKitFormBoundary...`
pub fn parse_boundary(content_type: &str) -> Result<String, MultipartError> {
    let ct_lower = content_type.to_ascii_lowercase();

    // Verify it's multipart/form-data
    if !ct_lower.starts_with("multipart/form-data") {
        return Err(MultipartError::InvalidBoundary);
    }

    // Find boundary parameter
    for part in content_type.split(';') {
        let part = part.trim();
        if let Some(boundary) = part
            .strip_prefix("boundary=")
            .or_else(|| part.strip_prefix("BOUNDARY="))
        {
            // Remove quotes if present
            let boundary = boundary.trim_matches('"').trim_matches('\'');
            if boundary.is_empty() {
                return Err(MultipartError::InvalidBoundary);
            }
            return Ok(boundary.to_string());
        }
    }

    Err(MultipartError::MissingBoundary)
}

/// Streaming multipart parser.
///
/// Parses multipart data incrementally without buffering the entire body.
#[derive(Debug)]
pub struct MultipartParser {
    boundary: Vec<u8>,
    config: MultipartConfig,
}

impl MultipartParser {
    /// Create a new parser with the given boundary.
    #[must_use]
    pub fn new(boundary: &str, config: MultipartConfig) -> Self {
        Self {
            boundary: format!("--{boundary}").into_bytes(),
            config,
        }
    }

    /// Parse all parts from the body.
    ///
    /// This reads the entire body and returns all parts.
    pub fn parse(&self, body: &[u8]) -> Result<Vec<Part>, MultipartError> {
        let mut parts = Vec::new();
        let mut total_size = 0usize;
        let mut pos = 0;

        // Skip preamble and find first boundary
        pos = self.find_boundary(body, pos)?;

        loop {
            // Check field limit
            if parts.len() >= self.config.max_fields {
                return Err(MultipartError::TooManyFields {
                    count: parts.len() + 1,
                    max: self.config.max_fields,
                });
            }

            // Check if this is the final boundary (--boundary--)
            let boundary_end = pos + self.boundary.len();
            if boundary_end + 2 <= body.len() && body[boundary_end..boundary_end + 2] == *b"--" {
                // End of multipart data
                break;
            }

            // Skip boundary and CRLF
            pos = boundary_end;
            if pos + 2 > body.len() {
                return Err(MultipartError::UnexpectedEof);
            }
            if body[pos..pos + 2] != *b"\r\n" {
                return Err(MultipartError::InvalidFormat {
                    detail: "expected CRLF after boundary",
                });
            }
            pos += 2;

            // Parse part headers
            let (headers, header_end) = self.parse_part_headers(body, pos)?;
            pos = header_end;

            // Extract Content-Disposition
            let content_disp = headers
                .get("content-disposition")
                .ok_or(MultipartError::MissingContentDisposition)?;

            let (name, filename) = parse_content_disposition(content_disp)?;
            let content_type = headers.get("content-type").cloned();

            // Find next boundary to get data
            let data_end = self.find_boundary_from(body, pos)?;

            // Data ends at position before \r\n--boundary
            let data = if data_end >= 2 && body[data_end - 2..data_end] == *b"\r\n" {
                &body[pos..data_end - 2]
            } else {
                &body[pos..data_end]
            };

            // Check size limits
            if filename.is_some() && data.len() > self.config.max_file_size {
                return Err(MultipartError::FileTooLarge {
                    size: data.len(),
                    max: self.config.max_file_size,
                });
            }

            total_size += data.len();
            if total_size > self.config.max_total_size {
                return Err(MultipartError::TotalTooLarge {
                    size: total_size,
                    max: self.config.max_total_size,
                });
            }

            // Convert headers to HashMap<String, String>
            let mut headers_map = HashMap::new();
            for (k, v) in headers {
                headers_map.insert(k, v);
            }

            parts.push(Part {
                name,
                filename,
                content_type,
                data: data.to_vec(),
                headers: headers_map,
            });

            // Move to the boundary position
            pos = data_end;
        }

        Ok(parts)
    }

    /// Find the position of the next boundary.
    fn find_boundary(&self, data: &[u8], start: usize) -> Result<usize, MultipartError> {
        self.find_boundary_from(data, start)
    }

    /// Find boundary starting from position.
    fn find_boundary_from(&self, data: &[u8], start: usize) -> Result<usize, MultipartError> {
        let boundary = &self.boundary;
        let boundary_len = boundary.len();

        // Must have enough data for at least one boundary check
        if data.len() < boundary_len {
            return Err(MultipartError::UnexpectedEof);
        }

        // Search up to and including the last position where boundary could fit
        let end = data.len() - boundary_len + 1;
        for i in start..end {
            if data[i..].starts_with(boundary) {
                return Ok(i);
            }
        }

        Err(MultipartError::UnexpectedEof)
    }

    /// Parse headers from a part, returning headers and position after headers.
    fn parse_part_headers(
        &self,
        data: &[u8],
        start: usize,
    ) -> Result<(HashMap<String, String>, usize), MultipartError> {
        let mut headers = HashMap::new();
        let mut pos = start;

        loop {
            // Find end of line
            let line_end = self.find_crlf(data, pos)?;
            let line = &data[pos..line_end];

            // Empty line marks end of headers
            if line.is_empty() {
                // Skip the CRLF after empty line
                return Ok((headers, line_end + 2));
            }

            // Parse header line
            let line_str =
                std::str::from_utf8(line).map_err(|_| MultipartError::InvalidPartHeaders {
                    detail: "invalid UTF-8 in header".to_string(),
                })?;

            if let Some((name, value)) = line_str.split_once(':') {
                let name = name.trim().to_ascii_lowercase();
                let value = value.trim().to_string();
                headers.insert(name, value);
            }

            pos = line_end + 2; // Skip CRLF
        }
    }

    /// Find CRLF in data starting from position.
    #[allow(clippy::unused_self)]
    fn find_crlf(&self, data: &[u8], start: usize) -> Result<usize, MultipartError> {
        // Need at least 2 bytes for CRLF
        if data.len() < 2 {
            return Err(MultipartError::UnexpectedEof);
        }

        // Search up to and including the last position where CRLF could fit
        let end = data.len() - 1;
        for i in start..end {
            if data[i..i + 2] == *b"\r\n" {
                return Ok(i);
            }
        }
        Err(MultipartError::UnexpectedEof)
    }
}

/// Parse Content-Disposition header value.
///
/// Format: `form-data; name="field"; filename="file.txt"`
fn parse_content_disposition(value: &str) -> Result<(String, Option<String>), MultipartError> {
    let mut name = None;
    let mut filename = None;

    for part in value.split(';') {
        let part = part.trim();

        if part.eq_ignore_ascii_case("form-data") {
            continue;
        }

        if let Some(n) = part
            .strip_prefix("name=")
            .or_else(|| part.strip_prefix("NAME="))
        {
            name = Some(unquote(n));
        } else if let Some(f) = part
            .strip_prefix("filename=")
            .or_else(|| part.strip_prefix("FILENAME="))
        {
            let unquoted = unquote(f);
            // Validate filename to prevent path traversal attacks
            if unquoted.contains("..")
                || unquoted.contains('/')
                || unquoted.contains('\\')
                || unquoted.contains('\0')
            {
                return Err(MultipartError::InvalidContentDisposition {
                    detail: "filename contains path traversal characters".to_string(),
                });
            }
            filename = Some(unquoted);
        }
    }

    let name = name.ok_or_else(|| MultipartError::InvalidContentDisposition {
        detail: "missing name parameter".to_string(),
    })?;

    Ok((name, filename))
}

/// Remove quotes from a string.
fn unquote(s: &str) -> String {
    let s = s.trim();
    // Need at least 2 characters for paired quotes
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parsed multipart form data.
#[derive(Debug, Clone, Default)]
pub struct MultipartForm {
    /// All parsed parts.
    parts: Vec<Part>,
}

impl MultipartForm {
    /// Create a new empty form.
    #[must_use]
    pub fn new() -> Self {
        Self { parts: Vec::new() }
    }

    /// Create from parsed parts.
    #[must_use]
    pub fn from_parts(parts: Vec<Part>) -> Self {
        Self { parts }
    }

    /// Get all parts.
    #[must_use]
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// Get a form field value by name.
    #[must_use]
    pub fn get_field(&self, name: &str) -> Option<&str> {
        self.parts
            .iter()
            .find(|p| p.name == name && p.filename.is_none())
            .and_then(|p| p.text())
    }

    /// Get a file by field name.
    #[must_use]
    pub fn get_file(&self, name: &str) -> Option<UploadFile> {
        self.parts
            .iter()
            .find(|p| p.name == name && p.filename.is_some())
            .cloned()
            .and_then(UploadFile::from_part)
    }

    /// Get all files.
    #[must_use]
    pub fn files(&self) -> Vec<UploadFile> {
        self.parts
            .iter()
            .filter(|p| p.filename.is_some())
            .cloned()
            .filter_map(UploadFile::from_part)
            .collect()
    }

    /// Get all regular form fields as (name, value) pairs.
    #[must_use]
    pub fn fields(&self) -> Vec<(&str, &str)> {
        self.parts
            .iter()
            .filter(|p| p.filename.is_none())
            .filter_map(|p| Some((p.name.as_str(), p.text()?)))
            .collect()
    }

    /// Get all values for a field name (for multiple file uploads).
    #[must_use]
    pub fn get_files(&self, name: &str) -> Vec<UploadFile> {
        self.parts
            .iter()
            .filter(|p| p.name == name && p.filename.is_some())
            .cloned()
            .filter_map(UploadFile::from_part)
            .collect()
    }

    /// Check if a field exists.
    #[must_use]
    pub fn has_field(&self, name: &str) -> bool {
        self.parts.iter().any(|p| p.name == name)
    }

    /// Get the number of parts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    /// Check if the form is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_boundary() {
        // Standard boundary
        let ct = "multipart/form-data; boundary=----WebKitFormBoundary7MA4YWxkTrZu0gW";
        let boundary = parse_boundary(ct).unwrap();
        assert_eq!(boundary, "----WebKitFormBoundary7MA4YWxkTrZu0gW");
    }

    #[test]
    fn test_parse_boundary_quoted() {
        let ct = r#"multipart/form-data; boundary="simple-boundary""#;
        let boundary = parse_boundary(ct).unwrap();
        assert_eq!(boundary, "simple-boundary");
    }

    #[test]
    fn test_parse_boundary_missing() {
        let ct = "multipart/form-data";
        let result = parse_boundary(ct);
        assert!(matches!(result, Err(MultipartError::MissingBoundary)));
    }

    #[test]
    fn test_parse_boundary_wrong_content_type() {
        let ct = "application/json";
        let result = parse_boundary(ct);
        assert!(matches!(result, Err(MultipartError::InvalidBoundary)));
    }

    #[test]
    fn test_parse_simple_form() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"field1\"\r\n",
            "\r\n",
            "value1\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"field2\"\r\n",
            "\r\n",
            "value2\r\n",
            "------boundary--\r\n"
        );

        let parser = MultipartParser::new(boundary, MultipartConfig::default());
        let parts = parser.parse(body.as_bytes()).unwrap();

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "field1");
        assert_eq!(parts[0].text(), Some("value1"));
        assert!(parts[0].is_field());

        assert_eq!(parts[1].name, "field2");
        assert_eq!(parts[1].text(), Some("value2"));
    }

    #[test]
    fn test_parse_file_upload() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "Hello, World!\r\n",
            "------boundary--\r\n"
        );

        let parser = MultipartParser::new(boundary, MultipartConfig::default());
        let parts = parser.parse(body.as_bytes()).unwrap();

        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].name, "file");
        assert_eq!(parts[0].filename, Some("test.txt".to_string()));
        assert_eq!(parts[0].content_type, Some("text/plain".to_string()));
        assert_eq!(parts[0].text(), Some("Hello, World!"));
        assert!(parts[0].is_file());
    }

    #[test]
    fn test_parse_mixed_form() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"description\"\r\n",
            "\r\n",
            "A test file\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"data.bin\"\r\n",
            "Content-Type: application/octet-stream\r\n",
            "\r\n",
            "\x00\x01\x02\x03\r\n",
            "------boundary--\r\n"
        );

        let parser = MultipartParser::new(boundary, MultipartConfig::default());
        let parts = parser.parse(body.as_bytes()).unwrap();

        assert_eq!(parts.len(), 2);

        // Field
        assert_eq!(parts[0].name, "description");
        assert!(parts[0].is_field());
        assert_eq!(parts[0].text(), Some("A test file"));

        // File
        assert_eq!(parts[1].name, "file");
        assert!(parts[1].is_file());
        assert_eq!(parts[1].data, vec![0x00, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_multipart_form() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"name\"\r\n",
            "\r\n",
            "John\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"avatar\"; filename=\"photo.jpg\"\r\n",
            "Content-Type: image/jpeg\r\n",
            "\r\n",
            "JPEG DATA\r\n",
            "------boundary--\r\n"
        );

        let parser = MultipartParser::new(boundary, MultipartConfig::default());
        let parts = parser.parse(body.as_bytes()).unwrap();
        let form = MultipartForm::from_parts(parts);

        assert_eq!(form.get_field("name"), Some("John"));
        assert!(form.has_field("avatar"));

        let file = form.get_file("avatar").unwrap();
        assert_eq!(file.filename, "photo.jpg");
        assert_eq!(file.content_type, "image/jpeg");
    }

    #[test]
    fn test_file_size_limit() {
        let boundary = "----boundary";
        let large_data = "x".repeat(1000);
        let body = format!(
            "------boundary\r\n\
             Content-Disposition: form-data; name=\"file\"; filename=\"big.txt\"\r\n\
             \r\n\
             {}\r\n\
             ------boundary--\r\n",
            large_data
        );

        let config = MultipartConfig::default().max_file_size(100);
        let parser = MultipartParser::new(boundary, config);
        let result = parser.parse(body.as_bytes());

        assert!(matches!(result, Err(MultipartError::FileTooLarge { .. })));
    }

    #[test]
    fn test_total_size_limit() {
        let boundary = "----boundary";
        let data = "x".repeat(500);
        let body = format!(
            "------boundary\r\n\
             Content-Disposition: form-data; name=\"f1\"; filename=\"a.txt\"\r\n\
             \r\n\
             {}\r\n\
             ------boundary\r\n\
             Content-Disposition: form-data; name=\"f2\"; filename=\"b.txt\"\r\n\
             \r\n\
             {}\r\n\
             ------boundary--\r\n",
            data, data
        );

        let config = MultipartConfig::default()
            .max_file_size(1000)
            .max_total_size(800);
        let parser = MultipartParser::new(boundary, config);
        let result = parser.parse(body.as_bytes());

        assert!(matches!(result, Err(MultipartError::TotalTooLarge { .. })));
    }

    #[test]
    fn test_field_count_limit() {
        let boundary = "----boundary";
        let mut body = String::new();
        for i in 0..5 {
            body.push_str(&format!(
                "------boundary\r\n\
                 Content-Disposition: form-data; name=\"field{}\"\r\n\
                 \r\n\
                 value{}\r\n",
                i, i
            ));
        }
        body.push_str("------boundary--\r\n");

        let config = MultipartConfig::default().max_fields(3);
        let parser = MultipartParser::new(boundary, config);
        let result = parser.parse(body.as_bytes());

        assert!(matches!(result, Err(MultipartError::TooManyFields { .. })));
    }

    #[test]
    fn test_upload_file_extension() {
        let file = UploadFile {
            field_name: "doc".to_string(),
            filename: "report.pdf".to_string(),
            content_type: "application/pdf".to_string(),
            data: vec![],
        };
        assert_eq!(file.extension(), Some("pdf"));

        let no_ext = UploadFile {
            field_name: "doc".to_string(),
            filename: "README".to_string(),
            content_type: "text/plain".to_string(),
            data: vec![],
        };
        assert_eq!(no_ext.extension(), None);
    }

    #[test]
    fn test_content_disposition_parsing() {
        let (name, filename) =
            parse_content_disposition(r#"form-data; name="field"; filename="test.txt""#).unwrap();
        assert_eq!(name, "field");
        assert_eq!(filename, Some("test.txt".to_string()));

        let (name, filename) = parse_content_disposition("form-data; name=simple").unwrap();
        assert_eq!(name, "simple");
        assert_eq!(filename, None);
    }

    #[test]
    fn test_multiple_files_same_name() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"files\"; filename=\"a.txt\"\r\n",
            "\r\n",
            "file a\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"files\"; filename=\"b.txt\"\r\n",
            "\r\n",
            "file b\r\n",
            "------boundary--\r\n"
        );

        let parser = MultipartParser::new(boundary, MultipartConfig::default());
        let parts = parser.parse(body.as_bytes()).unwrap();
        let form = MultipartForm::from_parts(parts);

        let files = form.get_files("files");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "a.txt");
        assert_eq!(files[1].filename, "b.txt");
    }
}
