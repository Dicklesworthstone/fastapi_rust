//! API versioning patterns.
//!
//! Supports three strategies for API version negotiation:
//!
//! - **URL prefix**: `/v1/users`, `/v2/users`
//! - **Header**: `X-API-Version: 1`
//! - **Accept header**: `application/vnd.myapi.v1+json`
//!
//! # Example
//!
//! ```
//! use fastapi_core::versioning::{ApiVersion, VersionStrategy, VersionConfig};
//!
//! let config = VersionConfig::new()
//!     .strategy(VersionStrategy::UrlPrefix)
//!     .current(2)
//!     .supported(&[1, 2])
//!     .deprecated(&[1]);
//!
//! let v = ApiVersion::from_path("/v1/users");
//! assert_eq!(v, Some(ApiVersion(1)));
//! assert!(config.is_deprecated(&ApiVersion(1)));
//! ```

use std::fmt;

/// An API version number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ApiVersion(pub u32);

impl ApiVersion {
    /// Extract version from a URL path prefix like `/v1/...`.
    ///
    /// Returns `None` if the path doesn't start with `/v<N>`.
    pub fn from_path(path: &str) -> Option<Self> {
        let path = path.strip_prefix('/')?;
        let seg = path.split('/').next()?;
        let ver_str = seg.strip_prefix('v').or_else(|| seg.strip_prefix('V'))?;
        ver_str.parse::<u32>().ok().map(ApiVersion)
    }

    /// Extract version from a header value like `"2"` or `"v2"`.
    pub fn from_header_value(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        let num_str = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);
        num_str.parse::<u32>().ok().map(ApiVersion)
    }

    /// Extract version from an Accept header like `application/vnd.myapi.v2+json`.
    ///
    /// Looks for a `v<N>` pattern within the media type.
    pub fn from_accept_header(accept: &str) -> Option<Self> {
        // Look for vnd.*.v<N> pattern
        for part in accept.split(';') {
            let part = part.trim();
            // Find v<N> in the media type
            for segment in part.split('.') {
                if let Some(num_str) = segment
                    .strip_prefix('v')
                    .or_else(|| segment.strip_prefix('V'))
                {
                    // Take only digits before any '+' suffix
                    let digits: String = num_str.chars().take_while(char::is_ascii_digit).collect();
                    if let Ok(n) = digits.parse::<u32>() {
                        return Some(ApiVersion(n));
                    }
                }
            }
        }
        None
    }

    /// Strip the version prefix from a path.
    ///
    /// `/v1/users` → `/users`, `/v2/items/5` → `/items/5`.
    /// Returns the original path if no version prefix is found.
    pub fn strip_prefix(path: &str) -> &str {
        let Some(rest) = path.strip_prefix('/') else {
            return path;
        };
        let Some(after_seg) = rest.find('/') else {
            return path;
        };
        let seg = &rest[..after_seg];
        if seg.starts_with('v') || seg.starts_with('V') {
            let num_part = &seg[1..];
            if num_part.chars().all(|c| c.is_ascii_digit()) && !num_part.is_empty() {
                return &rest[after_seg..];
            }
        }
        path
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// Strategy for extracting the API version from a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionStrategy {
    /// Version in URL path prefix: `/v1/users`.
    UrlPrefix,
    /// Version in a custom header (default: `X-API-Version`).
    Header,
    /// Version in Accept header: `application/vnd.myapi.v1+json`.
    AcceptHeader,
}

/// Configuration for API versioning.
#[derive(Debug, Clone)]
pub struct VersionConfig {
    /// How to extract the version.
    pub strategy: VersionStrategy,
    /// The current (latest) version.
    pub current_version: u32,
    /// All supported versions.
    pub supported_versions: Vec<u32>,
    /// Deprecated versions (still work, but emit a warning header).
    pub deprecated_versions: Vec<u32>,
    /// Header name for the Header strategy.
    pub version_header: String,
    /// Header added to responses for deprecated versions.
    pub deprecation_header: String,
}

impl Default for VersionConfig {
    fn default() -> Self {
        Self {
            strategy: VersionStrategy::UrlPrefix,
            current_version: 1,
            supported_versions: vec![1],
            deprecated_versions: Vec::new(),
            version_header: "X-API-Version".to_string(),
            deprecation_header: "Deprecation".to_string(),
        }
    }
}

impl VersionConfig {
    /// Create a new version config with defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the versioning strategy.
    #[must_use]
    pub fn strategy(mut self, strategy: VersionStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set the current version.
    #[must_use]
    pub fn current(mut self, version: u32) -> Self {
        self.current_version = version;
        self
    }

    /// Set supported versions.
    #[must_use]
    pub fn supported(mut self, versions: &[u32]) -> Self {
        self.supported_versions = versions.to_vec();
        self
    }

    /// Set deprecated versions.
    #[must_use]
    pub fn deprecated(mut self, versions: &[u32]) -> Self {
        self.deprecated_versions = versions.to_vec();
        self
    }

    /// Set the version header name.
    #[must_use]
    pub fn version_header(mut self, name: impl Into<String>) -> Self {
        self.version_header = name.into();
        self
    }

    /// Check if a version is supported.
    pub fn is_supported(&self, version: &ApiVersion) -> bool {
        self.supported_versions.contains(&version.0)
    }

    /// Check if a version is deprecated.
    pub fn is_deprecated(&self, version: &ApiVersion) -> bool {
        self.deprecated_versions.contains(&version.0)
    }

    /// Extract version from a request path (URL prefix strategy).
    pub fn extract_from_path(&self, path: &str) -> Option<ApiVersion> {
        ApiVersion::from_path(path)
    }

    /// Extract version from a header value.
    pub fn extract_from_header(&self, value: &str) -> Option<ApiVersion> {
        ApiVersion::from_header_value(value)
    }

    /// Generate deprecation warning header value for a deprecated version.
    pub fn deprecation_warning(&self, version: &ApiVersion) -> Option<String> {
        if self.is_deprecated(version) {
            Some(format!(
                "API version {} is deprecated. Please migrate to v{}.",
                version, self.current_version
            ))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_version_from_path() {
        assert_eq!(ApiVersion::from_path("/v1/users"), Some(ApiVersion(1)));
        assert_eq!(ApiVersion::from_path("/v2/items/5"), Some(ApiVersion(2)));
        assert_eq!(ApiVersion::from_path("/v10/"), Some(ApiVersion(10)));
        assert_eq!(ApiVersion::from_path("/users"), None);
        assert_eq!(ApiVersion::from_path("/"), None);
        assert_eq!(ApiVersion::from_path(""), None);
    }

    #[test]
    fn api_version_from_header() {
        assert_eq!(ApiVersion::from_header_value("1"), Some(ApiVersion(1)));
        assert_eq!(ApiVersion::from_header_value("v2"), Some(ApiVersion(2)));
        assert_eq!(ApiVersion::from_header_value(" 3 "), Some(ApiVersion(3)));
        assert_eq!(ApiVersion::from_header_value("abc"), None);
    }

    #[test]
    fn api_version_from_accept() {
        assert_eq!(
            ApiVersion::from_accept_header("application/vnd.myapi.v1+json"),
            Some(ApiVersion(1))
        );
        assert_eq!(
            ApiVersion::from_accept_header("application/vnd.api.v3+json; charset=utf-8"),
            Some(ApiVersion(3))
        );
        assert_eq!(ApiVersion::from_accept_header("application/json"), None);
    }

    #[test]
    fn strip_version_prefix() {
        assert_eq!(ApiVersion::strip_prefix("/v1/users"), "/users");
        assert_eq!(ApiVersion::strip_prefix("/v2/items/5"), "/items/5");
        assert_eq!(ApiVersion::strip_prefix("/users"), "/users");
        assert_eq!(ApiVersion::strip_prefix("/"), "/");
    }

    #[test]
    fn version_display() {
        assert_eq!(ApiVersion(1).to_string(), "v1");
        assert_eq!(ApiVersion(42).to_string(), "v42");
    }

    #[test]
    fn version_config_builder() {
        let config = VersionConfig::new()
            .strategy(VersionStrategy::Header)
            .current(3)
            .supported(&[1, 2, 3])
            .deprecated(&[1]);

        assert_eq!(config.strategy, VersionStrategy::Header);
        assert_eq!(config.current_version, 3);
        assert!(config.is_supported(&ApiVersion(2)));
        assert!(!config.is_supported(&ApiVersion(4)));
        assert!(config.is_deprecated(&ApiVersion(1)));
        assert!(!config.is_deprecated(&ApiVersion(2)));
    }

    #[test]
    fn deprecation_warning() {
        let config = VersionConfig::new()
            .current(2)
            .supported(&[1, 2])
            .deprecated(&[1]);

        let warning = config.deprecation_warning(&ApiVersion(1));
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("v2"));

        assert!(config.deprecation_warning(&ApiVersion(2)).is_none());
    }

    #[test]
    fn version_config_defaults() {
        let config = VersionConfig::default();
        assert_eq!(config.strategy, VersionStrategy::UrlPrefix);
        assert_eq!(config.current_version, 1);
        assert_eq!(config.version_header, "X-API-Version");
    }

    #[test]
    fn version_ordering() {
        assert!(ApiVersion(1) < ApiVersion(2));
        assert!(ApiVersion(3) > ApiVersion(1));
        assert_eq!(ApiVersion(1), ApiVersion(1));
    }
}
