//! Theme system for fastapi_rust console output.
//!
//! Defines color palettes and styling presets for consistent
//! visual output across all components. Colors follow the FastAPI
//! visual identity and Swagger UI conventions for familiarity.
//!
//! # Theme Presets
//!
//! - `FastApi` - Default theme inspired by FastAPI documentation
//! - `Neon` - High-contrast cyberpunk theme
//! - `Minimal` - Grayscale with subtle accents
//! - `Monokai` - Dark theme inspired by the Monokai color scheme
//!
//! # Example
//!
//! ```rust
//! use fastapi_output::themes::{FastApiTheme, ThemePreset};
//!
//! // Get default theme
//! let theme = FastApiTheme::default();
//!
//! // Get theme by preset
//! let neon = FastApiTheme::from_preset(ThemePreset::Neon);
//!
//! // Parse from environment variable
//! let preset: ThemePreset = "monokai".parse().unwrap();
//! ```

// Hex color literals (0xRRGGBB) are idiomatic and readable as-is
#![allow(clippy::unreadable_literal)]

use std::str::FromStr;

/// A color in RGB format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// Red component (0-255).
    pub r: u8,
    /// Green component (0-255).
    pub g: u8,
    /// Blue component (0-255).
    pub b: u8,
}

impl Color {
    /// Create a new color from RGB values.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Create a color from a hex value (0xRRGGBB).
    #[must_use]
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
        }
    }

    /// Convert to hex string (e.g., "#009688").
    #[must_use]
    pub fn to_hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Convert to RGB tuple.
    #[must_use]
    pub const fn to_rgb(&self) -> (u8, u8, u8) {
        (self.r, self.g, self.b)
    }

    /// Convert to ANSI 24-bit foreground escape code.
    #[must_use]
    pub fn to_ansi_fg(&self) -> String {
        format!("\x1b[38;2;{};{};{}m", self.r, self.g, self.b)
    }

    /// Convert to ANSI 24-bit background escape code.
    #[must_use]
    pub fn to_ansi_bg(&self) -> String {
        format!("\x1b[48;2;{};{};{}m", self.r, self.g, self.b)
    }
}

/// Convert RGB tuple to hex string.
#[must_use]
pub fn rgb_to_hex(rgb: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

/// Parse hex color to RGB tuple.
///
/// Supports both 6-digit (#RRGGBB) and 3-digit (#RGB) formats.
/// The leading '#' is optional.
///
/// # Example
///
/// ```rust
/// use fastapi_output::themes::hex_to_rgb;
///
/// assert_eq!(hex_to_rgb("#009688"), Some((0, 150, 136)));
/// assert_eq!(hex_to_rgb("FF5500"), Some((255, 85, 0)));
/// assert_eq!(hex_to_rgb("#F00"), Some((255, 0, 0)));
/// ```
#[must_use]
pub fn hex_to_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some((r, g, b))
    } else if hex.len() == 3 {
        let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
        let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
        let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
        Some((r, g, b))
    } else {
        None
    }
}

/// FastAPI-inspired color theme for console output.
///
/// Contains colors for:
/// - Brand identity (primary, secondary, accent)
/// - Semantic meaning (success, warning, error, info)
/// - HTTP methods (GET, POST, PUT, DELETE, etc.)
/// - Status codes (1xx, 2xx, 3xx, 4xx, 5xx)
/// - Structural elements (border, header, muted, highlight)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastApiTheme {
    // === Brand Colors ===
    /// Primary brand color (teal, inspired by FastAPI docs).
    pub primary: Color,
    /// Secondary brand color.
    pub secondary: Color,
    /// Accent color for highlights.
    pub accent: Color,

    // === Semantic Colors ===
    /// Success indicator color (green).
    pub success: Color,
    /// Warning indicator color (orange/yellow).
    pub warning: Color,
    /// Error indicator color (red).
    pub error: Color,
    /// Info indicator color (blue).
    pub info: Color,

    // === HTTP Method Colors (Swagger UI conventions) ===
    /// GET method color (blue).
    pub http_get: Color,
    /// POST method color (green).
    pub http_post: Color,
    /// PUT method color (orange).
    pub http_put: Color,
    /// DELETE method color (red).
    pub http_delete: Color,
    /// PATCH method color (cyan).
    pub http_patch: Color,
    /// OPTIONS method color (gray).
    pub http_options: Color,
    /// HEAD method color (purple).
    pub http_head: Color,

    // === Status Code Colors ===
    /// 1xx informational (gray).
    pub status_1xx: Color,
    /// 2xx success (green).
    pub status_2xx: Color,
    /// 3xx redirect (cyan).
    pub status_3xx: Color,
    /// 4xx client error (yellow/orange).
    pub status_4xx: Color,
    /// 5xx server error (red).
    pub status_5xx: Color,

    // === Structural Colors ===
    /// Border color for boxes/panels.
    pub border: Color,
    /// Header text color.
    pub header: Color,
    /// Muted/secondary text color.
    pub muted: Color,
    /// Background highlight color.
    pub highlight_bg: Color,
}

impl FastApiTheme {
    /// Create a theme from a preset.
    #[must_use]
    pub fn from_preset(preset: ThemePreset) -> Self {
        match preset {
            ThemePreset::FastApi | ThemePreset::Default => Self::fastapi(),
            ThemePreset::Neon => Self::neon(),
            ThemePreset::Minimal => Self::minimal(),
            ThemePreset::Monokai => Self::monokai(),
        }
    }

    /// Create the default FastAPI-inspired theme.
    ///
    /// Colors chosen to match FastAPI documentation styling
    /// and Swagger UI conventions for familiarity.
    #[must_use]
    pub fn fastapi() -> Self {
        Self {
            // Brand colors (FastAPI teal/green)
            primary: Color::from_hex(0x009688),   // Teal 500
            secondary: Color::from_hex(0x4CAF50), // Green 500
            accent: Color::from_hex(0xFF9800),    // Orange 500

            // Semantic colors
            success: Color::from_hex(0x4CAF50), // Green
            warning: Color::from_hex(0xFF9800), // Orange
            error: Color::from_hex(0xF44336),   // Red
            info: Color::from_hex(0x2196F3),    // Blue

            // HTTP methods (Swagger UI)
            http_get: Color::from_hex(0x61AFFE),     // Blue
            http_post: Color::from_hex(0x49CC90),    // Green
            http_put: Color::from_hex(0xFCA130),     // Orange
            http_delete: Color::from_hex(0xF93E3E),  // Red
            http_patch: Color::from_hex(0x50E3C2),   // Cyan
            http_options: Color::from_hex(0x808080), // Gray
            http_head: Color::from_hex(0x9370DB),    // Purple

            // Status codes
            status_1xx: Color::from_hex(0x808080), // Gray
            status_2xx: Color::from_hex(0x4CAF50), // Green
            status_3xx: Color::from_hex(0x00BCD4), // Cyan
            status_4xx: Color::from_hex(0xFFC107), // Yellow/Amber
            status_5xx: Color::from_hex(0xF44336), // Red

            // Structural
            border: Color::from_hex(0x9E9E9E),       // Gray 500
            header: Color::from_hex(0x009688),       // Primary
            muted: Color::from_hex(0x757575),        // Gray 600
            highlight_bg: Color::from_hex(0x263238), // Blue Grey 900
        }
    }

    /// Create a neon/cyberpunk theme with high contrast.
    #[must_use]
    pub fn neon() -> Self {
        Self {
            primary: Color::from_hex(0x00FFFF),   // Cyan
            secondary: Color::from_hex(0xFF00FF), // Magenta
            accent: Color::from_hex(0xFFFF00),    // Yellow

            success: Color::from_hex(0x00FF80), // Neon green
            warning: Color::from_hex(0xFFFF00), // Yellow
            error: Color::from_hex(0xFF0040),   // Hot pink/red
            info: Color::from_hex(0x0080FF),    // Electric blue

            http_get: Color::from_hex(0x00FFFF),
            http_post: Color::from_hex(0x00FF80),
            http_put: Color::from_hex(0xFFA500),
            http_delete: Color::from_hex(0xFF0040),
            http_patch: Color::from_hex(0xFF00FF),
            http_options: Color::from_hex(0x808080),
            http_head: Color::from_hex(0x9400D3),

            status_1xx: Color::from_hex(0x808080),
            status_2xx: Color::from_hex(0x00FF80),
            status_3xx: Color::from_hex(0x00FFFF),
            status_4xx: Color::from_hex(0xFFFF00),
            status_5xx: Color::from_hex(0xFF0040),

            border: Color::from_hex(0x00FFFF),
            header: Color::from_hex(0xFF00FF),
            muted: Color::from_hex(0x646464),
            highlight_bg: Color::from_hex(0x141428),
        }
    }

    /// Create a minimal grayscale theme with accent colors.
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            primary: Color::from_hex(0xC8C8C8),
            secondary: Color::from_hex(0xB4B4B4),
            accent: Color::from_hex(0xFF9800),

            success: Color::from_hex(0x64C864),
            warning: Color::from_hex(0xFFB400),
            error: Color::from_hex(0xFF6464),
            info: Color::from_hex(0x6496FF),

            http_get: Color::from_hex(0x9696C8),
            http_post: Color::from_hex(0x96C896),
            http_put: Color::from_hex(0xC8B464),
            http_delete: Color::from_hex(0xC86464),
            http_patch: Color::from_hex(0x64C8C8),
            http_options: Color::from_hex(0x808080),
            http_head: Color::from_hex(0xB496C8),

            status_1xx: Color::from_hex(0x808080),
            status_2xx: Color::from_hex(0x64C864),
            status_3xx: Color::from_hex(0x64C8C8),
            status_4xx: Color::from_hex(0xC8B464),
            status_5xx: Color::from_hex(0xC86464),

            border: Color::from_hex(0x646464),
            header: Color::from_hex(0xDCDCDC),
            muted: Color::from_hex(0x505050),
            highlight_bg: Color::from_hex(0x1E1E1E),
        }
    }

    /// Create a Monokai-inspired dark theme.
    #[must_use]
    pub fn monokai() -> Self {
        Self {
            primary: Color::from_hex(0xA6E22E),   // Monokai green
            secondary: Color::from_hex(0x66D9EF), // Monokai cyan
            accent: Color::from_hex(0xFD971F),    // Monokai orange

            success: Color::from_hex(0xA6E22E),
            warning: Color::from_hex(0xFD971F),
            error: Color::from_hex(0xF92672), // Monokai pink/red
            info: Color::from_hex(0x66D9EF),

            http_get: Color::from_hex(0x66D9EF),
            http_post: Color::from_hex(0xA6E22E),
            http_put: Color::from_hex(0xFD971F),
            http_delete: Color::from_hex(0xF92672),
            http_patch: Color::from_hex(0xAE81FF), // Monokai purple
            http_options: Color::from_hex(0x75715E),
            http_head: Color::from_hex(0xAE81FF),

            status_1xx: Color::from_hex(0x75715E),
            status_2xx: Color::from_hex(0xA6E22E),
            status_3xx: Color::from_hex(0x66D9EF),
            status_4xx: Color::from_hex(0xFD971F),
            status_5xx: Color::from_hex(0xF92672),

            border: Color::from_hex(0x75715E),
            header: Color::from_hex(0xF8F8F2),
            muted: Color::from_hex(0x75715E),
            highlight_bg: Color::from_hex(0x272822),
        }
    }

    // === Color Lookup Methods ===

    /// Get the color for an HTTP method.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastapi_output::themes::FastApiTheme;
    ///
    /// let theme = FastApiTheme::default();
    /// let get_color = theme.http_method_color("GET");
    /// let post_color = theme.http_method_color("post"); // case-insensitive
    /// ```
    #[must_use]
    pub fn http_method_color(&self, method: &str) -> Color {
        match method.to_uppercase().as_str() {
            "GET" => self.http_get,
            "POST" => self.http_post,
            "PUT" => self.http_put,
            "DELETE" => self.http_delete,
            "PATCH" => self.http_patch,
            "OPTIONS" => self.http_options,
            "HEAD" => self.http_head,
            _ => self.muted,
        }
    }

    /// Get the color for an HTTP status code.
    ///
    /// # Example
    ///
    /// ```rust
    /// use fastapi_output::themes::FastApiTheme;
    ///
    /// let theme = FastApiTheme::default();
    /// let success_color = theme.status_code_color(200);
    /// let error_color = theme.status_code_color(500);
    /// ```
    #[must_use]
    pub fn status_code_color(&self, code: u16) -> Color {
        match code {
            100..=199 => self.status_1xx,
            200..=299 => self.status_2xx,
            300..=399 => self.status_3xx,
            400..=499 => self.status_4xx,
            500..=599 => self.status_5xx,
            _ => self.muted,
        }
    }

    // === Hex String Helpers ===

    /// Get primary color as hex string.
    #[must_use]
    pub fn primary_hex(&self) -> String {
        self.primary.to_hex()
    }

    /// Get success color as hex string.
    #[must_use]
    pub fn success_hex(&self) -> String {
        self.success.to_hex()
    }

    /// Get error color as hex string.
    #[must_use]
    pub fn error_hex(&self) -> String {
        self.error.to_hex()
    }

    /// Get warning color as hex string.
    #[must_use]
    pub fn warning_hex(&self) -> String {
        self.warning.to_hex()
    }

    /// Get info color as hex string.
    #[must_use]
    pub fn info_hex(&self) -> String {
        self.info.to_hex()
    }

    /// Get accent color as hex string.
    #[must_use]
    pub fn accent_hex(&self) -> String {
        self.accent.to_hex()
    }

    /// Get color for HTTP method as hex string.
    #[must_use]
    pub fn http_method_hex(&self, method: &str) -> String {
        self.http_method_color(method).to_hex()
    }

    /// Get color for status code as hex string.
    #[must_use]
    pub fn status_code_hex(&self, code: u16) -> String {
        self.status_code_color(code).to_hex()
    }
}

impl Default for FastApiTheme {
    fn default() -> Self {
        Self::fastapi()
    }
}

/// Predefined theme presets.
///
/// Select a theme by name or environment variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePreset {
    /// Default FastAPI-inspired theme.
    #[default]
    Default,
    /// Alias for Default - FastAPI-inspired theme.
    FastApi,
    /// Neon/cyberpunk high contrast theme.
    Neon,
    /// Minimal grayscale with accents.
    Minimal,
    /// Monokai dark theme.
    Monokai,
}

impl ThemePreset {
    /// Get the `FastApiTheme` for this preset.
    #[must_use]
    pub fn theme(&self) -> FastApiTheme {
        FastApiTheme::from_preset(*self)
    }

    /// List all available preset names.
    #[must_use]
    pub fn available_presets() -> &'static [&'static str] {
        &["default", "fastapi", "neon", "minimal", "monokai"]
    }
}

impl std::fmt::Display for ThemePreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => write!(f, "default"),
            Self::FastApi => write!(f, "fastapi"),
            Self::Neon => write!(f, "neon"),
            Self::Minimal => write!(f, "minimal"),
            Self::Monokai => write!(f, "monokai"),
        }
    }
}

impl FromStr for ThemePreset {
    type Err = ThemePresetParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default" | "fastapi" => Ok(Self::FastApi),
            "neon" | "cyberpunk" => Ok(Self::Neon),
            "minimal" | "gray" | "grey" => Ok(Self::Minimal),
            "monokai" | "dark" => Ok(Self::Monokai),
            _ => Err(ThemePresetParseError(s.to_string())),
        }
    }
}

/// Error parsing theme preset name.
#[derive(Debug, Clone)]
pub struct ThemePresetParseError(String);

impl ThemePresetParseError {
    /// Get the invalid preset name that was provided.
    #[must_use]
    pub fn invalid_name(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ThemePresetParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown theme preset '{}', available: {}",
            self.0,
            ThemePreset::available_presets().join(", ")
        )
    }
}

impl std::error::Error for ThemePresetParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_not_black(c: Color) -> bool {
        c.r > 0 || c.g > 0 || c.b > 0
    }

    // === Color Tests ===

    #[test]
    fn test_color_from_hex() {
        let color = Color::from_hex(0xFF5500);
        assert_eq!(color.r, 0xFF);
        assert_eq!(color.g, 0x55);
        assert_eq!(color.b, 0x00);
    }

    #[test]
    fn test_color_to_hex() {
        let color = Color::new(255, 85, 0);
        assert_eq!(color.to_hex(), "#ff5500");
    }

    #[test]
    fn test_color_to_rgb() {
        let color = Color::new(100, 150, 200);
        assert_eq!(color.to_rgb(), (100, 150, 200));
    }

    #[test]
    fn test_color_to_ansi() {
        let color = Color::new(255, 128, 64);
        assert_eq!(color.to_ansi_fg(), "\x1b[38;2;255;128;64m");
        assert_eq!(color.to_ansi_bg(), "\x1b[48;2;255;128;64m");
    }

    // === Conversion Utility Tests ===

    #[test]
    fn test_rgb_to_hex() {
        assert_eq!(rgb_to_hex((0, 150, 136)), "#009688");
        assert_eq!(rgb_to_hex((255, 255, 255)), "#ffffff");
        assert_eq!(rgb_to_hex((0, 0, 0)), "#000000");
    }

    #[test]
    fn test_hex_to_rgb_6_digit() {
        assert_eq!(hex_to_rgb("#009688"), Some((0, 150, 136)));
        assert_eq!(hex_to_rgb("009688"), Some((0, 150, 136)));
        assert_eq!(hex_to_rgb("#FF5500"), Some((255, 85, 0)));
        assert_eq!(hex_to_rgb("#ffffff"), Some((255, 255, 255)));
    }

    #[test]
    fn test_hex_to_rgb_3_digit() {
        assert_eq!(hex_to_rgb("#F00"), Some((255, 0, 0)));
        assert_eq!(hex_to_rgb("0F0"), Some((0, 255, 0)));
        assert_eq!(hex_to_rgb("#FFF"), Some((255, 255, 255)));
    }

    #[test]
    fn test_hex_to_rgb_invalid() {
        assert_eq!(hex_to_rgb("invalid"), None);
        assert_eq!(hex_to_rgb("#12345"), None);
        assert_eq!(hex_to_rgb(""), None);
        assert_eq!(hex_to_rgb("#GGG"), None);
    }

    // === Theme Tests ===

    #[test]
    fn test_theme_default_has_all_colors() {
        let theme = FastApiTheme::default();

        // Brand colors
        assert!(is_not_black(theme.primary));
        assert!(is_not_black(theme.secondary));
        assert!(is_not_black(theme.accent));

        // Semantic colors
        assert!(is_not_black(theme.success));
        assert!(is_not_black(theme.warning));
        assert!(is_not_black(theme.error));
        assert!(is_not_black(theme.info));

        // HTTP method colors
        assert!(is_not_black(theme.http_get));
        assert!(is_not_black(theme.http_post));
        assert!(is_not_black(theme.http_put));
        assert!(is_not_black(theme.http_delete));
    }

    #[test]
    fn test_theme_presets_differ() {
        let fastapi = FastApiTheme::fastapi();
        let neon = FastApiTheme::neon();
        let minimal = FastApiTheme::minimal();
        let monokai = FastApiTheme::monokai();

        assert_ne!(fastapi, neon);
        assert_ne!(fastapi, minimal);
        assert_ne!(fastapi, monokai);
        assert_ne!(neon, minimal);
        assert_ne!(neon, monokai);
        assert_ne!(minimal, monokai);
    }

    #[test]
    fn test_theme_from_preset() {
        assert_eq!(
            FastApiTheme::from_preset(ThemePreset::Default),
            FastApiTheme::fastapi()
        );
        assert_eq!(
            FastApiTheme::from_preset(ThemePreset::FastApi),
            FastApiTheme::fastapi()
        );
        assert_eq!(
            FastApiTheme::from_preset(ThemePreset::Neon),
            FastApiTheme::neon()
        );
        assert_eq!(
            FastApiTheme::from_preset(ThemePreset::Minimal),
            FastApiTheme::minimal()
        );
        assert_eq!(
            FastApiTheme::from_preset(ThemePreset::Monokai),
            FastApiTheme::monokai()
        );
    }

    // === HTTP Method Color Tests ===

    #[test]
    fn test_http_method_colors() {
        let theme = FastApiTheme::default();

        assert_eq!(theme.http_method_color("GET"), theme.http_get);
        assert_eq!(theme.http_method_color("get"), theme.http_get);
        assert_eq!(theme.http_method_color("POST"), theme.http_post);
        assert_eq!(theme.http_method_color("PUT"), theme.http_put);
        assert_eq!(theme.http_method_color("DELETE"), theme.http_delete);
        assert_eq!(theme.http_method_color("PATCH"), theme.http_patch);
        assert_eq!(theme.http_method_color("OPTIONS"), theme.http_options);
        assert_eq!(theme.http_method_color("HEAD"), theme.http_head);
        assert_eq!(theme.http_method_color("UNKNOWN"), theme.muted);
    }

    #[test]
    fn test_http_method_hex() {
        let theme = FastApiTheme::default();
        assert_eq!(theme.http_method_hex("GET"), theme.http_get.to_hex());
        assert_eq!(theme.http_method_hex("POST"), theme.http_post.to_hex());
    }

    // === Status Code Color Tests ===

    #[test]
    fn test_status_code_colors() {
        let theme = FastApiTheme::default();

        assert_eq!(theme.status_code_color(100), theme.status_1xx);
        assert_eq!(theme.status_code_color(199), theme.status_1xx);
        assert_eq!(theme.status_code_color(200), theme.status_2xx);
        assert_eq!(theme.status_code_color(201), theme.status_2xx);
        assert_eq!(theme.status_code_color(301), theme.status_3xx);
        assert_eq!(theme.status_code_color(404), theme.status_4xx);
        assert_eq!(theme.status_code_color(500), theme.status_5xx);
        assert_eq!(theme.status_code_color(503), theme.status_5xx);
        assert_eq!(theme.status_code_color(600), theme.muted);
        assert_eq!(theme.status_code_color(99), theme.muted);
    }

    #[test]
    fn test_status_code_hex() {
        let theme = FastApiTheme::default();
        assert_eq!(theme.status_code_hex(200), theme.status_2xx.to_hex());
        assert_eq!(theme.status_code_hex(500), theme.status_5xx.to_hex());
    }

    // === Hex Helper Tests ===

    #[test]
    fn test_hex_helpers() {
        let theme = FastApiTheme::default();

        assert_eq!(theme.primary_hex(), theme.primary.to_hex());
        assert_eq!(theme.success_hex(), theme.success.to_hex());
        assert_eq!(theme.error_hex(), theme.error.to_hex());
        assert_eq!(theme.warning_hex(), theme.warning.to_hex());
        assert_eq!(theme.info_hex(), theme.info.to_hex());
        assert_eq!(theme.accent_hex(), theme.accent.to_hex());
    }

    // === ThemePreset Tests ===

    #[test]
    fn test_theme_preset_display() {
        assert_eq!(ThemePreset::Default.to_string(), "default");
        assert_eq!(ThemePreset::FastApi.to_string(), "fastapi");
        assert_eq!(ThemePreset::Neon.to_string(), "neon");
        assert_eq!(ThemePreset::Minimal.to_string(), "minimal");
        assert_eq!(ThemePreset::Monokai.to_string(), "monokai");
    }

    #[test]
    fn test_theme_preset_from_str() {
        assert_eq!(
            "default".parse::<ThemePreset>().unwrap(),
            ThemePreset::FastApi
        );
        assert_eq!(
            "fastapi".parse::<ThemePreset>().unwrap(),
            ThemePreset::FastApi
        );
        assert_eq!(
            "FASTAPI".parse::<ThemePreset>().unwrap(),
            ThemePreset::FastApi
        );
        assert_eq!("neon".parse::<ThemePreset>().unwrap(), ThemePreset::Neon);
        assert_eq!(
            "cyberpunk".parse::<ThemePreset>().unwrap(),
            ThemePreset::Neon
        );
        assert_eq!(
            "minimal".parse::<ThemePreset>().unwrap(),
            ThemePreset::Minimal
        );
        assert_eq!("gray".parse::<ThemePreset>().unwrap(), ThemePreset::Minimal);
        assert_eq!("grey".parse::<ThemePreset>().unwrap(), ThemePreset::Minimal);
        assert_eq!(
            "monokai".parse::<ThemePreset>().unwrap(),
            ThemePreset::Monokai
        );
        assert_eq!("dark".parse::<ThemePreset>().unwrap(), ThemePreset::Monokai);
    }

    #[test]
    fn test_theme_preset_from_str_invalid() {
        let err = "invalid".parse::<ThemePreset>().unwrap_err();
        assert_eq!(err.invalid_name(), "invalid");
        assert!(err.to_string().contains("invalid"));
        assert!(err.to_string().contains("available"));
    }

    #[test]
    fn test_theme_preset_theme() {
        assert_eq!(ThemePreset::FastApi.theme(), FastApiTheme::fastapi());
        assert_eq!(ThemePreset::Neon.theme(), FastApiTheme::neon());
    }

    #[test]
    fn test_available_presets() {
        let presets = ThemePreset::available_presets();
        assert!(presets.contains(&"default"));
        assert!(presets.contains(&"fastapi"));
        assert!(presets.contains(&"neon"));
        assert!(presets.contains(&"minimal"));
        assert!(presets.contains(&"monokai"));
    }
}
