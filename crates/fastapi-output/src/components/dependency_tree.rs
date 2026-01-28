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
    use crate::testing::{assert_contains, assert_has_ansi, assert_no_ansi};

    // =================================================================
    // DependencyNode Builder Tests
    // =================================================================

    #[test]
    fn test_dependency_node_new() {
        let node = DependencyNode::new("TestService");
        assert_eq!(node.name, "TestService");
        assert!(node.children.is_empty());
        assert!(!node.cached);
        assert!(node.scope.is_none());
        assert!(node.note.is_none());
        assert!(!node.cycle);
    }

    #[test]
    fn test_dependency_node_child() {
        let node = DependencyNode::new("Parent")
            .child(DependencyNode::new("Child1"))
            .child(DependencyNode::new("Child2"));
        assert_eq!(node.children.len(), 2);
        assert_eq!(node.children[0].name, "Child1");
        assert_eq!(node.children[1].name, "Child2");
    }

    #[test]
    fn test_dependency_node_children() {
        let children = vec![
            DependencyNode::new("A"),
            DependencyNode::new("B"),
            DependencyNode::new("C"),
        ];
        let node = DependencyNode::new("Root").children(children);
        assert_eq!(node.children.len(), 3);
        assert_eq!(node.children[2].name, "C");
    }

    #[test]
    fn test_dependency_node_cached() {
        let node = DependencyNode::new("Cached").cached();
        assert!(node.cached);
    }

    #[test]
    fn test_dependency_node_scope() {
        let node = DependencyNode::new("Scoped").scope("singleton");
        assert_eq!(node.scope, Some("singleton".to_string()));
    }

    #[test]
    fn test_dependency_node_note() {
        let node = DependencyNode::new("Noted").note("Important service");
        assert_eq!(node.note, Some("Important service".to_string()));
    }

    #[test]
    fn test_dependency_node_cycle() {
        let node = DependencyNode::new("Circular").cycle();
        assert!(node.cycle);
    }

    #[test]
    fn test_dependency_node_full_builder() {
        let node = DependencyNode::new("FullService")
            .cached()
            .scope("request")
            .note("Main service entry")
            .cycle()
            .child(DependencyNode::new("Dep1"));

        assert_eq!(node.name, "FullService");
        assert!(node.cached);
        assert_eq!(node.scope, Some("request".to_string()));
        assert_eq!(node.note, Some("Main service entry".to_string()));
        assert!(node.cycle);
        assert_eq!(node.children.len(), 1);
    }

    // =================================================================
    // DependencyTreeDisplay Configuration Tests
    // =================================================================

    #[test]
    fn test_empty_roots() {
        let display = DependencyTreeDisplay::new(OutputMode::Plain, vec![]);
        let output = display.render();
        assert_eq!(output, "No dependencies registered.");
    }

    #[test]
    fn test_custom_title() {
        let roots = vec![DependencyNode::new("Service")];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots)
            .title(Some("Custom DI Tree".to_string()));
        let output = display.render();
        assert_contains(&output, "Custom DI Tree");
        assert!(!output.contains("Dependency Tree"));
    }

    #[test]
    fn test_no_title() {
        let roots = vec![DependencyNode::new("Service")];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots).title(None);
        let output = display.render();
        assert!(!output.contains("Dependency Tree"));
        assert_contains(&output, "Service");
    }

    #[test]
    fn test_hide_cached() {
        let roots = vec![DependencyNode::new("Cached").cached()];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots).hide_cached();
        let output = display.render();
        assert!(!output.contains("[cached]"));
    }

    #[test]
    fn test_hide_scopes() {
        let roots = vec![DependencyNode::new("Scoped").scope("singleton")];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots).hide_scopes();
        let output = display.render();
        assert!(!output.contains("scope:"));
    }

    #[test]
    fn test_hide_notes() {
        let roots = vec![DependencyNode::new("Noted").note("Important note")];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots).hide_notes();
        let output = display.render();
        assert!(!output.contains("Important note"));
    }

    // =================================================================
    // Rendering Mode Tests
    // =================================================================

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
        // Plain mode uses ASCII glyphs
        assert!(output.contains("+-") || output.contains("\\-"));
    }

    #[test]
    fn renders_rich_tree_with_ansi() {
        let roots = vec![DependencyNode::new("Service").cached().scope("request")];
        let display = DependencyTreeDisplay::new(OutputMode::Rich, roots);
        let output = display.render();

        assert_has_ansi(&output);
        assert_contains(&output, "Service");
        // Rich mode uses Unicode glyphs
        assert!(output.contains("├─") || output.contains("└─"));
    }

    #[test]
    fn renders_minimal_tree_with_unicode() {
        let roots = vec![DependencyNode::new("Service")];
        let display = DependencyTreeDisplay::new(OutputMode::Minimal, roots);
        let output = display.render();

        // Minimal uses same Unicode glyphs as Rich
        assert!(output.contains("└─"));
    }

    // =================================================================
    // Tree Structure Tests
    // =================================================================

    #[test]
    fn test_multiple_roots() {
        let roots = vec![
            DependencyNode::new("Root1"),
            DependencyNode::new("Root2"),
            DependencyNode::new("Root3"),
        ];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots);
        let output = display.render();

        assert_contains(&output, "Root1");
        assert_contains(&output, "Root2");
        assert_contains(&output, "Root3");
    }

    #[test]
    fn test_deep_nesting() {
        let roots = vec![
            DependencyNode::new("Level0").child(
                DependencyNode::new("Level1")
                    .child(DependencyNode::new("Level2").child(DependencyNode::new("Level3"))),
            ),
        ];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots);
        let output = display.render();

        assert_contains(&output, "Level0");
        assert_contains(&output, "Level1");
        assert_contains(&output, "Level2");
        assert_contains(&output, "Level3");
    }

    #[test]
    fn test_wide_tree() {
        let children = (0..5)
            .map(|i| DependencyNode::new(format!("Child{i}")))
            .collect();
        let roots = vec![DependencyNode::new("Root").children(children)];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots);
        let output = display.render();

        for i in 0..5 {
            assert_contains(&output, &format!("Child{i}"));
        }
    }

    // =================================================================
    // Cycle Detection Tests
    // =================================================================

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

    #[test]
    fn test_multiple_cycle_paths() {
        let roots = vec![
            DependencyNode::new("A")
                .child(DependencyNode::new("B").cycle())
                .child(DependencyNode::new("C").cycle()),
        ];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots)
            .with_cycle_path(vec!["A".into(), "B".into(), "A".into()])
            .with_cycle_path(vec!["A".into(), "C".into(), "A".into()]);
        let output = display.render();

        assert_contains(&output, "A -> B -> A");
        assert_contains(&output, "A -> C -> A");
    }

    #[test]
    fn test_empty_cycle_path_ignored() {
        let roots = vec![DependencyNode::new("Root")];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots).with_cycle_path(vec![]);
        let output = display.render();

        // Empty cycle path should not add cycles section
        assert!(!output.contains("Cycles detected:"));
    }

    // =================================================================
    // Label Formatting Tests
    // =================================================================

    #[test]
    fn test_node_with_all_annotations() {
        let roots = vec![
            DependencyNode::new("FullAnnotated")
                .cached()
                .scope("singleton")
                .note("Main dependency")
                .cycle(),
        ];
        let display = DependencyTreeDisplay::new(OutputMode::Plain, roots);
        let output = display.render();

        assert_contains(&output, "FullAnnotated");
        assert_contains(&output, "[cached]");
        assert_contains(&output, "(scope: singleton)");
        assert_contains(&output, "[cycle]");
        assert_contains(&output, "- Main dependency");
    }

    #[test]
    fn test_rich_mode_cycles_header_styled() {
        let roots = vec![DependencyNode::new("A").cycle()];
        let display = DependencyTreeDisplay::new(OutputMode::Rich, roots)
            .with_cycle_path(vec!["A".into(), "B".into()]);
        let output = display.render();

        // Cycles header should have ANSI codes in Rich mode
        assert_has_ansi(&output);
        assert_contains(&output, "Cycles detected:");
    }
}
