//! Dependency injection tree display component.
//!
//! Renders a hierarchical view of dependencies with optional
//! cycle highlighting and scope/caching annotations.

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;

const ANSI_RESET: &str = "\x1b[0m";

/// A single dependency node in the tree.
#[derive(Debug, Clone)]
pub struct DependencyNode {
    /// Display name of the dependency.
    pub name: String,
    /// Child dependencies.
    pub children: Vec<DependencyNode>,
    /// Whether the dependency is cached for the request.
    pub cached: bool,
    /// Optional scope label (e.g., "request", "function").
    pub scope: Option<String>,
    /// Optional note or detail.
    pub note: Option<String>,
    /// Whether this node represents a cycle edge.
    pub cycle: bool,
}

impl DependencyNode {
    /// Create a new dependency node with no children.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            children: Vec::new(),
            cached: false,
            scope: None,
            note: None,
            cycle: false,
        }
    }

    /// Add a child dependency.
    #[must_use]
    pub fn child(mut self, child: DependencyNode) -> Self {
        self.children.push(child);
        self
    }

    /// Replace children for the node.
    #[must_use]
    pub fn children(mut self, children: Vec<DependencyNode>) -> Self {
        self.children = children;
        self
    }

    /// Mark this dependency as cached.
    #[must_use]
    pub fn cached(mut self) -> Self {
        self.cached = true;
        self
    }

    /// Set the dependency scope label.
    #[must_use]
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// Add a note to this dependency.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Mark this node as a cycle edge.
    #[must_use]
    pub fn cycle(mut self) -> Self {
        self.cycle = true;
        self
    }
}

/// Display configuration for dependency trees.
#[derive(Debug, Clone)]
pub struct DependencyTreeDisplay {
    mode: OutputMode,
    theme: FastApiTheme,
    roots: Vec<DependencyNode>,
    show_cached: bool,
    show_scopes: bool,
    show_notes: bool,
    title: Option<String>,
    cycle_paths: Vec<Vec<String>>,
}

impl DependencyTreeDisplay {
    /// Create a new dependency tree display.
    #[must_use]
    pub fn new(mode: OutputMode, roots: Vec<DependencyNode>) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            roots,
            show_cached: true,
            show_scopes: true,
            show_notes: true,
            title: Some("Dependency Tree".to_string()),
            cycle_paths: Vec::new(),
        }
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Hide cached markers.
    #[must_use]
    pub fn hide_cached(mut self) -> Self {
        self.show_cached = false;
        self
    }

    /// Hide scope labels.
    #[must_use]
    pub fn hide_scopes(mut self) -> Self {
        self.show_scopes = false;
        self
    }

    /// Hide notes.
    #[must_use]
    pub fn hide_notes(mut self) -> Self {
        self.show_notes = false;
        self
    }

    /// Set a custom title (None to disable).
    #[must_use]
    pub fn title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    /// Add a cycle path for summary output.
    #[must_use]
    pub fn with_cycle_path(mut self, path: Vec<String>) -> Self {
        if !path.is_empty() {
            self.cycle_paths.push(path);
        }
        self
    }

    /// Render the dependency tree to a string.
    #[must_use]
    pub fn render(&self) -> String {
        if self.roots.is_empty() {
            return "No dependencies registered.".to_string();
        }

        let glyphs = TreeGlyphs::for_mode(self.mode);
        let mut lines = Vec::new();

        if let Some(title) = &self.title {
            lines.push(title.clone());
            lines.push("-".repeat(title.len()));
        }

        for (idx, root) in self.roots.iter().enumerate() {
            let is_last = idx + 1 == self.roots.len();
            self.render_node(&mut lines, "", root, is_last, &glyphs);
        }

        if !self.cycle_paths.is_empty() {
            lines.push(String::new());
            lines.push(self.render_cycles_header());
            for cycle in &self.cycle_paths {
                lines.push(format!("  {}", cycle.join(" -> ")));
            }
        }

        lines.join("\n")
    }

    fn render_cycles_header(&self) -> String {
        if self.mode.uses_ansi() {
            let error = self.theme.error.to_ansi_fg();
            format!("{error}Cycles detected:{ANSI_RESET}")
        } else {
            "Cycles detected:".to_string()
        }
    }

    fn render_node(
        &self,
        lines: &mut Vec<String>,
        prefix: &str,
        node: &DependencyNode,
        is_last: bool,
        glyphs: &TreeGlyphs,
    ) {
        let connector = if is_last { glyphs.last } else { glyphs.branch };
        let label = self.render_label(node);
        lines.push(format!("{prefix}{connector} {label}"));

        let next_prefix = if is_last {
            format!("{prefix}{}", glyphs.spacer)
        } else {
            format!("{prefix}{}", glyphs.vertical)
        };

        for (idx, child) in node.children.iter().enumerate() {
            let child_is_last = idx + 1 == node.children.len();
            self.render_node(lines, &next_prefix, child, child_is_last, glyphs);
        }
    }

    fn render_label(&self, node: &DependencyNode) -> String {
        let mut parts = Vec::new();
        let name = if self.mode.uses_ansi() {
            format!(
                "{}{}{}",
                self.theme.primary.to_ansi_fg(),
                node.name,
                ANSI_RESET
            )
        } else {
            node.name.clone()
        };
        parts.push(name);

        if self.show_cached && node.cached {
            let cached = if self.mode.uses_ansi() {
                format!("{}[cached]{}", self.theme.muted.to_ansi_fg(), ANSI_RESET)
            } else {
                "[cached]".to_string()
            };
            parts.push(cached);
        }

        if self.show_scopes {
            if let Some(scope) = &node.scope {
                let scope_text = if self.mode.uses_ansi() {
                    format!(
                        "{}(scope: {}){}",
                        self.theme.secondary.to_ansi_fg(),
                        scope,
                        ANSI_RESET
                    )
                } else {
                    format!("(scope: {scope})")
                };
                parts.push(scope_text);
            }
        }

        if node.cycle {
            let cycle = if self.mode.uses_ansi() {
                format!("{}[cycle]{}", self.theme.error.to_ansi_fg(), ANSI_RESET)
            } else {
                "[cycle]".to_string()
            };
            parts.push(cycle);
        }

        if self.show_notes {
            if let Some(note) = &node.note {
                parts.push(format!("- {note}"));
            }
        }

        parts.join(" ")
    }
}

struct TreeGlyphs {
    branch: &'static str,
    last: &'static str,
    vertical: &'static str,
    spacer: &'static str,
}

impl TreeGlyphs {
    fn for_mode(mode: OutputMode) -> Self {
        match mode {
            OutputMode::Plain => Self {
                branch: "+-",
                last: "\\-",
                vertical: "| ",
                spacer: "  ",
            },
            OutputMode::Minimal | OutputMode::Rich => Self {
                branch: "├─",
                last: "└─",
                vertical: "│ ",
                spacer: "  ",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_contains, assert_no_ansi};

    #[test]
    fn renders_plain_tree() {
        let roots = vec![
            DependencyNode::new("Database")
                .cached()
                .scope("request")
                .child(DependencyNode::new("Config")),
        ];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots);
        let output = display.render();

        assert_contains(&output, "Dependency Tree");
        assert_contains(&output, "Database");
        assert_contains(&output, "[cached]");
        assert_contains(&output, "scope: request");
        assert_contains(&output, "Config");
        assert_no_ansi(&output);
    }

    #[test]
    fn renders_cycle_marker() {
        let roots = vec![
            DependencyNode::new("Auth")
                .child(DependencyNode::new("Db").cycle())
                .child(DependencyNode::new("Cache")),
        ];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots).with_cycle_path(vec![
            "Auth".into(),
            "Db".into(),
            "Auth".into(),
        ]);
        let output = display.render();

        assert_contains(&output, "[cycle]");
        assert_contains(&output, "Cycles detected:");
        assert_contains(&output, "Auth -> Db -> Auth");
    }
}
