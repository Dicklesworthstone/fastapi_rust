//! Startup banner component.
//!
//! Displays the server startup information with ASCII art logo,
//! version info, server URLs, and documentation links.

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";

/// ASCII art logo for fastapi_rust.
const LOGO_ASCII: &str = r"
  ___         _      _   ___ ___   ___         _
 | __| _ _ __| |_   /_\ | _ \_ _| | _ \_  _ __| |_
 | _| / _` (_-<  _| / _ \|  _/| |  |   / || (_-<  _|
 |_|  \__,_/__/\__|/_/ \_\_| |___| |_|_\_,_/__/\__|
";

/// Simple text logo for plain mode.
const LOGO_PLAIN: &str = "FastAPI Rust";

/// Server information for the banner.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    /// Server version string.
    pub version: String,
    /// Host address.
    pub host: String,
    /// Port number.
    pub port: u16,
    /// Whether HTTPS is enabled.
    pub https: bool,
    /// OpenAPI docs path (if enabled).
    pub docs_path: Option<String>,
    /// ReDoc path (if enabled).
    pub redoc_path: Option<String>,
    /// OpenAPI JSON path.
    pub openapi_path: Option<String>,
}

impl ServerInfo {
    /// Create a new server info with minimal configuration.
    #[must_use]
    pub fn new(version: impl Into<String>, host: impl Into<String>, port: u16) -> Self {
        Self {
            version: version.into(),
            host: host.into(),
            port,
            https: false,
            docs_path: None,
            redoc_path: None,
            openapi_path: None,
        }
    }

    /// Set whether HTTPS is enabled.
    #[must_use]
    pub fn https(mut self, enabled: bool) -> Self {
        self.https = enabled;
        self
    }

    /// Set the OpenAPI docs path.
    #[must_use]
    pub fn docs_path(mut self, path: impl Into<String>) -> Self {
        self.docs_path = Some(path.into());
        self
    }

    /// Set the ReDoc path.
    #[must_use]
    pub fn redoc_path(mut self, path: impl Into<String>) -> Self {
        self.redoc_path = Some(path.into());
        self
    }

    /// Set the OpenAPI JSON path.
    #[must_use]
    pub fn openapi_path(mut self, path: impl Into<String>) -> Self {
        self.openapi_path = Some(path.into());
        self
    }

    /// Get the base URL.
    #[must_use]
    pub fn base_url(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        format!("{}://{}:{}", scheme, self.host, self.port)
    }
}

impl Default for ServerInfo {
    fn default() -> Self {
        Self::new("0.1.0", "127.0.0.1", 8000)
    }
}

/// Configuration for banner display.
#[derive(Debug, Clone)]
pub struct BannerConfig {
    /// Whether to show the ASCII art logo.
    pub show_logo: bool,
    /// Whether to show documentation links.
    pub show_docs: bool,
    /// Whether to show a border around the banner.
    pub show_border: bool,
    /// Custom tagline (if any).
    pub tagline: Option<String>,
}

impl Default for BannerConfig {
    fn default() -> Self {
        Self {
            show_logo: true,
            show_docs: true,
            show_border: true,
            tagline: Some("High performance, easy to learn, fast to code".to_string()),
        }
    }
}

/// Startup banner display.
#[derive(Debug, Clone)]
pub struct Banner {
    mode: OutputMode,
    theme: FastApiTheme,
    config: BannerConfig,
}

impl Banner {
    /// Create a new banner with the specified mode.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            config: BannerConfig::default(),
        }
    }

    /// Create a banner with custom configuration.
    #[must_use]
    pub fn with_config(mode: OutputMode, config: BannerConfig) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            config,
        }
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Render the banner to a string.
    #[must_use]
    pub fn render(&self, info: &ServerInfo) -> String {
        match self.mode {
            OutputMode::Plain => self.render_plain(info),
            OutputMode::Minimal => self.render_minimal(info),
            OutputMode::Rich => self.render_rich(info),
        }
    }

    fn render_plain(&self, info: &ServerInfo) -> String {
        let mut lines = Vec::new();

        // Header
        lines.push(format!("{} v{}", LOGO_PLAIN, info.version));

        if let Some(tagline) = &self.config.tagline {
            lines.push(tagline.clone());
        }

        lines.push(String::new());

        // Server info
        lines.push(format!("Server: {}", info.base_url()));

        // Documentation links
        if self.config.show_docs {
            if let Some(docs) = &info.docs_path {
                lines.push(format!("Docs:   {}{}", info.base_url(), docs));
            }
            if let Some(redoc) = &info.redoc_path {
                lines.push(format!("ReDoc:  {}{}", info.base_url(), redoc));
            }
            if let Some(openapi) = &info.openapi_path {
                lines.push(format!("OpenAPI: {}{}", info.base_url(), openapi));
            }
        }

        lines.join("\n")
    }

    fn render_minimal(&self, info: &ServerInfo) -> String {
        let mut lines = Vec::new();
        let primary = self.theme.primary.to_ansi_fg();
        let muted = self.theme.muted.to_ansi_fg();

        // Header with color
        lines.push(format!(
            "{primary}{ANSI_BOLD}{} v{}{ANSI_RESET}",
            LOGO_PLAIN, info.version
        ));

        if let Some(tagline) = &self.config.tagline {
            lines.push(format!("{muted}{tagline}{ANSI_RESET}"));
        }

        lines.push(String::new());

        // Server info
        let accent = self.theme.accent.to_ansi_fg();
        lines.push(format!("Server: {accent}{}{ANSI_RESET}", info.base_url()));

        // Documentation links
        if self.config.show_docs {
            if let Some(docs) = &info.docs_path {
                lines.push(format!(
                    "Docs:   {accent}{}{}{ANSI_RESET}",
                    info.base_url(),
                    docs
                ));
            }
        }

        lines.join("\n")
    }

    fn render_rich(&self, info: &ServerInfo) -> String {
        let mut lines = Vec::new();
        let primary = self.theme.primary.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let muted = self.theme.muted.to_ansi_fg();
        let border = self.theme.border.to_ansi_fg();

        // ASCII art logo
        if self.config.show_logo {
            for line in LOGO_ASCII.lines() {
                if !line.is_empty() {
                    lines.push(format!("{primary}{line}{ANSI_RESET}"));
                }
            }
        }

        // Version
        lines.push(format!(
            "{muted}                                    v{}{ANSI_RESET}",
            info.version
        ));

        // Tagline
        if let Some(tagline) = &self.config.tagline {
            lines.push(format!("{muted}  {tagline}{ANSI_RESET}"));
        }

        lines.push(String::new());

        // Border
        if self.config.show_border {
            lines.push(format!(
                "{border}  ─────────────────────────────────────────────{ANSI_RESET}"
            ));
        }

        // Server info
        let success = self.theme.success.to_ansi_fg();
        lines.push(format!(
            "  {success}▶{ANSI_RESET} Server running at {accent}{}{ANSI_RESET}",
            info.base_url()
        ));

        // Documentation links
        if self.config.show_docs {
            if let Some(docs) = &info.docs_path {
                lines.push(format!(
                    "  {muted}📖{ANSI_RESET} Interactive docs: {accent}{}{}{ANSI_RESET}",
                    info.base_url(),
                    docs
                ));
            }
            if let Some(redoc) = &info.redoc_path {
                lines.push(format!(
                    "  {muted}📚{ANSI_RESET} ReDoc:            {accent}{}{}{ANSI_RESET}",
                    info.base_url(),
                    redoc
                ));
            }
            if let Some(openapi) = &info.openapi_path {
                lines.push(format!(
                    "  {muted}📋{ANSI_RESET} OpenAPI JSON:     {accent}{}{}{ANSI_RESET}",
                    info.base_url(),
                    openapi
                ));
            }
        }

        // Footer border
        if self.config.show_border {
            lines.push(format!(
                "{border}  ─────────────────────────────────────────────{ANSI_RESET}"
            ));
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_info_new() {
        let info = ServerInfo::new("1.0.0", "localhost", 8080);
        assert_eq!(info.version, "1.0.0");
        assert_eq!(info.host, "localhost");
        assert_eq!(info.port, 8080);
        assert!(!info.https);
    }

    #[test]
    fn test_server_info_builder() {
        let info = ServerInfo::new("1.0.0", "0.0.0.0", 443)
            .https(true)
            .docs_path("/docs")
            .redoc_path("/redoc")
            .openapi_path("/openapi.json");

        assert!(info.https);
        assert_eq!(info.docs_path, Some("/docs".to_string()));
        assert_eq!(info.redoc_path, Some("/redoc".to_string()));
        assert_eq!(info.openapi_path, Some("/openapi.json".to_string()));
    }

    #[test]
    fn test_server_info_base_url() {
        let http = ServerInfo::new("1.0.0", "localhost", 8000);
        assert_eq!(http.base_url(), "http://localhost:8000");

        let https = ServerInfo::new("1.0.0", "example.com", 443).https(true);
        assert_eq!(https.base_url(), "https://example.com:443");
    }

    #[test]
    fn test_banner_plain_contains_essentials() {
        let banner = Banner::new(OutputMode::Plain);
        let info = ServerInfo::new("0.1.0", "127.0.0.1", 8000).docs_path("/docs");

        let output = banner.render(&info);

        assert!(output.contains("FastAPI Rust"));
        assert!(output.contains("v0.1.0"));
        assert!(output.contains("http://127.0.0.1:8000"));
        assert!(output.contains("/docs"));
    }

    #[test]
    fn test_banner_plain_no_ansi() {
        let banner = Banner::new(OutputMode::Plain);
        let info = ServerInfo::default();

        let output = banner.render(&info);

        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn test_banner_rich_has_ansi() {
        let banner = Banner::new(OutputMode::Rich);
        let info = ServerInfo::default();

        let output = banner.render(&info);

        assert!(output.contains("\x1b["));
    }

    #[test]
    fn test_banner_config_no_logo() {
        let config = BannerConfig {
            show_logo: false,
            ..Default::default()
        };
        let banner = Banner::with_config(OutputMode::Rich, config);
        let info = ServerInfo::default();

        let output = banner.render(&info);

        // Should not contain the distinctive ASCII art characters
        assert!(!output.contains("___"));
    }

    #[test]
    fn test_banner_config_no_docs() {
        let config = BannerConfig {
            show_docs: false,
            ..Default::default()
        };
        let banner = Banner::with_config(OutputMode::Plain, config);
        let info = ServerInfo::new("1.0.0", "localhost", 8000).docs_path("/docs");

        let output = banner.render(&info);

        // Should not contain docs link
        assert!(!output.contains("Docs:"));
    }
}
