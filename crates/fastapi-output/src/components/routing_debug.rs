//! Routing debug output component.
//!
//! Provides detailed visual display of the routing decision process,
//! showing which routes were considered, why matches succeeded or failed,
//! parameter extraction results, and middleware that will be applied.
//!
//! # Feature Gating
//!
//! This module is designed for debug output. In production, routing debug
//! should only be enabled when explicitly requested.
//!
//! ```rust,ignore
//! if config.debug_routing {
//!     let debug = RoutingDebug::new(OutputMode::Rich);
//!     println!("{}", debug.format(&routing_result));
//! }
//! ```

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;
use std::collections::HashMap;
use std::fmt::Write;
use std::time::Duration;

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_DIM: &str = "\x1b[2m";

/// Result of a route matching attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchResult {
    /// Route matched successfully.
    Matched,
    /// Route did not match - path mismatch.
    PathMismatch,
    /// Route did not match - method mismatch.
    MethodMismatch,
    /// Route did not match - parameter type validation failed.
    ParamTypeMismatch {
        /// Name of the parameter that failed.
        param_name: String,
        /// Expected type.
        expected_type: String,
        /// Actual value that didn't match.
        actual_value: String,
    },
    /// Route did not match - guard/condition failed.
    GuardFailed {
        /// Name of the guard that failed.
        guard_name: String,
    },
}

impl MatchResult {
    /// Get a human-readable description of the result.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::Matched => "Matched".to_string(),
            Self::PathMismatch => "Path did not match".to_string(),
            Self::MethodMismatch => "Method not allowed".to_string(),
            Self::ParamTypeMismatch {
                param_name,
                expected_type,
                actual_value,
            } => {
                format!("Parameter '{param_name}' expected {expected_type}, got '{actual_value}'")
            }
            Self::GuardFailed { guard_name } => format!("Guard '{guard_name}' failed"),
        }
    }

    /// Check if this is a successful match.
    #[must_use]
    pub fn is_match(&self) -> bool {
        matches!(self, Self::Matched)
    }
}

/// Information about a candidate route that was considered.
#[derive(Debug, Clone)]
pub struct CandidateRoute {
    /// Route pattern (e.g., "/users/{id}").
    pub pattern: String,
    /// Allowed HTTP methods for this route.
    pub methods: Vec<String>,
    /// Handler function name.
    pub handler: Option<String>,
    /// Match result for this candidate.
    pub result: MatchResult,
    /// Whether this route partially matched (path ok, method wrong).
    pub partial_match: bool,
}

impl CandidateRoute {
    /// Create a new candidate route.
    #[must_use]
    pub fn new(pattern: impl Into<String>, result: MatchResult) -> Self {
        Self {
            pattern: pattern.into(),
            methods: Vec::new(),
            handler: None,
            result,
            partial_match: false,
        }
    }

    /// Set the allowed methods.
    #[must_use]
    pub fn methods(mut self, methods: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.methods = methods.into_iter().map(Into::into).collect();
        self
    }

    /// Set the handler name.
    #[must_use]
    pub fn handler(mut self, handler: impl Into<String>) -> Self {
        self.handler = Some(handler.into());
        self
    }

    /// Mark as partial match.
    #[must_use]
    pub fn partial_match(mut self, partial: bool) -> Self {
        self.partial_match = partial;
        self
    }
}

/// Extracted path parameters.
#[derive(Debug, Clone)]
pub struct ExtractedParams {
    /// Parameter name to extracted value.
    pub params: Vec<(String, String)>,
}

impl ExtractedParams {
    /// Create new extracted params.
    #[must_use]
    pub fn new() -> Self {
        Self { params: Vec::new() }
    }

    /// Add a parameter.
    #[must_use]
    pub fn param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.push((name.into(), value.into()));
        self
    }

    /// Check if empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }
}

impl Default for ExtractedParams {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about middleware that will be applied.
#[derive(Debug, Clone)]
pub struct MiddlewareInfo {
    /// Middleware name.
    pub name: String,
    /// Whether this middleware is route-specific.
    pub route_specific: bool,
    /// Order in the middleware stack.
    pub order: usize,
}

impl MiddlewareInfo {
    /// Create new middleware info.
    #[must_use]
    pub fn new(name: impl Into<String>, order: usize) -> Self {
        Self {
            name: name.into(),
            route_specific: false,
            order,
        }
    }

    /// Mark as route-specific.
    #[must_use]
    pub fn route_specific(mut self, specific: bool) -> Self {
        self.route_specific = specific;
        self
    }
}

/// Complete routing debug information.
#[derive(Debug, Clone)]
pub struct RoutingDebugInfo {
    /// The request path being routed.
    pub request_path: String,
    /// The request method.
    pub request_method: String,
    /// All candidate routes that were considered.
    pub candidates: Vec<CandidateRoute>,
    /// The matched route (if any).
    pub matched_route: Option<String>,
    /// Extracted path parameters.
    pub extracted_params: ExtractedParams,
    /// Middleware that will be applied.
    pub middleware: Vec<MiddlewareInfo>,
    /// Time taken to route (in microseconds).
    pub routing_time: Option<Duration>,
    /// Whether any routes partially matched (405 scenario).
    pub has_partial_matches: bool,
}

impl RoutingDebugInfo {
    /// Create new routing debug info.
    #[must_use]
    pub fn new(path: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            request_path: path.into(),
            request_method: method.into(),
            candidates: Vec::new(),
            matched_route: None,
            extracted_params: ExtractedParams::new(),
            middleware: Vec::new(),
            routing_time: None,
            has_partial_matches: false,
        }
    }

    /// Add a candidate route.
    #[must_use]
    pub fn candidate(mut self, candidate: CandidateRoute) -> Self {
        if candidate.partial_match {
            self.has_partial_matches = true;
        }
        if candidate.result.is_match() {
            self.matched_route = Some(candidate.pattern.clone());
        }
        self.candidates.push(candidate);
        self
    }

    /// Set extracted parameters.
    #[must_use]
    pub fn params(mut self, params: ExtractedParams) -> Self {
        self.extracted_params = params;
        self
    }

    /// Add middleware info.
    #[must_use]
    pub fn middleware(mut self, mw: MiddlewareInfo) -> Self {
        self.middleware.push(mw);
        self
    }

    /// Set routing time.
    #[must_use]
    pub fn routing_time(mut self, duration: Duration) -> Self {
        self.routing_time = Some(duration);
        self
    }

    /// Check if routing was successful.
    #[must_use]
    pub fn is_matched(&self) -> bool {
        self.matched_route.is_some()
    }
}

/// Routing debug output formatter.
#[derive(Debug, Clone)]
pub struct RoutingDebug {
    mode: OutputMode,
    theme: FastApiTheme,
    /// Show all candidates or just relevant ones.
    pub show_all_candidates: bool,
    /// Show middleware stack.
    pub show_middleware: bool,
}

impl RoutingDebug {
    /// Create a new routing debug formatter.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            show_all_candidates: true,
            show_middleware: true,
        }
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Format routing debug information.
    #[must_use]
    pub fn format(&self, info: &RoutingDebugInfo) -> String {
        match self.mode {
            OutputMode::Plain => self.format_plain(info),
            OutputMode::Minimal => self.format_minimal(info),
            OutputMode::Rich => self.format_rich(info),
        }
    }

    fn format_plain(&self, info: &RoutingDebugInfo) -> String {
        let mut lines = Vec::new();

        // Header
        lines.push("=== Routing Debug ===".to_string());
        lines.push(format!(
            "Request: {} {}",
            info.request_method, info.request_path
        ));

        if let Some(duration) = info.routing_time {
            lines.push(format!("Routing time: {}", format_duration(duration)));
        }

        // Result summary
        lines.push(String::new());
        if let Some(matched) = &info.matched_route {
            lines.push(format!("Result: MATCHED -> {matched}"));
        } else if info.has_partial_matches {
            lines.push("Result: 405 Method Not Allowed".to_string());
        } else {
            lines.push("Result: 404 Not Found".to_string());
        }

        // Candidates
        if self.show_all_candidates && !info.candidates.is_empty() {
            lines.push(String::new());
            lines.push("Candidates considered:".to_string());
            for candidate in &info.candidates {
                let status = if candidate.result.is_match() {
                    "[MATCH]"
                } else if candidate.partial_match {
                    "[PARTIAL]"
                } else {
                    "[SKIP]"
                };
                let methods = candidate.methods.join(", ");
                lines.push(format!("  {status} {} [{methods}]", candidate.pattern));
                if !candidate.result.is_match() {
                    lines.push(format!(
                        "        Reason: {}",
                        candidate.result.description()
                    ));
                }
            }
        }

        // Extracted parameters
        if !info.extracted_params.is_empty() {
            lines.push(String::new());
            lines.push("Extracted parameters:".to_string());
            for (name, value) in &info.extracted_params.params {
                lines.push(format!("  {name}: {value}"));
            }
        }

        // Middleware
        if self.show_middleware && !info.middleware.is_empty() {
            lines.push(String::new());
            lines.push("Middleware stack:".to_string());
            for mw in &info.middleware {
                let scope = if mw.route_specific {
                    "(route)"
                } else {
                    "(global)"
                };
                lines.push(format!("  {}. {} {scope}", mw.order, mw.name));
            }
        }

        lines.push("=====================".to_string());
        lines.join("\n")
    }

    fn format_minimal(&self, info: &RoutingDebugInfo) -> String {
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let success = self.theme.success.to_ansi_fg();
        let error = self.theme.error.to_ansi_fg();
        let warning = self.theme.warning.to_ansi_fg();

        let mut lines = Vec::new();

        // Header
        lines.push(format!("{muted}=== Routing Debug ==={ANSI_RESET}"));
        let method_color = self.method_color(&info.request_method).to_ansi_fg();
        lines.push(format!(
            "{method_color}{}{ANSI_RESET} {accent}{}{ANSI_RESET}",
            info.request_method, info.request_path
        ));

        // Result
        if let Some(matched) = &info.matched_route {
            lines.push(format!("{success}✓ Matched:{ANSI_RESET} {matched}"));
        } else if info.has_partial_matches {
            lines.push(format!("{warning}⚠ 405 Method Not Allowed{ANSI_RESET}"));
        } else {
            lines.push(format!("{error}✗ 404 Not Found{ANSI_RESET}"));
        }

        // Timing
        if let Some(duration) = info.routing_time {
            lines.push(format!(
                "{muted}Routed in {}{ANSI_RESET}",
                format_duration(duration)
            ));
        }

        // Extracted parameters
        if !info.extracted_params.is_empty() {
            lines.push(format!("{muted}Parameters:{ANSI_RESET}"));
            for (name, value) in &info.extracted_params.params {
                lines.push(format!("  {accent}{name}{ANSI_RESET}: {value}"));
            }
        }

        lines.push(format!("{muted}=================={ANSI_RESET}"));
        lines.join("\n")
    }

    fn format_rich(&self, info: &RoutingDebugInfo) -> String {
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let success = self.theme.success.to_ansi_fg();
        let error = self.theme.error.to_ansi_fg();
        let warning = self.theme.warning.to_ansi_fg();
        let border = self.theme.border.to_ansi_fg();
        let header_style = self.theme.header.to_ansi_fg();

        let mut lines = Vec::new();

        // Top border
        lines.push(format!(
            "{border}┌─────────────────────────────────────────────┐{ANSI_RESET}"
        ));
        lines.push(format!(
            "{border}│{ANSI_RESET} {header_style}{ANSI_BOLD}Routing Debug{ANSI_RESET}                                {border}│{ANSI_RESET}"
        ));
        lines.push(format!(
            "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
        ));

        // Request line
        let method_bg = self.method_color(&info.request_method).to_ansi_bg();
        lines.push(format!(
            "{border}│{ANSI_RESET} {method_bg}{ANSI_BOLD} {} {ANSI_RESET} {accent}{}{ANSI_RESET}",
            info.request_method, info.request_path
        ));

        // Result row
        lines.push(format!(
            "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
        ));
        if let Some(matched) = &info.matched_route {
            lines.push(format!(
                "{border}│{ANSI_RESET} {success}✓ Matched{ANSI_RESET} → {matched}"
            ));
        } else if info.has_partial_matches {
            lines.push(format!(
                "{border}│{ANSI_RESET} {warning}⚠ 405 Method Not Allowed{ANSI_RESET}"
            ));
            // Show allowed methods
            let allowed: Vec<_> = info
                .candidates
                .iter()
                .filter(|c| c.partial_match)
                .flat_map(|c| c.methods.iter())
                .collect();
            if !allowed.is_empty() {
                lines.push(format!(
                    "{border}│{ANSI_RESET}   {muted}Allowed:{ANSI_RESET} {}",
                    allowed.into_iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        } else {
            lines.push(format!(
                "{border}│{ANSI_RESET} {error}✗ 404 Not Found{ANSI_RESET}"
            ));
        }

        // Timing
        if let Some(duration) = info.routing_time {
            lines.push(format!(
                "{border}│{ANSI_RESET} {muted}Routed in {}{ANSI_RESET}",
                format_duration(duration)
            ));
        }

        // Candidates section
        if self.show_all_candidates && !info.candidates.is_empty() {
            lines.push(format!(
                "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
            ));
            lines.push(format!(
                "{border}│{ANSI_RESET} {header_style}Candidates{ANSI_RESET} {muted}({}){ANSI_RESET}",
                info.candidates.len()
            ));

            for candidate in &info.candidates {
                let (icon, color) = if candidate.result.is_match() {
                    ("✓", &success)
                } else if candidate.partial_match {
                    ("◐", &warning)
                } else {
                    ("○", &muted)
                };
                let methods = candidate.methods.join("|");
                lines.push(format!(
                    "{border}│{ANSI_RESET}   {color}{icon}{ANSI_RESET} {}{} {muted}[{methods}]{ANSI_RESET}",
                    candidate.pattern,
                    if let Some(h) = &candidate.handler {
                        format!(" {muted}→ {h}{ANSI_RESET}")
                    } else {
                        String::new()
                    }
                ));
                if !candidate.result.is_match()
                    && !matches!(candidate.result, MatchResult::PathMismatch)
                {
                    lines.push(format!(
                        "{border}│{ANSI_RESET}     {muted}{}{ANSI_RESET}",
                        candidate.result.description()
                    ));
                }
            }
        }

        // Extracted parameters
        if !info.extracted_params.is_empty() {
            lines.push(format!(
                "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
            ));
            lines.push(format!(
                "{border}│{ANSI_RESET} {header_style}Extracted Parameters{ANSI_RESET}"
            ));
            for (name, value) in &info.extracted_params.params {
                lines.push(format!(
                    "{border}│{ANSI_RESET}   {accent}{name}{ANSI_RESET}: {value}"
                ));
            }
        }

        // Middleware stack
        if self.show_middleware && !info.middleware.is_empty() {
            lines.push(format!(
                "{border}├─────────────────────────────────────────────┤{ANSI_RESET}"
            ));
            lines.push(format!(
                "{border}│{ANSI_RESET} {header_style}Middleware Stack{ANSI_RESET}"
            ));
            for mw in &info.middleware {
                let scope = if mw.route_specific {
                    format!("{accent}(route){ANSI_RESET}")
                } else {
                    format!("{muted}(global){ANSI_RESET}")
                };
                lines.push(format!(
                    "{border}│{ANSI_RESET}   {muted}{}→{ANSI_RESET} {} {scope}",
                    mw.order, mw.name
                ));
            }
        }

        // Bottom border
        lines.push(format!(
            "{border}└─────────────────────────────────────────────┘{ANSI_RESET}"
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

/// Format a duration in human-readable form.
fn format_duration(duration: Duration) -> String {
    let micros = duration.as_micros();
    if micros < 1000 {
        format!("{micros}µs")
    } else if micros < 1_000_000 {
        let whole = micros / 1000;
        let frac = (micros % 1000) / 10;
        format!("{whole}.{frac:02}ms")
    } else {
        let whole = micros / 1_000_000;
        let frac = (micros % 1_000_000) / 10_000;
        format!("{whole}.{frac:02}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_successful_routing() -> RoutingDebugInfo {
        RoutingDebugInfo::new("/api/users/42", "GET")
            .candidate(
                CandidateRoute::new("/api/health", MatchResult::PathMismatch)
                    .methods(["GET"])
                    .handler("health_check"),
            )
            .candidate(
                CandidateRoute::new("/api/users", MatchResult::PathMismatch)
                    .methods(["GET", "POST"])
                    .handler("list_users"),
            )
            .candidate(
                CandidateRoute::new("/api/users/{id}", MatchResult::Matched)
                    .methods(["GET", "PUT", "DELETE"])
                    .handler("get_user"),
            )
            .params(ExtractedParams::new().param("id", "42"))
            .middleware(MiddlewareInfo::new("RequestLogger", 1))
            .middleware(MiddlewareInfo::new("Auth", 2))
            .middleware(MiddlewareInfo::new("RateLimit", 3).route_specific(true))
            .routing_time(Duration::from_micros(45))
    }

    fn sample_404_routing() -> RoutingDebugInfo {
        RoutingDebugInfo::new("/api/nonexistent", "GET")
            .candidate(
                CandidateRoute::new("/api/users", MatchResult::PathMismatch)
                    .methods(["GET"])
                    .handler("list_users"),
            )
            .candidate(
                CandidateRoute::new("/api/items", MatchResult::PathMismatch)
                    .methods(["GET"])
                    .handler("list_items"),
            )
            .routing_time(Duration::from_micros(12))
    }

    fn sample_405_routing() -> RoutingDebugInfo {
        RoutingDebugInfo::new("/api/users", "DELETE")
            .candidate(
                CandidateRoute::new("/api/users", MatchResult::MethodMismatch)
                    .methods(["GET", "POST"])
                    .handler("list_users")
                    .partial_match(true),
            )
            .routing_time(Duration::from_micros(8))
    }

    #[test]
    fn test_match_result_description() {
        assert_eq!(MatchResult::Matched.description(), "Matched");
        assert_eq!(
            MatchResult::PathMismatch.description(),
            "Path did not match"
        );
        assert_eq!(
            MatchResult::ParamTypeMismatch {
                param_name: "id".to_string(),
                expected_type: "int".to_string(),
                actual_value: "abc".to_string(),
            }
            .description(),
            "Parameter 'id' expected int, got 'abc'"
        );
    }

    #[test]
    fn test_routing_debug_plain_success() {
        let debug = RoutingDebug::new(OutputMode::Plain);
        let output = debug.format(&sample_successful_routing());

        assert!(output.contains("Routing Debug"));
        assert!(output.contains("GET /api/users/42"));
        assert!(output.contains("MATCHED"));
        assert!(output.contains("/api/users/{id}"));
        assert!(output.contains("id: 42"));
        assert!(output.contains("RequestLogger"));
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn test_routing_debug_plain_404() {
        let debug = RoutingDebug::new(OutputMode::Plain);
        let output = debug.format(&sample_404_routing());

        assert!(output.contains("404 Not Found"));
        assert!(!output.contains("MATCHED"));
    }

    #[test]
    fn test_routing_debug_plain_405() {
        let debug = RoutingDebug::new(OutputMode::Plain);
        let output = debug.format(&sample_405_routing());

        assert!(output.contains("405 Method Not Allowed"));
        assert!(output.contains("[PARTIAL]"));
    }

    #[test]
    fn test_routing_debug_rich_has_ansi() {
        let debug = RoutingDebug::new(OutputMode::Rich);
        let output = debug.format(&sample_successful_routing());

        assert!(output.contains("\x1b["));
        assert!(output.contains("✓ Matched"));
    }

    #[test]
    fn test_extracted_params_builder() {
        let params = ExtractedParams::new()
            .param("id", "42")
            .param("name", "alice");

        assert_eq!(params.params.len(), 2);
        assert!(!params.is_empty());
    }

    #[test]
    fn test_candidate_route_builder() {
        let candidate = CandidateRoute::new("/api/users/{id}", MatchResult::Matched)
            .methods(["GET", "PUT"])
            .handler("get_user")
            .partial_match(false);

        assert_eq!(candidate.pattern, "/api/users/{id}");
        assert_eq!(candidate.methods, vec!["GET", "PUT"]);
        assert!(candidate.result.is_match());
    }

    #[test]
    fn test_middleware_info_builder() {
        let mw = MiddlewareInfo::new("Auth", 1).route_specific(true);

        assert_eq!(mw.name, "Auth");
        assert_eq!(mw.order, 1);
        assert!(mw.route_specific);
    }
}
