//! Multipart form data parser.
//!
//! Provides parsing of `multipart/form-data` request bodies, commonly used for file uploads.
//! The parser enforces per-file and total size limits.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default maximum file size (10MB).
pub const DEFAULT_MAX_FILE_SIZE: usize = 10 * 1024 * 1024;

/// Default maximum total upload size (50MB).
pub const DEFAULT_MAX_TOTAL_SIZE: usize = 50 * 1024 * 1024;

/// Default maximum number of fields.
pub const DEFAULT_MAX_FIELDS: usize = 100;

/// Default threshold for spooling uploads to a temporary file (1MB).
pub const DEFAULT_SPOOL_THRESHOLD: usize = 1024 * 1024;
/// RFC 2046 recommends multipart boundary length <= 70 characters.
const MAX_BOUNDARY_LEN: usize = 70;

/// Configuration for multipart parsing.
#[derive(Debug, Clone)]
pub struct MultipartConfig {
    /// Maximum size per file in bytes.
    max_file_size: usize,
    /// Maximum total upload size in bytes.
    max_total_size: usize,
    /// Maximum number of fields (including files).
    max_fields: usize,
    /// Threshold above which uploaded files are spooled to a temporary file.
    spool_threshold: usize,
}

impl Default for MultipartConfig {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_total_size: DEFAULT_MAX_TOTAL_SIZE,
            max_fields: DEFAULT_MAX_FIELDS,
            spool_threshold: DEFAULT_SPOOL_THRESHOLD,
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

    /// Set the threshold above which files are spooled to a temporary file.
    #[must_use]
    pub fn spool_threshold(mut self, size: usize) -> Self {
        self.spool_threshold = size;
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

    /// Get the spool-to-disk threshold.
    #[must_use]
    pub fn get_spool_threshold(&self) -> usize {
        self.spool_threshold
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
    /// I/O error while spooling streamed part data.
    Io { detail: String },
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
            Self::InvalidPartHeaders { detail } => write!(f, "invalid part headers: {detail}"),
            Self::UnexpectedEof => write!(f, "unexpected end of multipart data"),
            Self::InvalidFormat { detail } => write!(f, "invalid multipart format: {detail}"),
            Self::Io { detail } => write!(f, "multipart I/O error: {detail}"),
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
    spooled_path: Option<PathBuf>,
    spooled_len: Option<usize>,
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
        self.spooled_len.unwrap_or(self.data.len())
    }

    /// Returns true when this part's data is backed by a spooled temp file.
    #[must_use]
    pub fn is_spooled(&self) -> bool {
        self.spooled_path.is_some()
    }

    /// Path to the spooled temporary file, if this part is backed by disk.
    #[must_use]
    pub fn spooled_path(&self) -> Option<&Path> {
        self.spooled_path.as_deref()
    }

    /// Read part bytes regardless of in-memory or spooled backing.
    pub fn bytes(&self) -> std::io::Result<Vec<u8>> {
        if let Some(path) = &self.spooled_path {
            std::fs::read(path)
        } else {
            Ok(self.data.clone())
        }
    }
}

#[derive(Debug)]
enum UploadStorage {
    InMemory(Vec<u8>),
    SpooledTempFile { path: PathBuf, len: u64 },
}

/// An uploaded file with metadata and FastAPI-style async file operations.
#[derive(Debug)]
pub struct UploadFile {
    /// The field name.
    pub field_name: String,
    /// The original filename.
    pub filename: String,
    /// Content-Type of the file.
    pub content_type: String,
    storage: UploadStorage,
    cursor: u64,
    closed: bool,
}

impl UploadFile {
    /// Create a new UploadFile from a Part.
    ///
    /// Returns `None` if the part is not a file.
    #[must_use]
    pub fn from_part(part: Part) -> Option<Self> {
        Self::from_part_with_spool_threshold(part, DEFAULT_SPOOL_THRESHOLD)
    }

    /// Create a new UploadFile from a Part with a custom spool threshold.
    ///
    /// Returns `None` if the part is not a file.
    #[must_use]
    pub fn from_part_with_spool_threshold(part: Part, spool_threshold: usize) -> Option<Self> {
        let Part {
            name,
            filename,
            content_type,
            data,
            headers: _,
            spooled_path,
            spooled_len,
        } = part;
        let filename = filename?;

        let storage = if let Some(path) = spooled_path {
            UploadStorage::SpooledTempFile {
                path,
                len: u64::try_from(spooled_len.unwrap_or(data.len())).unwrap_or(u64::MAX),
            }
        } else if data.len() > spool_threshold {
            match spool_to_tempfile(&data) {
                Ok(path) => UploadStorage::SpooledTempFile {
                    path,
                    len: u64::try_from(data.len()).unwrap_or(u64::MAX),
                },
                Err(_) => UploadStorage::InMemory(data),
            }
        } else {
            UploadStorage::InMemory(data)
        };

        Some(Self {
            field_name: name,
            filename,
            content_type: content_type.unwrap_or_else(|| "application/octet-stream".to_string()),
            storage,
            cursor: 0,
            closed: false,
        })
    }

    /// Get the file size in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        match &self.storage {
            UploadStorage::InMemory(data) => data.len(),
            UploadStorage::SpooledTempFile { len, .. } => {
                usize::try_from(*len).unwrap_or(usize::MAX)
            }
        }
    }

    /// Returns true when this file has been spooled to a temporary file.
    #[must_use]
    pub fn is_spooled(&self) -> bool {
        matches!(self.storage, UploadStorage::SpooledTempFile { .. })
    }

    /// Path to the spooled temporary file, if this upload is backed by disk.
    #[must_use]
    pub fn spooled_path(&self) -> Option<&Path> {
        match &self.storage {
            UploadStorage::InMemory(_) => None,
            UploadStorage::SpooledTempFile { path, .. } => Some(path.as_path()),
        }
    }

    /// Read file contents without changing the current cursor.
    pub fn bytes(&self) -> std::io::Result<Vec<u8>> {
        match &self.storage {
            UploadStorage::InMemory(data) => Ok(data.clone()),
            UploadStorage::SpooledTempFile { path, .. } => std::fs::read(path),
        }
    }

    /// Read from the current cursor position.
    ///
    /// - `size = Some(n)`: read up to `n` bytes
    /// - `size = None`: read until EOF
    pub async fn read(&mut self, size: Option<usize>) -> std::io::Result<Vec<u8>> {
        self.ensure_open()?;

        match &mut self.storage {
            UploadStorage::InMemory(data) => {
                let start = usize::try_from(self.cursor).unwrap_or(usize::MAX);
                if start >= data.len() {
                    return Ok(Vec::new());
                }

                let end = match size {
                    Some(n) => start.saturating_add(n).min(data.len()),
                    None => data.len(),
                };
                self.cursor = u64::try_from(end).unwrap_or(u64::MAX);
                Ok(data[start..end].to_vec())
            }
            UploadStorage::SpooledTempFile { path, len } => {
                let mut file = std::fs::File::open(path)?;
                file.seek(SeekFrom::Start(self.cursor))?;

                let max_to_read = match size {
                    Some(n) => u64::try_from(n).unwrap_or(u64::MAX),
                    None => len.saturating_sub(self.cursor),
                };

                let mut reader = file.take(max_to_read);
                let mut out = Vec::new();
                reader.read_to_end(&mut out)?;
                self.cursor = self
                    .cursor
                    .saturating_add(u64::try_from(out.len()).unwrap_or(u64::MAX));
                Ok(out)
            }
        }
    }

    /// Write bytes at the current cursor position.
    ///
    /// Returns the number of bytes written.
    pub async fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.ensure_open()?;
        if bytes.is_empty() {
            return Ok(0);
        }

        match &mut self.storage {
            UploadStorage::InMemory(data) => {
                let start = usize::try_from(self.cursor).unwrap_or(usize::MAX);
                if start > data.len() {
                    data.resize(start, 0);
                }

                let end = start.saturating_add(bytes.len());
                if end > data.len() {
                    data.resize(end, 0);
                }
                data[start..end].copy_from_slice(bytes);
                self.cursor = u64::try_from(end).unwrap_or(u64::MAX);
                Ok(bytes.len())
            }
            UploadStorage::SpooledTempFile { path, len } => {
                let mut file = OpenOptions::new().read(true).write(true).open(path)?;
                file.seek(SeekFrom::Start(self.cursor))?;
                file.write_all(bytes)?;
                self.cursor = self
                    .cursor
                    .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                if self.cursor > *len {
                    *len = self.cursor;
                }
                Ok(bytes.len())
            }
        }
    }

    /// Move the current cursor.
    pub async fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.ensure_open()?;
        let new_cursor = resolve_seek(self.cursor, self.len_u64(), position)?;
        self.cursor = new_cursor;
        Ok(new_cursor)
    }

    /// Close the file handle and clean up any temporary storage.
    pub async fn close(&mut self) -> std::io::Result<()> {
        if self.closed {
            return Ok(());
        }

        if let UploadStorage::SpooledTempFile { path, .. } = &self.storage {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        self.closed = true;
        Ok(())
    }

    /// Get the file extension from the filename.
    #[must_use]
    pub fn extension(&self) -> Option<&str> {
        self.filename
            .rsplit('.')
            .next()
            .filter(|ext| !ext.is_empty() && *ext != self.filename)
    }

    fn ensure_open(&self) -> std::io::Result<()> {
        if self.closed {
            Err(std::io::Error::other("upload file is closed"))
        } else {
            Ok(())
        }
    }

    fn len_u64(&self) -> u64 {
        match &self.storage {
            UploadStorage::InMemory(data) => u64::try_from(data.len()).unwrap_or(u64::MAX),
            UploadStorage::SpooledTempFile { len, .. } => *len,
        }
    }
}

impl Drop for UploadFile {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        if let UploadStorage::SpooledTempFile { path, .. } = &self.storage {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn resolve_seek(current: u64, len: u64, position: SeekFrom) -> std::io::Result<u64> {
    let next = match position {
        SeekFrom::Start(offset) => i128::from(offset),
        SeekFrom::End(offset) => i128::from(len) + i128::from(offset),
        SeekFrom::Current(offset) => i128::from(current) + i128::from(offset),
    };

    if next < 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "seek before start of file",
        ));
    }

    u64::try_from(next).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "seek target exceeds addressable range",
        )
    })
}

static UPLOAD_SPOOL_COUNTER: AtomicU64 = AtomicU64::new(1);

fn create_spool_tempfile() -> std::io::Result<(PathBuf, std::fs::File)> {
    let temp_dir = std::env::temp_dir();
    let ts_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    for _ in 0..32 {
        let counter = UPLOAD_SPOOL_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = temp_dir.join(format!(
            "fastapi-rust-upload-{}-{ts_nanos}-{counter}.tmp",
            std::process::id()
        ));

        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "failed to allocate unique spool file",
    ))
}

fn spool_to_tempfile(data: &[u8]) -> std::io::Result<PathBuf> {
    let (path, mut file) = create_spool_tempfile()?;
    file.write_all(data)?;
    Ok(path)
}

/// Parse boundary from Content-Type header.
///
/// Content-Type format: `multipart/form-data; boundary=----WebKitFormBoundary...`
pub fn parse_boundary(content_type: &str) -> Result<String, MultipartError> {
    let content_type = content_type.trim();
    let main = content_type.split(';').next().unwrap_or("").trim();
    if !main.eq_ignore_ascii_case("multipart/form-data") {
        return Err(MultipartError::InvalidBoundary);
    }

    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        if k.trim().eq_ignore_ascii_case("boundary") {
            let boundary = v.trim();
            let boundary = boundary.trim_matches('"').trim_matches('\'');
            if boundary.is_empty() || boundary.len() > MAX_BOUNDARY_LEN {
                return Err(MultipartError::InvalidBoundary);
            }
            return Ok(boundary.to_string());
        }
    }

    Err(MultipartError::MissingBoundary)
}

/// Multipart parser (boundary-based).
#[derive(Debug)]
pub struct MultipartParser {
    boundary: Vec<u8>,
    config: MultipartConfig,
}

/// Incremental parser state for streamed multipart bodies.
#[derive(Debug, Default)]
pub struct MultipartStreamState {
    started: bool,
    done: bool,
    part_count: usize,
    total_size: usize,
    current_part: Option<StreamingPartState>,
}

#[derive(Debug, Clone)]
enum PartStreamingStorage {
    InMemory(Vec<u8>),
    SpooledTempFile { path: PathBuf, len: usize },
}

#[derive(Debug, Clone)]
struct StreamingPartState {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    headers: HashMap<String, String>,
    size: usize,
    storage: PartStreamingStorage,
}

impl StreamingPartState {
    fn new(
        name: String,
        filename: Option<String>,
        content_type: Option<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            name,
            filename,
            content_type,
            headers,
            size: 0,
            storage: PartStreamingStorage::InMemory(Vec::new()),
        }
    }

    fn append(
        &mut self,
        chunk: &[u8],
        config: &MultipartConfig,
        total_size: &mut usize,
    ) -> Result<(), MultipartError> {
        if chunk.is_empty() {
            return Ok(());
        }

        let next_size = self.size.saturating_add(chunk.len());
        if self.filename.is_some() && next_size > config.max_file_size {
            return Err(MultipartError::FileTooLarge {
                size: next_size,
                max: config.max_file_size,
            });
        }

        let next_total = total_size.saturating_add(chunk.len());
        if next_total > config.max_total_size {
            return Err(MultipartError::TotalTooLarge {
                size: next_total,
                max: config.max_total_size,
            });
        }

        match &mut self.storage {
            PartStreamingStorage::InMemory(data) => {
                if self.filename.is_some() && next_size > config.spool_threshold {
                    let (path, mut file) =
                        create_spool_tempfile().map_err(|e| MultipartError::Io {
                            detail: format!("failed to create spool tempfile: {e}"),
                        })?;
                    file.write_all(data).map_err(|e| MultipartError::Io {
                        detail: format!("failed to write spool tempfile: {e}"),
                    })?;
                    file.write_all(chunk).map_err(|e| MultipartError::Io {
                        detail: format!("failed to write spool tempfile: {e}"),
                    })?;
                    self.storage = PartStreamingStorage::SpooledTempFile {
                        path,
                        len: next_size,
                    };
                } else {
                    data.extend_from_slice(chunk);
                }
            }
            PartStreamingStorage::SpooledTempFile { path, len } => {
                let mut file =
                    OpenOptions::new()
                        .append(true)
                        .open(path)
                        .map_err(|e| MultipartError::Io {
                            detail: format!("failed to open spool tempfile for append: {e}"),
                        })?;
                file.write_all(chunk).map_err(|e| MultipartError::Io {
                    detail: format!("failed to append spool tempfile: {e}"),
                })?;
                *len = next_size;
            }
        }

        self.size = next_size;
        *total_size = next_total;
        Ok(())
    }

    fn into_part(mut self) -> Part {
        let storage = std::mem::replace(
            &mut self.storage,
            PartStreamingStorage::InMemory(Vec::new()),
        );
        let (data, spooled_path, spooled_len) = match storage {
            PartStreamingStorage::InMemory(data) => {
                let len = data.len();
                (data, None, Some(len))
            }
            PartStreamingStorage::SpooledTempFile { path, len } => {
                (Vec::new(), Some(path), Some(len))
            }
        };

        Part {
            name: std::mem::take(&mut self.name),
            filename: std::mem::take(&mut self.filename),
            content_type: std::mem::take(&mut self.content_type),
            data,
            headers: std::mem::take(&mut self.headers),
            spooled_path,
            spooled_len,
        }
    }
}

impl Drop for StreamingPartState {
    fn drop(&mut self) {
        if let PartStreamingStorage::SpooledTempFile { path, .. } = &self.storage {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl MultipartStreamState {
    /// Returns true if the closing boundary has been fully parsed.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done
    }
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
    pub fn parse(&self, body: &[u8]) -> Result<Vec<Part>, MultipartError> {
        let mut parts = Vec::new();
        let mut total_size = 0usize;
        let mut pos = 0;

        // Skip preamble and find first boundary
        pos = self.find_boundary_from(body, pos)?;

        loop {
            if parts.len() >= self.config.max_fields {
                return Err(MultipartError::TooManyFields {
                    count: parts.len() + 1,
                    max: self.config.max_fields,
                });
            }

            let boundary_end = pos + self.boundary.len();
            if boundary_end + 2 <= body.len() && body[boundary_end..boundary_end + 2] == *b"--" {
                break;
            }

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

            let (headers, header_end) = self.parse_part_headers(body, pos)?;
            pos = header_end;

            let content_disp = headers
                .get("content-disposition")
                .ok_or(MultipartError::MissingContentDisposition)?;
            let (name, filename) = parse_content_disposition(content_disp)?;
            let content_type = headers.get("content-type").cloned();

            let data_end = self.find_boundary_from(body, pos)?;
            let data = if data_end >= 2 && body[data_end - 2..data_end] == *b"\r\n" {
                &body[pos..data_end - 2]
            } else {
                &body[pos..data_end]
            };

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

            parts.push(Part {
                name,
                filename,
                content_type,
                data: data.to_vec(),
                headers,
                spooled_path: None,
                spooled_len: None,
            });

            pos = data_end;
        }

        Ok(parts)
    }

    /// Parse any newly-available parts from a streamed multipart buffer.
    ///
    /// This method mutates `buffer` by draining bytes that were fully consumed.
    /// It can be called repeatedly as new bytes arrive.
    ///
    /// - Set `eof = false` while more chunks may still arrive.
    /// - Set `eof = true` on the final call to enforce that the stream ended on
    ///   a valid multipart boundary.
    #[allow(clippy::too_many_lines)]
    pub fn parse_incremental(
        &self,
        buffer: &mut Vec<u8>,
        state: &mut MultipartStreamState,
        eof: bool,
    ) -> Result<Vec<Part>, MultipartError> {
        let mut parsed = Vec::new();

        loop {
            if state.done {
                return Ok(parsed);
            }

            if !state.started {
                match self.find_boundary_from(buffer, 0) {
                    Ok(boundary_pos) => {
                        state.started = true;
                        if boundary_pos > 0 {
                            buffer.drain(..boundary_pos);
                        }
                    }
                    Err(MultipartError::UnexpectedEof) => {
                        if eof {
                            return Err(MultipartError::UnexpectedEof);
                        }
                        // Keep only the suffix that could still contain a split boundary.
                        let keep = self.boundary.len().saturating_add(4);
                        if buffer.len() > keep {
                            let drain_to = buffer.len() - keep;
                            buffer.drain(..drain_to);
                        }
                        return Ok(parsed);
                    }
                    Err(err) => return Err(err),
                }
            }

            if state.current_part.is_none() {
                if !buffer.starts_with(&self.boundary) {
                    match self.find_boundary_from(buffer, 0) {
                        Ok(boundary_pos) => {
                            if boundary_pos > 0 {
                                buffer.drain(..boundary_pos);
                            }
                        }
                        Err(MultipartError::UnexpectedEof) => {
                            if eof {
                                return Err(MultipartError::UnexpectedEof);
                            }
                            return Ok(parsed);
                        }
                        Err(err) => return Err(err),
                    }
                }

                let boundary_end = self.boundary.len();
                if boundary_end + 2 > buffer.len() {
                    if eof {
                        return Err(MultipartError::UnexpectedEof);
                    }
                    return Ok(parsed);
                }

                let boundary_suffix = &buffer[boundary_end..boundary_end + 2];
                if boundary_suffix == b"--" {
                    state.done = true;

                    // Consume through final boundary marker (+ optional CRLF).
                    let mut consumed = boundary_end + 2;
                    if consumed + 2 <= buffer.len() && buffer[consumed..consumed + 2] == *b"\r\n" {
                        consumed += 2;
                    }
                    buffer.drain(..consumed);
                    return Ok(parsed);
                }

                if boundary_suffix != b"\r\n" {
                    return Err(MultipartError::InvalidFormat {
                        detail: "expected CRLF after boundary",
                    });
                }

                let headers_start = boundary_end + 2;
                let (headers, data_start) = match self.parse_part_headers(buffer, headers_start) {
                    Ok(v) => v,
                    Err(MultipartError::UnexpectedEof) => {
                        if eof {
                            return Err(MultipartError::UnexpectedEof);
                        }
                        return Ok(parsed);
                    }
                    Err(err) => return Err(err),
                };

                let content_disp = headers
                    .get("content-disposition")
                    .ok_or(MultipartError::MissingContentDisposition)?;
                let (name, filename) = parse_content_disposition(content_disp)?;
                let content_type = headers.get("content-type").cloned();

                state.current_part = Some(StreamingPartState::new(
                    name,
                    filename,
                    content_type,
                    headers,
                ));
                buffer.drain(..data_start);
                continue;
            }

            let data_end = match self.find_boundary_in_part_data(buffer, 0) {
                Ok(pos) => Some(pos),
                Err(MultipartError::UnexpectedEof) => None,
                Err(err) => return Err(err),
            };

            if let Some(data_end) = data_end {
                let write_end = if data_end >= 2 && buffer[data_end - 2..data_end] == *b"\r\n" {
                    data_end - 2
                } else {
                    data_end
                };
                if write_end > 0 {
                    let Some(part_state) = state.current_part.as_mut() else {
                        return Err(MultipartError::InvalidFormat {
                            detail: "missing current multipart part state",
                        });
                    };
                    part_state.append(&buffer[..write_end], &self.config, &mut state.total_size)?;
                }

                state.part_count = state.part_count.saturating_add(1);
                if state.part_count > self.config.max_fields {
                    return Err(MultipartError::TooManyFields {
                        count: state.part_count,
                        max: self.config.max_fields,
                    });
                }

                let Some(part_state) = state.current_part.take() else {
                    return Err(MultipartError::InvalidFormat {
                        detail: "missing current multipart part state",
                    });
                };
                parsed.push(part_state.into_part());

                // Keep the next boundary in-buffer for the next iteration.
                buffer.drain(..data_end);
                continue;
            }

            if eof {
                return Err(MultipartError::UnexpectedEof);
            }

            // No complete boundary yet: flush the safe prefix into the current part storage.
            let keep = self.boundary.len().saturating_add(4);
            if buffer.len() > keep {
                let flush_len = buffer.len() - keep;
                let Some(part_state) = state.current_part.as_mut() else {
                    return Err(MultipartError::InvalidFormat {
                        detail: "missing current multipart part state",
                    });
                };
                part_state.append(&buffer[..flush_len], &self.config, &mut state.total_size)?;
                buffer.drain(..flush_len);
            }
            return Ok(parsed);
        }
    }

    fn find_boundary_from(&self, data: &[u8], start: usize) -> Result<usize, MultipartError> {
        let boundary = &self.boundary;
        let boundary_len = boundary.len();
        if data.len() < boundary_len {
            return Err(MultipartError::UnexpectedEof);
        }

        let end = data.len() - boundary_len + 1;
        for i in start..end {
            if !data[i..].starts_with(boundary) {
                continue;
            }

            // Boundaries must occur at the start of the body or at the start of a CRLF-delimited
            // line, and must be followed by either CRLF (next part) or `--` (final boundary).
            if i != 0 && (i < 2 || data[i - 2..i] != *b"\r\n") {
                continue;
            }

            let boundary_end = i + boundary_len;
            if boundary_end + 2 > data.len() {
                return Err(MultipartError::UnexpectedEof);
            }
            let suffix = &data[boundary_end..boundary_end + 2];
            if suffix != b"\r\n" && suffix != b"--" {
                continue;
            }

            return Ok(i);
        }

        Err(MultipartError::UnexpectedEof)
    }

    fn find_boundary_in_part_data(
        &self,
        data: &[u8],
        start: usize,
    ) -> Result<usize, MultipartError> {
        let boundary = &self.boundary;
        let boundary_len = boundary.len();
        if data.len() < boundary_len + 2 {
            return Err(MultipartError::UnexpectedEof);
        }

        let end = data.len() - boundary_len + 1;
        for i in start..end {
            if !data[i..].starts_with(boundary) {
                continue;
            }

            // Inside part payloads, boundaries must be preceded by CRLF.
            if i < 2 || data[i - 2..i] != *b"\r\n" {
                continue;
            }

            let boundary_end = i + boundary_len;
            if boundary_end + 2 > data.len() {
                return Err(MultipartError::UnexpectedEof);
            }
            let suffix = &data[boundary_end..boundary_end + 2];
            if suffix != b"\r\n" && suffix != b"--" {
                continue;
            }

            return Ok(i);
        }

        Err(MultipartError::UnexpectedEof)
    }

    fn parse_part_headers(
        &self,
        data: &[u8],
        start: usize,
    ) -> Result<(HashMap<String, String>, usize), MultipartError> {
        let mut headers = HashMap::new();
        let mut pos = start;

        loop {
            let line_end = find_crlf(data, pos)?;
            let line = &data[pos..line_end];
            if line.is_empty() {
                return Ok((headers, line_end + 2));
            }

            let line_str =
                std::str::from_utf8(line).map_err(|_| MultipartError::InvalidPartHeaders {
                    detail: "invalid UTF-8 in header".to_string(),
                })?;

            if let Some((name, value)) = line_str.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }

            pos = line_end + 2;
        }
    }
}

fn find_crlf(data: &[u8], start: usize) -> Result<usize, MultipartError> {
    if data.len() < 2 {
        return Err(MultipartError::UnexpectedEof);
    }
    let end = data.len() - 1;
    for i in start..end {
        if data[i..i + 2] == *b"\r\n" {
            return Ok(i);
        }
    }
    Err(MultipartError::UnexpectedEof)
}

/// Parse Content-Disposition header value.
///
/// Format: `form-data; name=\"field\"; filename=\"file.txt\"`
fn parse_content_disposition(value: &str) -> Result<(String, Option<String>), MultipartError> {
    let mut name = None;
    let mut filename = None;

    for part in value.split(';') {
        let part = part.trim();
        if part.eq_ignore_ascii_case("form-data") {
            continue;
        }

        if let Some((key, raw_value)) = part.split_once('=') {
            let key = key.trim();
            let value = raw_value.trim();
            if key.eq_ignore_ascii_case("name") {
                name = Some(unquote(value));
            } else if key.eq_ignore_ascii_case("filename") {
                let unquoted = unquote(value);
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
    }

    let name = name.ok_or_else(|| MultipartError::InvalidContentDisposition {
        detail: "missing name parameter".to_string(),
    })?;

    Ok((name, filename))
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Parsed multipart form data.
#[derive(Debug)]
pub struct MultipartForm {
    parts: Vec<Part>,
    spool_threshold: usize,
}

impl Default for MultipartForm {
    fn default() -> Self {
        Self::new()
    }
}

impl MultipartForm {
    /// Create a new empty form.
    #[must_use]
    pub fn new() -> Self {
        Self {
            parts: Vec::new(),
            spool_threshold: DEFAULT_SPOOL_THRESHOLD,
        }
    }

    /// Create from parsed parts.
    #[must_use]
    pub fn from_parts(parts: Vec<Part>) -> Self {
        Self {
            parts,
            spool_threshold: DEFAULT_SPOOL_THRESHOLD,
        }
    }

    /// Create from parsed parts with a custom file spool threshold.
    #[must_use]
    pub fn from_parts_with_spool_threshold(parts: Vec<Part>, spool_threshold: usize) -> Self {
        Self {
            parts,
            spool_threshold,
        }
    }

    /// Get all parts.
    #[must_use]
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// Consume the form and return all parsed parts.
    #[must_use]
    pub fn into_parts(mut self) -> Vec<Part> {
        std::mem::take(&mut self.parts)
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
            .and_then(|part| Self::upload_from_borrowed_part(part, self.spool_threshold))
    }

    /// Remove and return a file by field name without cloning part data.
    pub fn take_file(&mut self, name: &str) -> Option<UploadFile> {
        let index = self
            .parts
            .iter()
            .position(|p| p.name == name && p.filename.is_some())?;
        let part = self.parts.swap_remove(index);
        UploadFile::from_part_with_spool_threshold(part, self.spool_threshold)
    }

    /// Get all files.
    #[must_use]
    pub fn files(&self) -> Vec<UploadFile> {
        self.parts
            .iter()
            .filter(|p| p.filename.is_some())
            .filter_map(|part| Self::upload_from_borrowed_part(part, self.spool_threshold))
            .collect()
    }

    /// Consume the form and return all file uploads without cloning part data.
    #[must_use]
    pub fn into_files(mut self) -> Vec<UploadFile> {
        let spool_threshold = self.spool_threshold;
        std::mem::take(&mut self.parts)
            .into_iter()
            .filter_map(|part| UploadFile::from_part_with_spool_threshold(part, spool_threshold))
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
            .filter_map(|part| Self::upload_from_borrowed_part(part, self.spool_threshold))
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

    fn upload_from_borrowed_part(part: &Part, spool_threshold: usize) -> Option<UploadFile> {
        let data = part.bytes().ok()?;
        let owned_part = Part {
            name: part.name.clone(),
            filename: part.filename.clone(),
            content_type: part.content_type.clone(),
            data,
            headers: part.headers.clone(),
            spooled_path: None,
            spooled_len: None,
        };
        UploadFile::from_part_with_spool_threshold(owned_part, spool_threshold)
    }
}

impl Drop for MultipartForm {
    fn drop(&mut self) {
        for part in &self.parts {
            if let Some(path) = part.spooled_path() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_boundary() {
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
    fn test_parse_boundary_case_insensitive_param_name() {
        let ct = r#"multipart/form-data; Boundary="simple-boundary""#;
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
    fn test_parse_boundary_rejects_too_long_value() {
        let too_long = "a".repeat(MAX_BOUNDARY_LEN + 1);
        let ct = format!("multipart/form-data; boundary={too_long}");
        let result = parse_boundary(&ct);
        assert!(matches!(result, Err(MultipartError::InvalidBoundary)));
    }

    #[test]
    fn test_parse_boundary_wrong_content_type() {
        let ct = "application/json";
        let result = parse_boundary(ct);
        assert!(matches!(result, Err(MultipartError::InvalidBoundary)));
    }

    #[test]
    fn test_parse_content_disposition_case_insensitive_params() {
        let (name, filename) =
            parse_content_disposition("form-data; Name=\"field\"; FileName=\"upload.txt\"")
                .expect("content disposition should parse");
        assert_eq!(name, "field");
        assert_eq!(filename.as_deref(), Some("upload.txt"));
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
    fn test_parse_simple_form_with_mixed_case_disposition_params() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; Name=\"field1\"\r\n",
            "\r\n",
            "value1\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; Name=\"file\"; FileName=\"note.txt\"\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "hello\r\n",
            "------boundary--\r\n"
        );

        let parser = MultipartParser::new(boundary, MultipartConfig::default());
        let parts = parser.parse(body.as_bytes()).expect("multipart parse");

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "field1");
        assert_eq!(parts[0].text(), Some("value1"));
        assert_eq!(parts[1].name, "file");
        assert_eq!(parts[1].filename.as_deref(), Some("note.txt"));
        assert_eq!(parts[1].text(), Some("hello"));
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

        assert_eq!(parts[0].name, "description");
        assert!(parts[0].is_field());
        assert_eq!(parts[0].text(), Some("A test file"));

        assert_eq!(parts[1].name, "file");
        assert!(parts[1].is_file());
        assert_eq!(parts[1].data, vec![0x00, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_multipart_form_helpers() {
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
        assert_eq!(form.files().len(), 1);
        let f = form.get_file("avatar").unwrap();
        assert_eq!(f.filename, "photo.jpg");
        assert_eq!(f.content_type, "image/jpeg");
    }

    #[test]
    fn test_multipart_form_take_file_and_into_files_move_data() {
        let parts = vec![
            Part {
                name: "note".to_string(),
                filename: None,
                content_type: None,
                data: b"hi".to_vec(),
                headers: HashMap::new(),
                spooled_path: None,
                spooled_len: None,
            },
            Part {
                name: "avatar".to_string(),
                filename: Some("a.bin".to_string()),
                content_type: Some("application/octet-stream".to_string()),
                data: vec![1, 2, 3, 4],
                headers: HashMap::new(),
                spooled_path: None,
                spooled_len: None,
            },
            Part {
                name: "avatar".to_string(),
                filename: Some("b.bin".to_string()),
                content_type: Some("application/octet-stream".to_string()),
                data: vec![9; 32],
                headers: HashMap::new(),
                spooled_path: None,
                spooled_len: None,
            },
        ];

        let mut form = MultipartForm::from_parts_with_spool_threshold(parts, 8);
        let first = form.take_file("avatar").expect("first avatar file");
        assert_eq!(first.filename, "a.bin");
        assert_eq!(form.get_field("note"), Some("hi"));
        assert_eq!(form.get_files("avatar").len(), 1);

        let files = form.into_files();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "b.bin");
        assert!(
            files[0].is_spooled(),
            "remaining file should respect custom spool threshold"
        );
    }

    #[test]
    fn test_multipart_form_respects_custom_spool_threshold() {
        let part = Part {
            name: "avatar".to_string(),
            filename: Some("photo.jpg".to_string()),
            content_type: Some("image/jpeg".to_string()),
            data: vec![0xAB; 64],
            headers: HashMap::new(),
            spooled_path: None,
            spooled_len: None,
        };

        let form = MultipartForm::from_parts_with_spool_threshold(vec![part], 1);
        let mut file = form.get_file("avatar").expect("avatar file");
        assert!(file.is_spooled(), "custom threshold should force spooling");

        let spooled_path = file
            .spooled_path()
            .expect("spooled file path")
            .to_path_buf();
        assert!(spooled_path.exists(), "spooled file should exist");

        futures_executor::block_on(file.close()).expect("close upload");
        assert!(!spooled_path.exists(), "spooled file should be removed");
    }

    #[test]
    fn test_boundary_like_sequence_in_part_body_does_not_terminate_part() {
        // Ensure we do not treat an in-body "------boundaryX" as a boundary delimiter.
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"data.bin\"\r\n",
            "Content-Type: application/octet-stream\r\n",
            "\r\n",
            "line1\r\n",
            "------boundaryX\r\n",
            "line2\r\n",
            "------boundary--\r\n"
        );

        let parser = MultipartParser::new(boundary, MultipartConfig::default());
        let parts = parser.parse(body.as_bytes()).unwrap();

        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].name, "file");
        assert!(parts[0].is_file());
        assert_eq!(parts[0].data, b"line1\r\n------boundaryX\r\nline2".to_vec());
    }

    #[test]
    fn test_upload_file_async_read_seek_write() {
        let part = Part {
            name: "file".to_string(),
            filename: Some("note.txt".to_string()),
            content_type: Some("text/plain".to_string()),
            data: b"hello".to_vec(),
            headers: HashMap::new(),
            spooled_path: None,
            spooled_len: None,
        };

        let mut file = UploadFile::from_part(part).expect("expected file");
        assert!(!file.is_spooled());

        let first = futures_executor::block_on(file.read(Some(2))).expect("read prefix");
        assert_eq!(first, b"he".to_vec());

        futures_executor::block_on(file.seek(SeekFrom::Start(0))).expect("seek start");
        futures_executor::block_on(file.write(b"Y")).expect("overwrite first byte");
        futures_executor::block_on(file.seek(SeekFrom::Start(0))).expect("seek start");
        let all = futures_executor::block_on(file.read(None)).expect("read full file");
        assert_eq!(all, b"Yello".to_vec());

        futures_executor::block_on(file.close()).expect("close upload");
        assert!(futures_executor::block_on(file.read(Some(1))).is_err());
    }

    #[test]
    fn test_upload_file_spools_large_payload() {
        let payload_len = DEFAULT_SPOOL_THRESHOLD + 4096;
        let payload = vec![b'a'; payload_len];
        let part = Part {
            name: "file".to_string(),
            filename: Some("large.bin".to_string()),
            content_type: Some("application/octet-stream".to_string()),
            data: payload.clone(),
            headers: HashMap::new(),
            spooled_path: None,
            spooled_len: None,
        };

        let mut file = UploadFile::from_part(part).expect("expected file");
        assert!(file.is_spooled());
        assert_eq!(file.size(), payload_len);

        let spooled_path = file
            .spooled_path()
            .expect("spooled file path")
            .to_path_buf();
        assert!(spooled_path.exists());

        let full = file.bytes().expect("read full bytes");
        assert_eq!(full.len(), payload_len);
        assert_eq!(full, payload);

        let prefix = futures_executor::block_on(file.read(Some(8))).expect("read prefix");
        assert_eq!(prefix, b"aaaaaaaa".to_vec());

        futures_executor::block_on(file.close()).expect("close upload");
        assert!(!spooled_path.exists());
    }

    #[test]
    fn test_upload_file_seek_before_start_is_error() {
        let part = Part {
            name: "file".to_string(),
            filename: Some("note.txt".to_string()),
            content_type: Some("text/plain".to_string()),
            data: b"hello".to_vec(),
            headers: HashMap::new(),
            spooled_path: None,
            spooled_len: None,
        };

        let mut file = UploadFile::from_part(part).expect("expected file");
        let err = futures_executor::block_on(file.seek(SeekFrom::Current(-10)))
            .expect_err("seek should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_incremental_parse_with_chunked_input() {
        let boundary = "----boundary";
        let body = concat!(
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"field1\"\r\n",
            "\r\n",
            "value1\r\n",
            "------boundary\r\n",
            "Content-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "hello-stream\r\n",
            "------boundary--\r\n"
        )
        .as_bytes()
        .to_vec();

        let parser = MultipartParser::new(boundary, MultipartConfig::default());
        let mut state = MultipartStreamState::default();
        let mut buffer = Vec::new();
        let mut parts = Vec::new();

        for chunk in body.chunks(5) {
            buffer.extend_from_slice(chunk);
            let mut parsed = parser
                .parse_incremental(&mut buffer, &mut state, false)
                .expect("incremental parse");
            parts.append(&mut parsed);
        }

        let mut tail = parser
            .parse_incremental(&mut buffer, &mut state, true)
            .expect("final parse");
        parts.append(&mut tail);

        assert!(state.is_done());
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "field1");
        assert_eq!(parts[0].text(), Some("value1"));
        assert_eq!(parts[1].name, "file");
        assert_eq!(parts[1].filename.as_deref(), Some("test.txt"));
        assert_eq!(parts[1].data, b"hello-stream".to_vec());
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_incremental_parse_keeps_buffer_bounded_for_large_streamed_file() {
        let boundary = "----boundary";
        let payload = vec![b'x'; 256 * 1024];

        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"large.bin\"\r\n",
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(&payload);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let parser =
            MultipartParser::new(boundary, MultipartConfig::default().spool_threshold(1024));
        let mut state = MultipartStreamState::default();
        let mut buffer = Vec::new();
        let mut parts = Vec::new();
        let mut max_buffer_len = 0usize;

        for chunk in body.chunks(513) {
            buffer.extend_from_slice(chunk);
            let mut parsed = parser
                .parse_incremental(&mut buffer, &mut state, false)
                .expect("incremental parse");
            parts.append(&mut parsed);
            max_buffer_len = max_buffer_len.max(buffer.len());
        }

        let mut tail = parser
            .parse_incremental(&mut buffer, &mut state, true)
            .expect("final parse");
        parts.append(&mut tail);

        assert!(state.is_done());
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].name, "file");
        assert_eq!(parts[0].filename.as_deref(), Some("large.bin"));
        assert!(parts[0].is_spooled());
        let spooled_path = parts[0].spooled_path().expect("spooled path").to_path_buf();
        assert!(parts[0].data.is_empty());
        assert_eq!(parts[0].bytes().expect("read spooled bytes"), payload);
        std::fs::remove_file(spooled_path).expect("cleanup spooled test file");

        // During streamed parsing, parser buffer should stay bounded rather than
        // growing with the whole payload size.
        assert!(
            max_buffer_len < 8 * 1024,
            "incremental parser buffer grew too large: {max_buffer_len}"
        );
    }

    #[test]
    fn test_multipart_form_drop_cleans_spooled_parts() {
        let boundary = "----boundary";
        let payload = vec![b'z'; 32 * 1024];

        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"file\"; filename=\"drop.bin\"\r\n",
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(&payload);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let parser =
            MultipartParser::new(boundary, MultipartConfig::default().spool_threshold(1024));
        let mut state = MultipartStreamState::default();
        let mut buffer = Vec::new();
        let mut parts = Vec::new();
        for chunk in body.chunks(257) {
            buffer.extend_from_slice(chunk);
            let mut parsed = parser
                .parse_incremental(&mut buffer, &mut state, false)
                .expect("incremental parse");
            parts.append(&mut parsed);
        }
        let mut tail = parser
            .parse_incremental(&mut buffer, &mut state, true)
            .expect("final parse");
        parts.append(&mut tail);

        assert_eq!(parts.len(), 1);
        assert!(parts[0].is_spooled());
        let spooled_path = parts[0].spooled_path().expect("spooled path").to_path_buf();
        assert!(spooled_path.exists());

        let form = MultipartForm::from_parts_with_spool_threshold(parts, 1024);
        drop(form);

        assert!(
            !spooled_path.exists(),
            "dropping multipart form should clean spooled part file"
        );
    }
}
