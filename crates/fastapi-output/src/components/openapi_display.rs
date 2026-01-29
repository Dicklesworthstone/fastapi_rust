//! OpenAPI schema display component.
//!
//! Provides visual representation of OpenAPI specifications including
//! endpoint tables, schema visualization, and documentation display.
//!
//! # Features
//!
//! - Endpoint summary table with method coloring
//! - Schema type visualization (objects, arrays, enums)
//! - Request/response body display
//! - Authentication requirements display

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";

/// An OpenAPI endpoint for display.
#[derive(Debug, Clone)]
pub struct EndpointInfo {
    /// HTTP method.
    pub method: String,
    /// Path pattern.
    pub path: String,
    /// Operation summary.
    pub summary: Option<String>,
    /// Operation description.
    pub description: Option<String>,
    /// Tags for grouping.
    pub tags: Vec<String>,
    /// Whether deprecated.
    pub deprecated: bool,
    /// Security requirements.
    pub security: Vec<String>,
    /// Operation ID.
    pub operation_id: Option<String>,
}

impl EndpointInfo {
    /// Create a new endpoint info.
    #[must_use]
    pub fn new(method: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            summary: None,
            description: None,
            tags: Vec::new(),
            deprecated: false,
            security: Vec::new(),
            operation_id: None,
        }
    }

    /// Set the summary.
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Set the description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add a tag.
    #[must_use]
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Mark as deprecated.
    #[must_use]
    pub fn deprecated(mut self, deprecated: bool) -> Self {
        self.deprecated = deprecated;
        self
    }

    /// Add a security requirement.
    #[must_use]
    pub fn security(mut self, security: impl Into<String>) -> Self {
        self.security.push(security.into());
        self
    }

    /// Set the operation ID.
    #[must_use]
    pub fn operation_id(mut self, id: impl Into<String>) -> Self {
        self.operation_id = Some(id.into());
        self
    }
}

/// Schema type for display.
#[derive(Debug, Clone)]
pub enum SchemaType {
    /// String type.
    String {
        /// Format (e.g., "email", "date-time").
        format: Option<String>,
        /// Enum values if constrained.
        enum_values: Vec<String>,
    },
    /// Integer type.
    Integer {
        /// Format (e.g., "int32", "int64").
        format: Option<String>,
        /// Minimum value.
        minimum: Option<i64>,
        /// Maximum value.
        maximum: Option<i64>,
    },
    /// Number type.
    Number {
        /// Format (e.g., "float", "double").
        format: Option<String>,
    },
    /// Boolean type.
    Boolean,
    /// Array type.
    Array {
        /// Item schema.
        items: Box<SchemaType>,
    },
    /// Object type.
    Object {
        /// Properties.
        properties: Vec<PropertyInfo>,
        /// Required property names.
        required: Vec<String>,
    },
    /// Reference to another schema.
    Ref {
        /// Reference name.
        name: String,
    },
    /// Any of (union type).
    AnyOf {
        /// Options.
        options: Vec<SchemaType>,
    },
    /// Null type.
    Null,
}

impl SchemaType {
    /// Get a short type description.
    #[must_use]
    pub fn short_description(&self) -> String {
        match self {
            Self::String {
                format,
                enum_values,
            } => {
                if !enum_values.is_empty() {
                    format!("enum[{}]", enum_values.len())
                } else if let Some(fmt) = format {
                    format!("string<{fmt}>")
                } else {
                    "string".to_string()
                }
            }
            Self::Integer { format, .. } => {
                if let Some(fmt) = format {
                    format!("integer<{fmt}>")
                } else {
                    "integer".to_string()
                }
            }
            Self::Number { format } => {
                if let Some(fmt) = format {
                    format!("number<{fmt}>")
                } else {
                    "number".to_string()
                }
            }
            Self::Boolean => "boolean".to_string(),
            Self::Array { items } => format!("array[{}]", items.short_description()),
            Self::Object { properties, .. } => format!("object{{{}}}", properties.len()),
            Self::Ref { name } => format!("${name}"),
            Self::AnyOf { options } => {
                let types: Vec<_> = options.iter().map(SchemaType::short_description).collect();
                types.join(" | ")
            }
            Self::Null => "null".to_string(),
        }
    }
}

/// Property information for object schemas.
#[derive(Debug, Clone)]
pub struct PropertyInfo {
    /// Property name.
    pub name: String,
    /// Property type.
    pub schema: SchemaType,
    /// Description.
    pub description: Option<String>,
    /// Whether required.
    pub required: bool,
    /// Default value.
    pub default: Option<String>,
    /// Example value.
    pub example: Option<String>,
}

impl PropertyInfo {
    /// Create a new property info.
    #[must_use]
    pub fn new(name: impl Into<String>, schema: SchemaType) -> Self {
        Self {
            name: name.into(),
            schema,
            description: None,
            required: false,
            default: None,
            example: None,
        }
    }

    /// Set the description.
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Mark as required.
    #[must_use]
    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// Set the default value.
    #[must_use]
    pub fn default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }

    /// Set the example value.
    #[must_use]
    pub fn example(mut self, example: impl Into<String>) -> Self {
        self.example = Some(example.into());
        self
    }
}

/// OpenAPI spec summary for display.
#[derive(Debug, Clone)]
pub struct OpenApiSummary {
    /// API title.
    pub title: String,
    /// API version.
    pub version: String,
    /// API description.
    pub description: Option<String>,
    /// Server URLs.
    pub servers: Vec<String>,
    /// Endpoints.
    pub endpoints: Vec<EndpointInfo>,
    /// Total endpoint count.
    pub endpoint_count: usize,
}

impl OpenApiSummary {
    /// Create a new OpenAPI summary.
    #[must_use]
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            description: None,
            servers: Vec::new(),
            endpoints: Vec::new(),
            endpoint_count: 0,
        }
    }

    /// Set the description.
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add a server URL.
    #[must_use]
    pub fn server(mut self, url: impl Into<String>) -> Self {
        self.servers.push(url.into());
        self
    }

    /// Add an endpoint.
    #[must_use]
    pub fn endpoint(mut self, endpoint: EndpointInfo) -> Self {
        self.endpoint_count += 1;
        self.endpoints.push(endpoint);
        self
    }
}

/// Configuration for OpenAPI display.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct OpenApiDisplayConfig {
    /// Show endpoint descriptions.
    pub show_descriptions: bool,
    /// Show security requirements.
    pub show_security: bool,
    /// Show deprecated endpoints.
    pub show_deprecated: bool,
    /// Group by tags.
    pub group_by_tags: bool,
    /// Maximum endpoints to show (0 = unlimited).
    pub max_endpoints: usize,
}

impl Default for OpenApiDisplayConfig {
    fn default() -> Self {
        Self {
            show_descriptions: false,
            show_security: true,
            show_deprecated: true,
            group_by_tags: false,
            max_endpoints: 0,
        }
    }
}

/// OpenAPI endpoint table display.
#[derive(Debug, Clone)]
pub struct OpenApiDisplay {
    mode: OutputMode,
    theme: FastApiTheme,
    config: OpenApiDisplayConfig,
}

impl OpenApiDisplay {
    /// Create a new OpenAPI display.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            config: OpenApiDisplayConfig::default(),
        }
    }

    /// Create with custom configuration.
    #[must_use]
    pub fn with_config(mode: OutputMode, config: OpenApiDisplayConfig) -> Self {
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

    /// Group endpoints by their first tag.
    fn group_endpoints_by_tag<'a>(
        &self,
        endpoints: &'a [EndpointInfo],
    ) -> Vec<(String, Vec<&'a EndpointInfo>)> {
        use std::collections::BTreeMap;

        let mut groups: BTreeMap<String, Vec<&'a EndpointInfo>> = BTreeMap::new();

        for endpoint in endpoints {
            let tag = endpoint
                .tags
                .first()
                .cloned()
                .unwrap_or_else(|| "Other".to_string());
            groups.entry(tag).or_default().push(endpoint);
        }

        groups.into_iter().collect()
    }

    /// Render an OpenAPI summary.
    #[must_use]
    pub fn render_summary(&self, summary: &OpenApiSummary) -> String {
        match self.mode {
            OutputMode::Plain => self.render_summary_plain(summary),
            OutputMode::Minimal => self.render_summary_minimal(summary),
            OutputMode::Rich => self.render_summary_rich(summary),
        }
    }

    fn render_summary_plain(&self, summary: &OpenApiSummary) -> String {
        let mut lines = Vec::new();

        // Header
        lines.push(format!("{} v{}", summary.title, summary.version));
        lines.push("=".repeat(summary.title.len() + summary.version.len() + 2));

        if let Some(desc) = &summary.description {
            lines.push(desc.clone());
        }

        // Servers
        if !summary.servers.is_empty() {
            lines.push(String::new());
            lines.push("Servers:".to_string());
            for server in &summary.servers {
                lines.push(format!("  - {server}"));
            }
        }

        // Endpoints table
        lines.push(String::new());
        lines.push(format!("Endpoints ({}):", summary.endpoint_count));
        lines.push(String::new());

        // Calculate column widths
        let method_width = summary
            .endpoints
            .iter()
            .map(|e| e.method.len())
            .max()
            .unwrap_or(6)
            .max(6);
        let path_width = summary
            .endpoints
            .iter()
            .map(|e| e.path.len())
            .max()
            .unwrap_or(10)
            .min(40);

        // Header row
        lines.push(format!(
            "{:width$}  {:pwidth$}  Summary",
            "Method",
            "Path",
            width = method_width,
            pwidth = path_width
        ));
        lines.push("-".repeat(method_width + path_width + 30));

        // Endpoint rows
        let endpoints = if self.config.max_endpoints > 0 {
            summary.endpoints.iter().take(self.config.max_endpoints)
        } else {
            summary.endpoints.iter().take(usize::MAX)
        };

        for endpoint in endpoints {
            if !self.config.show_deprecated && endpoint.deprecated {
                continue;
            }

            let path = if endpoint.path.len() > path_width {
                format!("{}...", &endpoint.path[..path_width - 3])
            } else {
                endpoint.path.clone()
            };

            let summary_text = endpoint.summary.as_deref().unwrap_or("-");

            // Build indicators
            let mut indicators = Vec::new();
            if !endpoint.security.is_empty() {
                indicators.push("[auth]");
            }
            if endpoint.deprecated {
                indicators.push("[deprecated]");
            }
            let indicator_str = if indicators.is_empty() {
                String::new()
            } else {
                format!(" {}", indicators.join(" "))
            };

            lines.push(format!(
                "{:width$}  {:pwidth$}  {summary_text}{indicator_str}",
                endpoint.method,
                path,
                width = method_width,
                pwidth = path_width
            ));
        }

        if self.config.max_endpoints > 0 && summary.endpoint_count > self.config.max_endpoints {
            lines.push(format!(
                "... and {} more",
                summary.endpoint_count - self.config.max_endpoints
            ));
        }

        // Legend
        let has_auth = summary.endpoints.iter().any(|e| !e.security.is_empty());
        let has_deprecated = summary.endpoints.iter().any(|e| e.deprecated);

        if has_auth || has_deprecated {
            lines.push(String::new());
            lines.push("Legend:".to_string());
            if has_auth {
                lines.push("  [auth] = Authentication required".to_string());
            }
            if has_deprecated {
                lines.push("  [deprecated] = Endpoint is deprecated".to_string());
            }
        }

        lines.join("\n")
    }

    fn render_summary_minimal(&self, summary: &OpenApiSummary) -> String {
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let header = self.theme.header.to_ansi_fg();

        let mut lines = Vec::new();

        // Header
        lines.push(format!(
            "{header}{ANSI_BOLD}{}{ANSI_RESET} {muted}v{}{ANSI_RESET}",
            summary.title, summary.version
        ));

        if let Some(desc) = &summary.description {
            lines.push(format!("{muted}{desc}{ANSI_RESET}"));
        }

        // Endpoints
        lines.push(String::new());
        lines.push(format!(
            "{header}Endpoints{ANSI_RESET} {muted}({}){ANSI_RESET}",
            summary.endpoint_count
        ));

        let endpoints = if self.config.max_endpoints > 0 {
            summary.endpoints.iter().take(self.config.max_endpoints)
        } else {
            summary.endpoints.iter().take(usize::MAX)
        };

        for endpoint in endpoints {
            if !self.config.show_deprecated && endpoint.deprecated {
                continue;
            }

            let method_color = self.method_color(&endpoint.method).to_ansi_fg();

            // Build indicators
            let mut indicators = Vec::new();
            if !endpoint.security.is_empty() {
                indicators.push("🔒");
            }
            if endpoint.deprecated {
                indicators.push("⚠");
            }
            let indicator_str = if indicators.is_empty() {
                String::new()
            } else {
                format!(" {}", indicators.join(" "))
            };

            let summary_text = endpoint
                .summary
                .as_ref()
                .map(|s| format!(" {muted}- {s}{ANSI_RESET}"))
                .unwrap_or_default();

            lines.push(format!(
                "  {method_color}{:7}{ANSI_RESET} {accent}{}{ANSI_RESET}{summary_text}{indicator_str}",
                endpoint.method, endpoint.path
            ));
        }

        // Legend
        let has_auth = summary.endpoints.iter().any(|e| !e.security.is_empty());
        let has_deprecated = summary.endpoints.iter().any(|e| e.deprecated);

        if has_auth || has_deprecated {
            lines.push(String::new());
            if has_auth {
                lines.push(format!("{muted}🔒 = Auth required{ANSI_RESET}"));
            }
            if has_deprecated {
                lines.push(format!("{muted}⚠  = Deprecated{ANSI_RESET}"));
            }
        }

        lines.join("\n")
    }

    fn render_summary_rich(&self, summary: &OpenApiSummary) -> String {
        let muted = self.theme.muted.to_ansi_fg();
        let _accent = self.theme.accent.to_ansi_fg();
        let border = self.theme.border.to_ansi_fg();
        let header_style = self.theme.header.to_ansi_fg();
        let success = self.theme.success.to_ansi_fg();

        let mut lines = Vec::new();

        // Calculate table width
        let path_width = summary
            .endpoints
            .iter()
            .map(|e| e.path.len())
            .max()
            .unwrap_or(10)
            .min(35);
        let summary_width = 25;
        let table_width = 9 + path_width + summary_width + 4; // method(9) + path + summary + borders

        // Top border with title
        lines.push(format!("{border}┌{}┐{ANSI_RESET}", "─".repeat(table_width)));

        // Title row
        let title_text = format!("{} v{}", summary.title, summary.version);
        let title_pad = (table_width - title_text.len()) / 2;
        lines.push(format!(
            "{border}│{ANSI_RESET}{}{header_style}{ANSI_BOLD}{title_text}{ANSI_RESET}{}{border}│{ANSI_RESET}",
            " ".repeat(title_pad),
            " ".repeat(table_width - title_pad - title_text.len())
        ));

        // Description if present
        if let Some(desc) = &summary.description {
            let desc_truncated = if desc.len() > table_width - 4 {
                format!("{}...", &desc[..table_width - 7])
            } else {
                desc.clone()
            };
            lines.push(format!(
                "{border}│{ANSI_RESET} {muted}{:width$}{ANSI_RESET} {border}│{ANSI_RESET}",
                desc_truncated,
                width = table_width - 2
            ));
        }

        lines.push(format!("{border}├{}┤{ANSI_RESET}", "─".repeat(table_width)));

        // Column headers
        lines.push(format!(
            "{border}│{ANSI_RESET} {header_style}{:7}{ANSI_RESET}  {header_style}{:pwidth$}{ANSI_RESET}  {header_style}{:swidth$}{ANSI_RESET} {border}│{ANSI_RESET}",
            "Method",
            "Path",
            "Summary",
            pwidth = path_width,
            swidth = summary_width
        ));
        lines.push(format!("{border}├{}┤{ANSI_RESET}", "─".repeat(table_width)));

        // Endpoint rows
        let endpoints = if self.config.max_endpoints > 0 {
            summary.endpoints.iter().take(self.config.max_endpoints)
        } else {
            summary.endpoints.iter().take(usize::MAX)
        };

        for endpoint in endpoints {
            if !self.config.show_deprecated && endpoint.deprecated {
                continue;
            }

            let method_bg = self.method_color(&endpoint.method).to_ansi_bg();

            let path = if endpoint.path.len() > path_width {
                format!("{}...", &endpoint.path[..path_width - 3])
            } else {
                endpoint.path.clone()
            };

            let summary_text = endpoint.summary.as_ref().map_or_else(
                || "-".to_string(),
                |s| {
                    if s.len() > summary_width {
                        format!("{}...", &s[..summary_width - 3])
                    } else {
                        s.clone()
                    }
                },
            );

            // Build indicators
            let mut indicators = Vec::new();
            if !endpoint.security.is_empty() {
                indicators.push("🔒");
            }
            if endpoint.deprecated {
                indicators.push("⚠");
            }
            let indicator_str = if indicators.is_empty() {
                String::new()
            } else {
                format!(" {muted}{}{ANSI_RESET}", indicators.join(" "))
            };

            lines.push(format!(
                "{border}│{ANSI_RESET} {method_bg}{ANSI_BOLD} {:5} {ANSI_RESET}  {:pwidth$}{indicator_str}  {muted}{:swidth$}{ANSI_RESET} {border}│{ANSI_RESET}",
                endpoint.method,
                path,
                summary_text,
                pwidth = path_width,
                swidth = summary_width
            ));
        }

        // Bottom border
        lines.push(format!("{border}└{}┘{ANSI_RESET}", "─".repeat(table_width)));

        // Legend
        let has_auth = summary.endpoints.iter().any(|e| !e.security.is_empty());
        let has_deprecated = summary.endpoints.iter().any(|e| e.deprecated);

        if has_auth || has_deprecated {
            lines.push(String::new());
            if has_auth {
                lines.push(format!("{muted}  🔒 = Authentication required{ANSI_RESET}"));
            }
            if has_deprecated {
                lines.push(format!("{muted}  ⚠  = Deprecated{ANSI_RESET}"));
            }
        }

        lines.push(String::new());

        // Summary line
        lines.push(format!(
            "{success}✓{ANSI_RESET} {muted}{} endpoint(s) documented{ANSI_RESET}",
            summary.endpoint_count
        ));

        lines.join("\n")
    }

    /// Render a schema type.
    #[must_use]
    pub fn render_schema(&self, schema: &SchemaType, title: Option<&str>) -> String {
        match self.mode {
            OutputMode::Plain => self.render_schema_plain(schema, title, 0),
            OutputMode::Minimal => self.render_schema_minimal(schema, title, 0),
            OutputMode::Rich => self.render_schema_rich(schema, title),
        }
    }

    #[allow(clippy::self_only_used_in_recursion)]
    fn render_schema_plain(
        &self,
        schema: &SchemaType,
        title: Option<&str>,
        indent: usize,
    ) -> String {
        let mut lines = Vec::new();
        let prefix = " ".repeat(indent);

        if let Some(t) = title {
            lines.push(format!("{prefix}{t}:"));
        }

        match schema {
            SchemaType::Object {
                properties,
                required,
            } => {
                lines.push(format!("{prefix}{{"));
                for prop in properties {
                    let required_marker = if prop.required || required.contains(&prop.name) {
                        " (required)"
                    } else {
                        ""
                    };
                    let type_desc = prop.schema.short_description();
                    lines.push(format!(
                        "{prefix}  \"{}\": {type_desc}{required_marker}",
                        prop.name
                    ));
                }
                lines.push(format!("{prefix}}}"));
            }
            SchemaType::Array { items } => {
                lines.push(format!("{prefix}["));
                lines.push(self.render_schema_plain(items, None, indent + 2));
                lines.push(format!("{prefix}]"));
            }
            SchemaType::String { enum_values, .. } if !enum_values.is_empty() => {
                lines.push(format!("{prefix}enum: [{}]", enum_values.join(", ")));
            }
            _ => {
                lines.push(format!("{prefix}{}", schema.short_description()));
            }
        }

        lines.join("\n")
    }

    fn render_schema_minimal(
        &self,
        schema: &SchemaType,
        title: Option<&str>,
        indent: usize,
    ) -> String {
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let info = self.theme.info.to_ansi_fg();

        let mut lines = Vec::new();
        let prefix = " ".repeat(indent);

        if let Some(t) = title {
            lines.push(format!("{prefix}{accent}{t}:{ANSI_RESET}"));
        }

        match schema {
            SchemaType::Object {
                properties,
                required,
            } => {
                lines.push(format!("{prefix}{muted}{{{ANSI_RESET}"));
                for prop in properties {
                    let required_marker = if prop.required || required.contains(&prop.name) {
                        format!(" {info}*{ANSI_RESET}")
                    } else {
                        String::new()
                    };
                    let type_desc = prop.schema.short_description();
                    lines.push(format!(
                        "{prefix}  {accent}\"{}\"{ANSI_RESET}: {muted}{type_desc}{ANSI_RESET}{required_marker}",
                        prop.name
                    ));
                }
                lines.push(format!("{prefix}{muted}}}{ANSI_RESET}"));
            }
            SchemaType::Array { items } => {
                lines.push(format!("{prefix}{muted}[{ANSI_RESET}"));
                lines.push(self.render_schema_minimal(items, None, indent + 2));
                lines.push(format!("{prefix}{muted}]{ANSI_RESET}"));
            }
            _ => {
                lines.push(format!(
                    "{prefix}{info}{}{ANSI_RESET}",
                    schema.short_description()
                ));
            }
        }

        lines.join("\n")
    }

    fn render_schema_rich(&self, schema: &SchemaType, title: Option<&str>) -> String {
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let border = self.theme.border.to_ansi_fg();
        let header_style = self.theme.header.to_ansi_fg();
        let info = self.theme.info.to_ansi_fg();
        let warning = self.theme.warning.to_ansi_fg();

        let mut lines = Vec::new();

        // Box border
        let width = 45;
        lines.push(format!("{border}┌{}┐{ANSI_RESET}", "─".repeat(width)));

        if let Some(t) = title {
            lines.push(format!(
                "{border}│{ANSI_RESET} {header_style}{ANSI_BOLD}{t}{ANSI_RESET}{}",
                " ".repeat(width - t.len() - 1)
            ));
            lines.push(format!("{border}├{}┤{ANSI_RESET}", "─".repeat(width)));
        }

        match schema {
            SchemaType::Object {
                properties,
                required,
            } => {
                for prop in properties {
                    let required_marker = if prop.required || required.contains(&prop.name) {
                        format!(" {warning}*{ANSI_RESET}")
                    } else {
                        String::new()
                    };
                    let type_desc = prop.schema.short_description();

                    lines.push(format!(
                        "{border}│{ANSI_RESET}  {accent}\"{}\"{ANSI_RESET}: {info}{type_desc}{ANSI_RESET}{required_marker}",
                        prop.name
                    ));

                    if let Some(desc) = &prop.description {
                        let desc_truncated = if desc.len() > width - 6 {
                            format!("{}...", &desc[..width - 9])
                        } else {
                            desc.clone()
                        };
                        lines.push(format!(
                            "{border}│{ANSI_RESET}    {muted}{desc_truncated}{ANSI_RESET}"
                        ));
                    }
                }
            }
            SchemaType::Array { items } => {
                lines.push(format!(
                    "{border}│{ANSI_RESET}  {muted}Array of:{ANSI_RESET} {info}{}{ANSI_RESET}",
                    items.short_description()
                ));
            }
            SchemaType::String { enum_values, .. } if !enum_values.is_empty() => {
                lines.push(format!(
                    "{border}│{ANSI_RESET}  {muted}Enum values:{ANSI_RESET}"
                ));
                for val in enum_values {
                    lines.push(format!(
                        "{border}│{ANSI_RESET}    {accent}• {val}{ANSI_RESET}"
                    ));
                }
            }
            _ => {
                lines.push(format!(
                    "{border}│{ANSI_RESET}  {info}{}{ANSI_RESET}",
                    schema.short_description()
                ));
            }
        }

        lines.push(format!("{border}└{}┘{ANSI_RESET}", "─".repeat(width)));

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

    fn sample_summary() -> OpenApiSummary {
        OpenApiSummary::new("My API", "1.0.0")
            .description("A sample REST API")
            .server("https://api.example.com")
            .endpoint(
                EndpointInfo::new("GET", "/users")
                    .summary("List all users")
                    .tag("users"),
            )
            .endpoint(
                EndpointInfo::new("POST", "/users")
                    .summary("Create a new user")
                    .tag("users"),
            )
            .endpoint(
                EndpointInfo::new("GET", "/users/{id}")
                    .summary("Get user by ID")
                    .tag("users"),
            )
            .endpoint(
                EndpointInfo::new("DELETE", "/users/{id}")
                    .summary("Delete user")
                    .tag("users")
                    .deprecated(true),
            )
    }

    fn sample_schema() -> SchemaType {
        SchemaType::Object {
            properties: vec![
                PropertyInfo::new(
                    "id",
                    SchemaType::Integer {
                        format: Some("int64".to_string()),
                        minimum: None,
                        maximum: None,
                    },
                )
                .description("Unique identifier")
                .required(true),
                PropertyInfo::new(
                    "name",
                    SchemaType::String {
                        format: None,
                        enum_values: vec![],
                    },
                )
                .description("User's full name")
                .required(true),
                PropertyInfo::new(
                    "email",
                    SchemaType::String {
                        format: Some("email".to_string()),
                        enum_values: vec![],
                    },
                )
                .description("Email address"),
                PropertyInfo::new(
                    "status",
                    SchemaType::String {
                        format: None,
                        enum_values: vec![
                            "active".to_string(),
                            "inactive".to_string(),
                            "pending".to_string(),
                        ],
                    },
                )
                .default("pending"),
            ],
            required: vec!["id".to_string(), "name".to_string()],
        }
    }

    #[test]
    fn test_endpoint_info_builder() {
        let endpoint = EndpointInfo::new("POST", "/users")
            .summary("Create user")
            .tag("users")
            .security("bearer")
            .deprecated(false);

        assert_eq!(endpoint.method, "POST");
        assert_eq!(endpoint.path, "/users");
        assert_eq!(endpoint.summary, Some("Create user".to_string()));
    }

    #[test]
    fn test_schema_type_description() {
        assert_eq!(SchemaType::Boolean.short_description(), "boolean");
        assert_eq!(
            SchemaType::String {
                format: Some("email".to_string()),
                enum_values: vec![]
            }
            .short_description(),
            "string<email>"
        );
        assert_eq!(
            SchemaType::Array {
                items: Box::new(SchemaType::Boolean)
            }
            .short_description(),
            "array[boolean]"
        );
    }

    #[test]
    fn test_openapi_display_plain() {
        let display = OpenApiDisplay::new(OutputMode::Plain);
        let output = display.render_summary(&sample_summary());

        assert!(output.contains("My API"));
        assert!(output.contains("v1.0.0"));
        assert!(output.contains("GET"));
        assert!(output.contains("/users"));
        assert!(output.contains("List all users"));
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn test_openapi_display_rich_has_ansi() {
        let display = OpenApiDisplay::new(OutputMode::Rich);
        let output = display.render_summary(&sample_summary());

        assert!(output.contains("\x1b["));
        assert!(output.contains("My API"));
    }

    #[test]
    fn test_schema_display_plain() {
        let display = OpenApiDisplay::new(OutputMode::Plain);
        let output = display.render_schema(&sample_schema(), Some("User"));

        assert!(output.contains("User:"));
        assert!(output.contains("\"id\""));
        assert!(output.contains("\"name\""));
        assert!(output.contains("required"));
    }

    #[test]
    fn test_schema_display_rich() {
        let display = OpenApiDisplay::new(OutputMode::Rich);
        let output = display.render_schema(&sample_schema(), Some("User"));

        assert!(output.contains("\x1b["));
        assert!(output.contains("id"));
    }

    #[test]
    fn test_max_endpoints_config() {
        let config = OpenApiDisplayConfig {
            max_endpoints: 2,
            ..Default::default()
        };
        let display = OpenApiDisplay::with_config(OutputMode::Plain, config);
        let output = display.render_summary(&sample_summary());

        assert!(output.contains("and 2 more"));
    }

    #[test]
    fn test_auth_indicator_plain() {
        let summary = OpenApiSummary::new("Auth API", "1.0.0")
            .endpoint(EndpointInfo::new("GET", "/public").summary("Public endpoint"))
            .endpoint(
                EndpointInfo::new("POST", "/protected")
                    .summary("Protected endpoint")
                    .security("bearer"),
            );

        let display = OpenApiDisplay::new(OutputMode::Plain);
        let output = display.render_summary(&summary);

        assert!(output.contains("[auth]"), "Should show [auth] indicator");
        assert!(output.contains("Legend:"), "Should show legend");
        assert!(output.contains("Authentication required"));
    }

    #[test]
    fn test_auth_indicator_rich() {
        let summary = OpenApiSummary::new("Auth API", "1.0.0").endpoint(
            EndpointInfo::new("POST", "/protected")
                .summary("Protected endpoint")
                .security("bearer"),
        );

        let display = OpenApiDisplay::new(OutputMode::Rich);
        let output = display.render_summary(&summary);

        assert!(output.contains("🔒"), "Should show lock indicator");
        assert!(output.contains("Authentication required"));
    }

    #[test]
    fn test_deprecated_indicator_rich() {
        let summary = OpenApiSummary::new("API", "1.0.0").endpoint(
            EndpointInfo::new("GET", "/old")
                .summary("Old endpoint")
                .deprecated(true),
        );

        let display = OpenApiDisplay::new(OutputMode::Rich);
        let output = display.render_summary(&summary);

        assert!(output.contains("⚠"), "Should show deprecated indicator");
        assert!(output.contains("Deprecated"));
    }

    #[test]
    fn test_combined_indicators() {
        let summary = OpenApiSummary::new("API", "1.0.0").endpoint(
            EndpointInfo::new("POST", "/old-protected")
                .summary("Old protected endpoint")
                .security("bearer")
                .deprecated(true),
        );

        let display = OpenApiDisplay::new(OutputMode::Rich);
        let output = display.render_summary(&summary);

        assert!(output.contains("🔒"), "Should show lock indicator");
        assert!(output.contains("⚠"), "Should show deprecated indicator");
    }

    #[test]
    fn test_no_legend_when_no_indicators() {
        let summary = OpenApiSummary::new("Simple API", "1.0.0")
            .endpoint(EndpointInfo::new("GET", "/simple").summary("Simple endpoint"));

        let display = OpenApiDisplay::new(OutputMode::Plain);
        let output = display.render_summary(&summary);

        assert!(
            !output.contains("Legend:"),
            "Should not show legend when no special endpoints"
        );
    }

    #[test]
    fn test_group_endpoints_helper() {
        let display = OpenApiDisplay::new(OutputMode::Plain);
        let endpoints = vec![
            EndpointInfo::new("GET", "/users").tag("users"),
            EndpointInfo::new("POST", "/users").tag("users"),
            EndpointInfo::new("GET", "/items").tag("items"),
            EndpointInfo::new("GET", "/health"), // No tag, should be "Other"
        ];

        let groups = display.group_endpoints_by_tag(&endpoints);

        assert_eq!(groups.len(), 3);

        // Check that groups contain expected endpoints
        let users_group = groups.iter().find(|(tag, _)| tag == "users");
        assert!(users_group.is_some());
        assert_eq!(users_group.unwrap().1.len(), 2);

        let other_group = groups.iter().find(|(tag, _)| tag == "Other");
        assert!(other_group.is_some());
        assert_eq!(other_group.unwrap().1.len(), 1);
    }
}
