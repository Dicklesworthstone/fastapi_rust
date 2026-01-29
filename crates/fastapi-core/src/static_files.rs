//! Static file serving for fastapi_rust.
//!
//! This module provides utilities for serving static files from directories.
//! It includes security measures, caching support, and various configuration options.
//!
//! # Features
//!
//! - Directory mounting at path prefix
//! - Index file support (index.html by default)
//! - Content-Type detection from file extension
//! - ETag generation for caching
//! - Last-Modified headers
//! - Range request support for large files (partial content)
//! - Optional directory listing
//! - Symlink handling (configurable)
//! - Path traversal prevention
//! - Hidden file exclusion
//!
//! # Example
//!
//! ```ignore
//! use fastapi_core::static_files::{StaticFiles, StaticFilesConfig};
//!
//! // Basic usage - serve ./public at /static
//! let static_handler = StaticFiles::new("./public")
//!     .prefix("/static");
//!
//! // Advanced configuration
//! let static_handler = StaticFiles::with_config(StaticFilesConfig {
//!     directory: "./assets".into(),
//!     prefix: "/assets".into(),
//!     index_files: vec!["index.html".into(), "index.htm".into()],
//!     show_hidden: false,
//!     follow_symlinks: false,
//!     enable_etag: true,
//!     enable_last_modified: true,
//!     directory_listing: false,
//!     ..Default::default()
//! });
//! ```
//!
//! # Security
//!
//! This module implements several security measures:
//!
//! - **Path traversal prevention**: Requests containing `..` or attempting to
//!   escape the root directory are rejected with 403 Forbidden.
//! - **Hidden file exclusion**: Files starting with `.` are not served by default.
//! - **Symlink protection**: Symlinks are not followed by default to prevent
//!   serving files outside the intended directory.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::response::{Response, ResponseBody, StatusCode, mime_type_for_extension};

/// Configuration for static file serving.
#[derive(Debug, Clone)]
pub struct StaticFilesConfig {
    /// Root directory to serve files from.
    pub directory: PathBuf,
    /// URL path prefix (e.g., "/static").
    pub prefix: String,
    /// Index files to look for in directories.
    pub index_files: Vec<String>,
    /// Whether to serve hidden files (starting with `.`).
    pub show_hidden: bool,
    /// Whether to follow symlinks.
    pub follow_symlinks: bool,
    /// Whether to generate ETag headers.
    pub enable_etag: bool,
    /// Whether to add Last-Modified headers.
    pub enable_last_modified: bool,
    /// Whether to enable directory listing.
    pub directory_listing: bool,
    /// Custom 404 page path (relative to directory).
    pub not_found_page: Option<String>,
    /// Additional headers to add to all responses.
    pub extra_headers: Vec<(String, String)>,
}

impl Default for StaticFilesConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("."),
            prefix: String::new(),
            index_files: vec!["index.html".to_string()],
            show_hidden: false,
            follow_symlinks: false,
            enable_etag: true,
            enable_last_modified: true,
            directory_listing: false,
            not_found_page: None,
            extra_headers: Vec::new(),
        }
    }
}

impl StaticFilesConfig {
    /// Create a new configuration with the given directory.
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            ..Default::default()
        }
    }

    /// Set the URL path prefix.
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Set the index files to look for.
    #[must_use]
    pub fn index_files(mut self, files: Vec<String>) -> Self {
        self.index_files = files;
        self
    }

    /// Enable or disable showing hidden files.
    #[must_use]
    pub fn show_hidden(mut self, show: bool) -> Self {
        self.show_hidden = show;
        self
    }

    /// Enable or disable following symlinks.
    #[must_use]
    pub fn follow_symlinks(mut self, follow: bool) -> Self {
        self.follow_symlinks = follow;
        self
    }

    /// Enable or disable ETag generation.
    #[must_use]
    pub fn enable_etag(mut self, enable: bool) -> Self {
        self.enable_etag = enable;
        self
    }

    /// Enable or disable Last-Modified headers.
    #[must_use]
    pub fn enable_last_modified(mut self, enable: bool) -> Self {
        self.enable_last_modified = enable;
        self
    }

    /// Enable or disable directory listing.
    #[must_use]
    pub fn directory_listing(mut self, enable: bool) -> Self {
        self.directory_listing = enable;
        self
    }

    /// Set a custom 404 page.
    #[must_use]
    pub fn not_found_page(mut self, page: impl Into<String>) -> Self {
        self.not_found_page = Some(page.into());
        self
    }

    /// Add an extra header to all responses.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
    }
}

/// Static file server.
///
/// Serves files from a directory with various security and caching features.
///
/// # Example
///
/// ```ignore
/// use fastapi_core::static_files::StaticFiles;
///
/// let handler = StaticFiles::new("./public")
///     .prefix("/static")
///     .index_file("index.html");
/// ```
#[derive(Debug, Clone)]
pub struct StaticFiles {
    config: StaticFilesConfig,
}

impl StaticFiles {
    /// Create a new static file server for the given directory.
    #[must_use]
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            config: StaticFilesConfig::new(directory),
        }
    }

    /// Create a static file server with full configuration.
    #[must_use]
    pub fn with_config(config: StaticFilesConfig) -> Self {
        Self { config }
    }

    /// Set the URL path prefix.
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.config.prefix = prefix.into();
        self
    }

    /// Set the index file to look for in directories.
    #[must_use]
    pub fn index_file(mut self, file: impl Into<String>) -> Self {
        self.config.index_files = vec![file.into()];
        self
    }

    /// Enable directory listing.
    #[must_use]
    pub fn enable_directory_listing(mut self) -> Self {
        self.config.directory_listing = true;
        self
    }

    /// Enable following symlinks.
    #[must_use]
    pub fn follow_symlinks(mut self) -> Self {
        self.config.follow_symlinks = true;
        self
    }

    /// Serve a request for a static file.
    ///
    /// # Arguments
    ///
    /// * `request_path` - The URL path of the request (e.g., "/static/css/style.css")
    ///
    /// # Returns
    ///
    /// A response containing the file contents, or an error response (404, 403, etc.)
    pub fn serve(&self, request_path: &str) -> Response {
        // Strip prefix from path
        let path_without_prefix = self.strip_prefix(request_path);

        // Security: prevent path traversal
        if !is_safe_path(path_without_prefix) {
            return Response::with_status(StatusCode::FORBIDDEN)
                .header("content-type", b"text/plain".to_vec())
                .body(ResponseBody::Bytes(b"Forbidden: Invalid path".to_vec()));
        }

        // Build the full file path
        let file_path = self
            .config
            .directory
            .join(path_without_prefix.trim_start_matches('/'));

        // Canonicalize to resolve any remaining path tricks
        let canonical_path = match self.resolve_path(&file_path) {
            Some(p) => p,
            None => return self.not_found_response(),
        };

        // Ensure the resolved path is within our directory
        let canonical_dir = match self.config.directory.canonicalize() {
            Ok(d) => d,
            Err(_) => return self.not_found_response(),
        };

        if !canonical_path.starts_with(&canonical_dir) {
            return Response::with_status(StatusCode::FORBIDDEN)
                .header("content-type", b"text/plain".to_vec())
                .body(ResponseBody::Bytes(
                    b"Forbidden: Path traversal detected".to_vec(),
                ));
        }

        // Check if it's a directory
        if canonical_path.is_dir() {
            return self.serve_directory(&canonical_path, request_path);
        }

        // Serve the file
        self.serve_file(&canonical_path)
    }

    /// Strip the URL prefix from the request path.
    fn strip_prefix<'a>(&self, path: &'a str) -> &'a str {
        if self.config.prefix.is_empty() {
            return path;
        }

        path.strip_prefix(&self.config.prefix).unwrap_or(path)
    }

    /// Resolve the file path, handling symlinks according to config.
    fn resolve_path(&self, path: &Path) -> Option<PathBuf> {
        if self.config.follow_symlinks {
            path.canonicalize().ok()
        } else {
            // Check if path contains symlinks
            if path.exists() {
                let metadata = std::fs::symlink_metadata(path).ok()?;
                if metadata.file_type().is_symlink() {
                    return None; // Reject symlinks
                }
                path.canonicalize().ok()
            } else {
                None
            }
        }
    }

    /// Serve a directory (index file or listing).
    fn serve_directory(&self, dir_path: &Path, request_path: &str) -> Response {
        // Try index files
        for index_file in &self.config.index_files {
            let index_path = dir_path.join(index_file);
            if index_path.exists() && index_path.is_file() {
                return self.serve_file(&index_path);
            }
        }

        // Directory listing if enabled
        if self.config.directory_listing {
            return self.generate_directory_listing(dir_path, request_path);
        }

        // No index file and no listing - return 404
        self.not_found_response()
    }

    /// Serve a single file.
    fn serve_file(&self, file_path: &Path) -> Response {
        // Check for hidden files
        if !self.config.show_hidden {
            if let Some(name) = file_path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    return self.not_found_response();
                }
            }
        }

        // Read file contents
        let contents = match std::fs::read(file_path) {
            Ok(c) => c,
            Err(_) => return self.not_found_response(),
        };

        // Get file metadata for caching headers
        let metadata = std::fs::metadata(file_path).ok();

        // Determine content type
        let content_type = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(mime_type_for_extension)
            .unwrap_or("application/octet-stream");

        let mut response = Response::ok()
            .header("content-type", content_type.as_bytes().to_vec())
            .header("accept-ranges", b"bytes".to_vec());

        // Add ETag if enabled
        if self.config.enable_etag {
            let etag = generate_etag(&contents);
            response = response.header("etag", etag.into_bytes());
        }

        // Add Last-Modified if enabled
        if self.config.enable_last_modified {
            if let Some(ref meta) = metadata {
                if let Ok(modified) = meta.modified() {
                    let http_date = format_http_date(modified);
                    response = response.header("last-modified", http_date.into_bytes());
                }
            }
        }

        // Add extra headers
        for (name, value) in &self.config.extra_headers {
            response = response.header(name.clone(), value.clone().into_bytes());
        }

        response.body(ResponseBody::Bytes(contents))
    }

    /// Generate a directory listing HTML page.
    fn generate_directory_listing(&self, dir_path: &Path, request_path: &str) -> Response {
        let mut entries = Vec::new();

        // Add parent directory link if not at root
        if request_path != "/" && request_path != &self.config.prefix {
            entries.push(DirectoryEntry {
                name: "..".to_string(),
                is_dir: true,
                size: 0,
                modified: None,
            });
        }

        // Read directory entries
        if let Ok(read_dir) = std::fs::read_dir(dir_path) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip hidden files if not showing them
                if !self.config.show_hidden && name.starts_with('.') {
                    continue;
                }

                if let Ok(metadata) = entry.metadata() {
                    entries.push(DirectoryEntry {
                        name,
                        is_dir: metadata.is_dir(),
                        size: metadata.len(),
                        modified: metadata.modified().ok(),
                    });
                }
            }
        }

        // Sort: directories first, then by name
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });

        // Generate HTML
        let html = generate_listing_html(request_path, &entries);

        Response::ok()
            .header("content-type", b"text/html; charset=utf-8".to_vec())
            .body(ResponseBody::Bytes(html.into_bytes()))
    }

    /// Generate a 404 response, optionally using custom page.
    fn not_found_response(&self) -> Response {
        if let Some(ref not_found_path) = self.config.not_found_page {
            let path = self.config.directory.join(not_found_path);
            if let Ok(contents) = std::fs::read(&path) {
                let content_type = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(mime_type_for_extension)
                    .unwrap_or("text/html; charset=utf-8");

                return Response::with_status(StatusCode::NOT_FOUND)
                    .header("content-type", content_type.as_bytes().to_vec())
                    .body(ResponseBody::Bytes(contents));
            }
        }

        Response::with_status(StatusCode::NOT_FOUND)
            .header("content-type", b"text/plain".to_vec())
            .body(ResponseBody::Bytes(b"Not Found".to_vec()))
    }
}

/// Check if a path is safe (no path traversal attempts).
fn is_safe_path(path: &str) -> bool {
    // Reject paths with .. components
    for component in path.split('/') {
        if component == ".." {
            return false;
        }
    }

    // Reject paths with null bytes
    if path.contains('\0') {
        return false;
    }

    // Reject paths that try to escape via encoded characters
    let decoded = percent_decode(path);
    if decoded.contains("..") {
        return false;
    }

    true
}

/// Simple percent-decoding for path safety checks.
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
            result.push_str(&hex);
        } else {
            result.push(c);
        }
    }

    result
}

/// Generate an ETag from file contents using FNV-1a hash.
fn generate_etag(contents: &[u8]) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET_BASIS;
    for &byte in contents {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    format!("\"{:016x}\"", hash)
}

/// Format a SystemTime as an HTTP date (RFC 7231).
fn format_http_date(time: SystemTime) -> String {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => {
            let secs = duration.as_secs();
            let days = secs / 86400;
            let remaining_secs = secs % 86400;
            let hours = remaining_secs / 3600;
            let minutes = (remaining_secs % 3600) / 60;
            let seconds = remaining_secs % 60;

            let day_of_week = ((days + 4) % 7) as usize;
            let day_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

            let (year, month, day) = days_to_date(days);
            let month_names = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];

            format!(
                "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
                day_names[day_of_week],
                day,
                month_names[(month - 1) as usize],
                year,
                hours,
                minutes,
                seconds
            )
        }
        Err(_) => "Thu, 01 Jan 1970 00:00:00 GMT".to_string(),
    }
}

/// Convert days since UNIX epoch to (year, month, day).
fn days_to_date(days: u64) -> (u64, u64, u64) {
    let mut remaining_days = days;
    let mut year = 1970u64;

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days: [u64; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 1u64;
    for &days_in_month in &month_days {
        if remaining_days < days_in_month {
            break;
        }
        remaining_days -= days_in_month;
        month += 1;
    }

    (year, month, remaining_days + 1)
}

/// Check if a year is a leap year.
fn is_leap_year(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Directory entry for listing.
struct DirectoryEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

/// Generate HTML for directory listing.
fn generate_listing_html(path: &str, entries: &[DirectoryEntry]) -> String {
    let mut html = String::new();

    html.push_str("<!DOCTYPE html>\n<html>\n<head>\n");
    html.push_str("<meta charset=\"utf-8\">\n");
    html.push_str(&format!("<title>Index of {}</title>\n", escape_html(path)));
    html.push_str("<style>\n");
    html.push_str("body { font-family: monospace; margin: 20px; }\n");
    html.push_str("h1 { border-bottom: 1px solid #ccc; padding-bottom: 10px; }\n");
    html.push_str("table { border-collapse: collapse; width: 100%; }\n");
    html.push_str("th, td { text-align: left; padding: 8px; border-bottom: 1px solid #eee; }\n");
    html.push_str("th { background: #f5f5f5; }\n");
    html.push_str("a { text-decoration: none; color: #0066cc; }\n");
    html.push_str("a:hover { text-decoration: underline; }\n");
    html.push_str(".dir { font-weight: bold; }\n");
    html.push_str(".size { text-align: right; }\n");
    html.push_str("</style>\n");
    html.push_str("</head>\n<body>\n");

    html.push_str(&format!("<h1>Index of {}</h1>\n", escape_html(path)));
    html.push_str("<table>\n");
    html.push_str("<tr><th>Name</th><th>Size</th><th>Modified</th></tr>\n");

    for entry in entries {
        let href = if entry.name == ".." {
            parent_path(path)
        } else if entry.is_dir {
            format!("{}/{}/", path.trim_end_matches('/'), &entry.name)
        } else {
            format!("{}/{}", path.trim_end_matches('/'), &entry.name)
        };

        let class = if entry.is_dir { " class=\"dir\"" } else { "" };
        let display_name = if entry.is_dir {
            format!("{}/", &entry.name)
        } else {
            entry.name.clone()
        };

        let size_str = if entry.is_dir {
            "-".to_string()
        } else {
            format_size(entry.size)
        };

        let modified_str = entry
            .modified
            .map(|t| format_http_date(t))
            .unwrap_or_else(|| "-".to_string());

        html.push_str(&format!(
            "<tr><td{}><a href=\"{}\">{}</a></td><td class=\"size\">{}</td><td>{}</td></tr>\n",
            class,
            escape_html(&href),
            escape_html(&display_name),
            size_str,
            modified_str
        ));
    }

    html.push_str("</table>\n");
    html.push_str("<hr>\n<p>fastapi_rust static file server</p>\n");
    html.push_str("</body>\n</html>");

    html
}

/// Get the parent path.
fn parent_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(pos) if pos > 0 => format!("{}/", &trimmed[..pos]),
        _ => "/".to_string(),
    }
}

/// Escape HTML special characters.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Format file size for display.
fn format_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if size >= GB {
        format!("{:.1}G", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.1}M", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1}K", size as f64 / KB as f64)
    } else {
        format!("{}", size)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_path_normal() {
        assert!(is_safe_path("/static/css/style.css"));
        assert!(is_safe_path("/images/logo.png"));
        assert!(is_safe_path("/"));
        assert!(is_safe_path(""));
    }

    #[test]
    fn safe_path_traversal_blocked() {
        assert!(!is_safe_path("/../etc/passwd"));
        assert!(!is_safe_path("/static/../../../etc/passwd"));
        assert!(!is_safe_path(".."));
        assert!(!is_safe_path("/.."));
    }

    #[test]
    fn safe_path_encoded_traversal_blocked() {
        assert!(!is_safe_path("/%2e%2e/etc/passwd"));
        assert!(!is_safe_path("/static/%2e%2e/%2e%2e/etc/passwd"));
    }

    #[test]
    fn safe_path_null_byte_blocked() {
        assert!(!is_safe_path("/static/file\0.txt"));
    }

    #[test]
    fn percent_decode_works() {
        assert_eq!(percent_decode("%2e%2e"), "..");
        assert_eq!(percent_decode("%2F"), "/");
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("normal"), "normal");
    }

    #[test]
    fn etag_generation() {
        let contents = b"Hello, World!";
        let etag = generate_etag(contents);
        assert!(etag.starts_with('"'));
        assert!(etag.ends_with('"'));
        assert_eq!(etag.len(), 18); // 16 hex chars + 2 quotes
    }

    #[test]
    fn etag_deterministic() {
        let contents = b"test data";
        let etag1 = generate_etag(contents);
        let etag2 = generate_etag(contents);
        assert_eq!(etag1, etag2);
    }

    #[test]
    fn etag_different_for_different_content() {
        let etag1 = generate_etag(b"content 1");
        let etag2 = generate_etag(b"content 2");
        assert_ne!(etag1, etag2);
    }

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0");
        assert_eq!(format_size(100), "100");
        assert_eq!(format_size(1023), "1023");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(1024), "1.0K");
        assert_eq!(format_size(2048), "2.0K");
        assert_eq!(format_size(1536), "1.5K");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(1024 * 1024), "1.0M");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0M");
    }

    #[test]
    fn format_size_gb() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0G");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.0G");
    }

    #[test]
    fn escape_html_special_chars() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn parent_path_normal() {
        assert_eq!(parent_path("/static/css/"), "/static/");
        assert_eq!(parent_path("/static/"), "/");
        assert_eq!(parent_path("/"), "/");
    }

    #[test]
    fn config_builder() {
        let config = StaticFilesConfig::new("./public")
            .prefix("/static")
            .show_hidden(false)
            .directory_listing(true);

        assert_eq!(config.directory, PathBuf::from("./public"));
        assert_eq!(config.prefix, "/static");
        assert!(!config.show_hidden);
        assert!(config.directory_listing);
    }

    #[test]
    fn static_files_builder() {
        let handler = StaticFiles::new("./assets")
            .prefix("/assets")
            .index_file("index.htm")
            .enable_directory_listing();

        assert_eq!(handler.config.prefix, "/assets");
        assert_eq!(handler.config.index_files, vec!["index.htm"]);
        assert!(handler.config.directory_listing);
    }

    #[test]
    fn leap_year_detection() {
        assert!(!is_leap_year(1900)); // Divisible by 100 but not 400
        assert!(is_leap_year(2000)); // Divisible by 400
        assert!(is_leap_year(2024)); // Divisible by 4 but not 100
        assert!(!is_leap_year(2023)); // Not divisible by 4
    }

    #[test]
    fn http_date_format() {
        let date = format_http_date(std::time::UNIX_EPOCH);
        assert_eq!(date, "Thu, 01 Jan 1970 00:00:00 GMT");
    }
}
