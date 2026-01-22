//! Route table display component.
//!
//! Displays registered routes in a formatted table with method coloring
//! and auto-width calculation.

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;
use std::fmt::Write;

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";

/// A single route entry for display.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    /// HTTP method.
    pub method: String,
    /// Route path pattern.
    pub path: String,
    /// Handler name or function.
    pub handler: Option<String>,
    /// Tags/groups for the route.
    pub tags: Vec<String>,
    /// Whether the route is deprecated.
    pub deprecated: bool,
}

impl RouteEntry {
    /// Create a new route entry.
    #[must_use]
    pub fn new(method: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            handler: None,
            tags: Vec::new(),
            deprecated: false,
        }
    }

    /// Set the handler name.
    #[must_use]
    pub fn handler(mut self, handler: impl Into<String>) -> Self {
        self.handler = Some(handler.into());
        self
    }

    /// Add a tag.
    #[must_use]
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add multiple tags.
    #[must_use]
    pub fn tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.tags.extend(tags.into_iter().map(Into::into));
        self
    }

    /// Mark as deprecated.
    #[must_use]
    pub fn deprecated(mut self, deprecated: bool) -> Self {
        self.deprecated = deprecated;
        self
    }
}

/// Configuration for route table display.
#[derive(Debug, Clone)]
pub struct RouteTableConfig {
    /// Whether to show handler names.
    pub show_handlers: bool,
    /// Whether to show tags.
    pub show_tags: bool,
    /// Whether to show deprecated routes.
    pub show_deprecated: bool,
    /// Maximum terminal width (0 = auto-detect or unlimited).
    pub max_width: usize,
    /// Title for the table.
    pub title: Option<String>,
}

impl Default for RouteTableConfig {
    fn default() -> Self {
        Self {
            show_handlers: true,
            show_tags: true,
            show_deprecated: true,
            max_width: 0,
            title: Some("Registered Routes".to_string()),
        }
    }
}

/// Route table display.
#[derive(Debug, Clone)]
pub struct RouteDisplay {
    mode: OutputMode,
    theme: FastApiTheme,
    config: RouteTableConfig,
}

impl RouteDisplay {
    /// Create a new route display.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            config: RouteTableConfig::default(),
        }
    }

    /// Create with custom configuration.
    #[must_use]
    pub fn with_config(mode: OutputMode, config: RouteTableConfig) -> Self {
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

    /// Render the route table.
    #[must_use]
    pub fn render(&self, routes: &[RouteEntry]) -> String {
        // Filter routes if needed
        let routes: Vec<_> = if self.config.show_deprecated {
            routes.to_vec()
        } else {
            routes.iter().filter(|r| !r.deprecated).cloned().collect()
        };

        if routes.is_empty() {
            return self.render_empty();
        }

        match self.mode {
            OutputMode::Plain => self.render_plain(&routes),
            OutputMode::Minimal => self.render_minimal(&routes),
            OutputMode::Rich => self.render_rich(&routes),
        }
    }

    fn render_empty(&self) -> String {
        match self.mode {
            OutputMode::Plain => "No routes registered.".to_string(),
            OutputMode::Minimal | OutputMode::Rich => {
                let muted = self.theme.muted.to_ansi_fg();
                format!("{muted}No routes registered.{ANSI_RESET}")
            }
        }
    }

    fn render_plain(&self, routes: &[RouteEntry]) -> String {
        let mut lines = Vec::new();

        // Title
        if let Some(title) = &self.config.title {
            lines.push(title.clone());
            lines.push("-".repeat(title.len()));
        }

        // Calculate column widths
        let method_width = routes.iter().map(|r| r.method.len()).max().unwrap_or(6);
        let path_width = routes.iter().map(|r| r.path.len()).max().unwrap_or(10);

        // Header
        let mut header = format!("{:width$}  Path", "Method", width = method_width);
        if self.config.show_handlers {
            header.push_str("  Handler");
        }
        if self.config.show_tags {
            header.push_str("  Tags");
        }
        lines.push(header);

        // Routes
        for route in routes {
            let mut line = format!(
                "{:width$}  {}",
                route.method,
                route.path,
                width = method_width
            );

            if self.config.show_handlers {
                if let Some(handler) = &route.handler {
                    // Pad path column
                    let padding = path_width.saturating_sub(route.path.len());
                    line.push_str(&" ".repeat(padding));
                    line.push_str("  ");
                    line.push_str(handler);
                }
            }

            if self.config.show_tags && !route.tags.is_empty() {
                line.push_str("  [");
                line.push_str(&route.tags.join(", "));
                line.push(']');
            }

            if route.deprecated {
                line.push_str(" (deprecated)");
            }

            lines.push(line);
        }

        // Summary
        lines.push(String::new());
        lines.push(format!("Total: {} route(s)", routes.len()));

        lines.join("\n")
    }

    fn render_minimal(&self, routes: &[RouteEntry]) -> String {
        let mut lines = Vec::new();
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();

        // Title
        if let Some(title) = &self.config.title {
            lines.push(format!("{accent}{title}{ANSI_RESET}"));
            lines.push(format!("{muted}{}{ANSI_RESET}", "-".repeat(title.len())));
        }

        // Routes
        for route in routes {
            let method_color = self.method_color(&route.method).to_ansi_fg();

            let mut line = format!(
                "{method_color}{:7}{ANSI_RESET} {}",
                route.method, route.path
            );

            if self.config.show_tags && !route.tags.is_empty() {
                let _ = write!(line, " {muted}[{}]{ANSI_RESET}", route.tags.join(", "));
            }

            if route.deprecated {
                let warning = self.theme.warning.to_ansi_fg();
                let _ = write!(line, " {warning}(deprecated){ANSI_RESET}");
            }

            lines.push(line);
        }

        // Summary
        lines.push(String::new());
        lines.push(format!(
            "{muted}Total: {} route(s){ANSI_RESET}",
            routes.len()
        ));

        lines.join("\n")
    }

    fn render_rich(&self, routes: &[RouteEntry]) -> String {
        let mut lines = Vec::new();
        let muted = self.theme.muted.to_ansi_fg();
        let border = self.theme.border.to_ansi_fg();
        let header_color = self.theme.header.to_ansi_fg();

        // Calculate widths
        let method_width = routes
            .iter()
            .map(|r| r.method.len())
            .max()
            .unwrap_or(6)
            .max(7);
        let path_width = routes
            .iter()
            .map(|r| r.path.len())
            .max()
            .unwrap_or(10)
            .max(20);

        // Top border
        let table_width = method_width + path_width + 10;
        lines.push(format!("{border}┌{}┐{ANSI_RESET}", "─".repeat(table_width)));

        // Title
        if let Some(title) = &self.config.title {
            let title_pad = (table_width - title.len()) / 2;
            lines.push(format!(
                "{border}│{ANSI_RESET}{}{header_color}{ANSI_BOLD}{}{ANSI_RESET}{}{border}│{ANSI_RESET}",
                " ".repeat(title_pad),
                title,
                " ".repeat(table_width - title_pad - title.len())
            ));
            lines.push(format!("{border}├{}┤{ANSI_RESET}", "─".repeat(table_width)));
        }

        // Header row
        lines.push(format!(
            "{border}│{ANSI_RESET} {header_color}{:width$}{ANSI_RESET}  {header_color}{:pwidth$}{ANSI_RESET} {border}│{ANSI_RESET}",
            "Method",
            "Path",
            width = method_width,
            pwidth = path_width + 4
        ));

        lines.push(format!("{border}├{}┤{ANSI_RESET}", "─".repeat(table_width)));

        // Routes
        for route in routes {
            let method_bg = self.method_color(&route.method).to_ansi_bg();

            // Format path with tags
            let mut path_display = route.path.clone();
            if self.config.show_tags && !route.tags.is_empty() {
                use std::fmt::Write;
                let _ = write!(path_display, " [{}]", route.tags.join(", "));
            }

            // Truncate if too long
            if path_display.len() > path_width + 4 {
                path_display = format!("{}...", &path_display[..=path_width]);
            }

            let deprecated_marker = if route.deprecated {
                let warning = self.theme.warning.to_ansi_fg();
                format!(" {warning}⚠{ANSI_RESET}")
            } else {
                String::new()
            };

            lines.push(format!(
                "{border}│{ANSI_RESET} {method_bg}{ANSI_BOLD} {:width$} {ANSI_RESET}  {}{}{} {border}│{ANSI_RESET}",
                route.method,
                path_display,
                deprecated_marker,
                " ".repeat((path_width + 4).saturating_sub(path_display.len() + deprecated_marker.len() / 10)),
                width = method_width
            ));
        }

        // Bottom border
        lines.push(format!("{border}└{}┘{ANSI_RESET}", "─".repeat(table_width)));

        // Summary
        let success = self.theme.success.to_ansi_fg();
        lines.push(format!(
            "{success}✓{ANSI_RESET} {muted}Total: {} route(s) registered{ANSI_RESET}",
            routes.len()
        ));

        lines.join("\n")
    }

    fn method_color(&self, method: &str) -> crate::themes::Color {
        match method.to_uppercase().as_str() {
            "GET" => self.theme.http_get,
            "POST" => self.theme.http_post,
            "PUT" => self.theme.http_put,
            "DELETE" => self.theme.http_delete,
            "PATCH" => self.theme.http_patch,
            "OPTIONS" => self.theme.http_options,
            "HEAD" => self.theme.http_head,
            _ => self.theme.muted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_routes() -> Vec<RouteEntry> {
        vec![
            RouteEntry::new("GET", "/api/users")
                .handler("list_users")
                .tag("users"),
            RouteEntry::new("POST", "/api/users")
                .handler("create_user")
                .tag("users"),
            RouteEntry::new("GET", "/api/users/{id}")
                .handler("get_user")
                .tag("users"),
            RouteEntry::new("DELETE", "/api/users/{id}")
                .handler("delete_user")
                .tag("users")
                .deprecated(true),
        ]
    }

    #[test]
    fn test_route_entry_builder() {
        let route = RouteEntry::new("POST", "/api/items")
            .handler("create_item")
            .tags(["items", "v2"])
            .deprecated(false);

        assert_eq!(route.method, "POST");
        assert_eq!(route.path, "/api/items");
        assert_eq!(route.handler, Some("create_item".to_string()));
        assert_eq!(route.tags, vec!["items", "v2"]);
        assert!(!route.deprecated);
    }

    #[test]
    fn test_route_display_plain() {
        let display = RouteDisplay::new(OutputMode::Plain);
        let routes = sample_routes();

        let output = display.render(&routes);

        assert!(output.contains("Registered Routes"));
        assert!(output.contains("GET"));
        assert!(output.contains("POST"));
        assert!(output.contains("/api/users"));
        assert!(output.contains("list_users"));
        assert!(output.contains("4 route(s)"));
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn test_route_display_empty() {
        let display = RouteDisplay::new(OutputMode::Plain);

        let output = display.render(&[]);

        assert!(output.contains("No routes registered"));
    }

    #[test]
    fn test_route_display_rich_has_ansi() {
        let display = RouteDisplay::new(OutputMode::Rich);
        let routes = sample_routes();

        let output = display.render(&routes);

        assert!(output.contains("\x1b["));
        assert!(output.contains("GET"));
        assert!(output.contains("/api/users"));
    }

    #[test]
    fn test_route_display_hide_deprecated() {
        let config = RouteTableConfig {
            show_deprecated: false,
            ..Default::default()
        };
        let display = RouteDisplay::with_config(OutputMode::Plain, config);
        let routes = sample_routes();

        let output = display.render(&routes);

        assert!(output.contains("3 route(s)")); // One deprecated route hidden
        assert!(!output.contains("deprecated"));
    }

    #[test]
    fn test_route_display_no_handlers() {
        let config = RouteTableConfig {
            show_handlers: false,
            ..Default::default()
        };
        let display = RouteDisplay::with_config(OutputMode::Plain, config);
        let routes = sample_routes();

        let output = display.render(&routes);

        assert!(!output.contains("list_users"));
    }

    #[test]
    fn test_route_display_no_tags() {
        let config = RouteTableConfig {
            show_tags: false,
            ..Default::default()
        };
        let display = RouteDisplay::with_config(OutputMode::Plain, config);
        let routes = sample_routes();

        let output = display.render(&routes);

        assert!(!output.contains("[users]"));
    }

    #[test]
    fn test_route_display_custom_title() {
        let config = RouteTableConfig {
            title: Some("API Endpoints".to_string()),
            ..Default::default()
        };
        let display = RouteDisplay::with_config(OutputMode::Plain, config);
        let routes = sample_routes();

        let output = display.render(&routes);

        assert!(output.contains("API Endpoints"));
    }
}
