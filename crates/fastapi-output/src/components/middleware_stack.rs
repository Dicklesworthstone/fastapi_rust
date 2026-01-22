//! Middleware stack visualization component.
//!
//! Provides a structured view of middleware execution order and
//! response flow, with plain and rich rendering modes.

use crate::facade::RichOutput;
use crate::mode::OutputMode;

/// Information about a single middleware layer.
#[derive(Debug, Clone)]
pub struct MiddlewareInfo {
    /// Display name of the middleware.
    pub name: String,
    /// Type name for debugging (optional).
    pub type_name: String,
    /// Execution order (1-based).
    pub order: usize,
    /// Whether middleware can short-circuit the request.
    pub can_short_circuit: bool,
    /// Optional configuration summary.
    pub config_summary: Option<String>,
}

impl MiddlewareInfo {
    /// Create a new middleware info entry.
    #[must_use]
    pub fn new(name: &str, order: usize) -> Self {
        Self {
            name: name.to_string(),
            type_name: name.to_string(),
            order,
            can_short_circuit: false,
            config_summary: None,
        }
    }

    /// Set a configuration summary.
    #[must_use]
    pub fn with_config(mut self, config: &str) -> Self {
        self.config_summary = Some(config.to_string());
        self
    }

    /// Set a type name different from the display name.
    #[must_use]
    pub fn with_type_name(mut self, type_name: &str) -> Self {
        self.type_name = type_name.to_string();
        self
    }

    /// Mark this middleware as short-circuiting.
    #[must_use]
    pub fn short_circuits(mut self) -> Self {
        self.can_short_circuit = true;
        self
    }
}

/// Middleware stack display component.
#[derive(Debug, Clone)]
pub struct MiddlewareStackDisplay {
    middlewares: Vec<MiddlewareInfo>,
    show_config: bool,
    show_flow: bool,
}

impl MiddlewareStackDisplay {
    /// Create a new middleware stack display.
    #[must_use]
    pub fn new(middlewares: Vec<MiddlewareInfo>) -> Self {
        Self {
            middlewares,
            show_config: true,
            show_flow: true,
        }
    }

    /// Hide configuration summaries.
    #[must_use]
    pub fn hide_config(mut self) -> Self {
        self.show_config = false;
        self
    }

    /// Hide response flow line.
    #[must_use]
    pub fn hide_flow(mut self) -> Self {
        self.show_flow = false;
        self
    }

    /// Render the middleware stack to the provided output.
    pub fn render(&self, output: &RichOutput) {
        match output.mode() {
            OutputMode::Rich => self.render_rich(output),
            OutputMode::Plain | OutputMode::Minimal => self.render_plain(output),
        }
    }

    fn render_plain(&self, output: &RichOutput) {
        for line in self.plain_lines() {
            output.print(&line);
        }
    }

    fn render_rich(&self, output: &RichOutput) {
        // Rich rendering is intentionally simple for now.
        // When rich_rust is enabled, this can be upgraded to use panels/trees.
        self.render_plain(output);
    }

    fn plain_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let total_layers = self.middlewares.len() + 1;
        lines.push(format!("Middleware Stack ({total_layers} layers):"));

        for mw in &self.middlewares {
            let sc = if mw.can_short_circuit {
                " [short-circuit]"
            } else {
                ""
            };
            lines.push(format!("  {}. {}{}", mw.order, mw.name, sc));

            if mw.type_name != mw.name {
                lines.push(format!("     type: {}", mw.type_name));
            }

            if self.show_config {
                if let Some(config) = &mw.config_summary {
                    lines.push(format!("     config: {config}"));
                }
            }
        }

        lines.push(format!("  {total_layers}. [Handler]"));

        if self.show_flow && !self.middlewares.is_empty() {
            let request_flow: Vec<String> = (1..=total_layers).map(|n| n.to_string()).collect();
            let response_flow: Vec<String> =
                (1..=total_layers).rev().map(|n| n.to_string()).collect();
            lines.push(String::new());
            lines.push(format!("Request flow: {}", request_flow.join(" -> ")));
            lines.push(format!("Response flow: {}", response_flow.join(" -> ")));
        }

        lines
    }

    /// Return a plain text representation.
    #[must_use]
    pub fn as_plain_text(&self) -> String {
        self.plain_lines().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_contains, assert_no_ansi, capture};

    #[test]
    fn test_middleware_info_new() {
        let mw = MiddlewareInfo::new("RequestLogger", 1);
        assert_eq!(mw.name, "RequestLogger");
        assert_eq!(mw.order, 1);
        assert!(!mw.can_short_circuit);
        assert!(mw.config_summary.is_none());
    }

    #[test]
    fn test_middleware_info_with_config() {
        let mw = MiddlewareInfo::new("Cors", 2).with_config("origins=*");
        assert_eq!(mw.config_summary, Some("origins=*".to_string()));
    }

    #[test]
    fn test_middleware_info_short_circuits() {
        let mw = MiddlewareInfo::new("Auth", 3).short_circuits();
        assert!(mw.can_short_circuit);
    }

    #[test]
    fn test_stack_display_multiple_middlewares() {
        let middlewares = vec![
            MiddlewareInfo::new("Logger", 1),
            MiddlewareInfo::new("Cors", 2).with_config("origins=*"),
            MiddlewareInfo::new("Auth", 3).short_circuits(),
        ];
        let display = MiddlewareStackDisplay::new(middlewares);

        let captured = capture(OutputMode::Plain, || {
            let output = RichOutput::plain();
            display.render(&output);
        });

        assert_contains(&captured, "4 layers");
        assert_contains(&captured, "1. Logger");
        assert_contains(&captured, "2. Cors");
        assert_contains(&captured, "3. Auth");
        assert_contains(&captured, "[short-circuit]");
        assert_contains(&captured, "[Handler]");
    }

    #[test]
    fn test_stack_display_response_flow() {
        let middlewares = vec![MiddlewareInfo::new("A", 1), MiddlewareInfo::new("B", 2)];
        let display = MiddlewareStackDisplay::new(middlewares);

        let captured = capture(OutputMode::Plain, || {
            let output = RichOutput::plain();
            display.render(&output);
        });

        assert_contains(&captured, "Request flow:");
        assert_contains(&captured, "1 -> 2 -> 3");
        assert_contains(&captured, "Response flow:");
        assert_contains(&captured, "3 -> 2 -> 1");
    }

    #[test]
    fn test_stack_display_hide_config() {
        let middlewares = vec![MiddlewareInfo::new("Logger", 1).with_config("should-not-appear")];
        let display = MiddlewareStackDisplay::new(middlewares).hide_config();

        let captured = capture(OutputMode::Plain, || {
            let output = RichOutput::plain();
            display.render(&output);
        });

        assert!(!captured.contains("should-not-appear"));
    }

    #[test]
    fn test_stack_display_hide_flow() {
        let middlewares = vec![MiddlewareInfo::new("A", 1)];
        let display = MiddlewareStackDisplay::new(middlewares).hide_flow();

        let captured = capture(OutputMode::Plain, || {
            let output = RichOutput::plain();
            display.render(&output);
        });

        assert!(!captured.contains("Response flow"));
    }

    #[test]
    fn test_stack_display_no_ansi() {
        let middlewares = vec![MiddlewareInfo::new("Test", 1)];
        let display = MiddlewareStackDisplay::new(middlewares);

        let captured = capture(OutputMode::Plain, || {
            let output = RichOutput::plain();
            display.render(&output);
        });

        assert_no_ansi(&captured);
    }

    #[test]
    fn test_stack_display_as_plain_text() {
        let middlewares = vec![
            MiddlewareInfo::new("Logger", 1),
            MiddlewareInfo::new("Auth", 2).short_circuits(),
        ];
        let display = MiddlewareStackDisplay::new(middlewares);
        let text = display.as_plain_text();

        assert!(text.contains("3 layers"));
        assert!(text.contains("Logger"));
        assert!(text.contains("[short-circuit]"));
    }

    #[test]
    fn test_large_middleware_stack() {
        let middlewares: Vec<MiddlewareInfo> = (1..=10)
            .map(|i| MiddlewareInfo::new(&format!("Middleware{i}"), i))
            .collect();
        let display = MiddlewareStackDisplay::new(middlewares);

        let captured = capture(OutputMode::Plain, || {
            let output = RichOutput::plain();
            display.render(&output);
        });

        assert_contains(&captured, "11 layers");
        assert_contains(&captured, "Middleware10");
    }

    #[test]
    fn test_middleware_with_special_chars() {
        let mw = MiddlewareInfo::new("Custom<T>", 1).with_config("key=\"value\"");
        let display = MiddlewareStackDisplay::new(vec![mw]);
        let text = display.as_plain_text();

        assert!(text.contains("Custom<T>"));
        assert!(text.contains("key=\"value\""));
    }
}
