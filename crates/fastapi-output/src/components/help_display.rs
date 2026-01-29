//! Help and usage display component.
//!
//! Provides beautiful help text and usage information display with
//! consistent formatting across output modes.
//!
//! # Features
//!
//! - Command-line argument help
//! - Configuration option display
//! - Quick reference guides
//! - Version and about information

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;
use std::fmt::Write;

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";

/// A command-line argument or option.
#[derive(Debug, Clone)]
pub struct ArgInfo {
    /// Short name (e.g., "-h").
    pub short: Option<String>,
    /// Long name (e.g., "--help").
    pub long: Option<String>,
    /// Value placeholder (e.g., "\<PORT\>").
    pub value: Option<String>,
    /// Description.
    pub description: String,
    /// Default value.
    pub default: Option<String>,
    /// Whether required.
    pub required: bool,
    /// Environment variable name.
    pub env_var: Option<String>,
}

impl ArgInfo {
    /// Create a new argument with long name.
    #[must_use]
    pub fn new(long: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            short: None,
            long: Some(long.into()),
            value: None,
            description: description.into(),
            default: None,
            required: false,
            env_var: None,
        }
    }

    /// Create a positional argument.
    #[must_use]
    pub fn positional(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            short: None,
            long: None,
            value: Some(name.into()),
            description: description.into(),
            default: None,
            required: true,
            env_var: None,
        }
    }

    /// Set short name.
    #[must_use]
    pub fn short(mut self, short: impl Into<String>) -> Self {
        self.short = Some(short.into());
        self
    }

    /// Set value placeholder.
    #[must_use]
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Set default value.
    #[must_use]
    pub fn default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }

    /// Mark as required.
    #[must_use]
    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// Set environment variable.
    #[must_use]
    pub fn env(mut self, var: impl Into<String>) -> Self {
        self.env_var = Some(var.into());
        self
    }

    /// Get the full argument name for display.
    #[must_use]
    pub fn full_name(&self) -> String {
        let mut parts = Vec::new();
        if let Some(short) = &self.short {
            parts.push(short.clone());
        }
        if let Some(long) = &self.long {
            parts.push(long.clone());
        }
        if parts.is_empty() {
            if let Some(value) = &self.value {
                return value.clone();
            }
        }
        parts.join(", ")
    }
}

/// A group of related arguments.
#[derive(Debug, Clone)]
pub struct ArgGroup {
    /// Group name.
    pub name: String,
    /// Group description.
    pub description: Option<String>,
    /// Arguments in this group.
    pub args: Vec<ArgInfo>,
}

impl ArgGroup {
    /// Create a new argument group.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            args: Vec::new(),
        }
    }

    /// Set description.
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add an argument.
    #[must_use]
    pub fn arg(mut self, arg: ArgInfo) -> Self {
        self.args.push(arg);
        self
    }
}

/// Command information for help display.
#[derive(Debug, Clone)]
pub struct CommandInfo {
    /// Command name.
    pub name: String,
    /// Command description.
    pub description: String,
    /// Alias(es) for the command.
    pub aliases: Vec<String>,
}

impl CommandInfo {
    /// Create a new command.
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            aliases: Vec::new(),
        }
    }

    /// Add an alias.
    #[must_use]
    pub fn alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }
}

/// Help information for display.
#[derive(Debug, Clone)]
pub struct HelpInfo {
    /// Program name.
    pub name: String,
    /// Program version.
    pub version: Option<String>,
    /// Short description.
    pub about: Option<String>,
    /// Longer description.
    pub description: Option<String>,
    /// Usage string.
    pub usage: Option<String>,
    /// Argument groups.
    pub groups: Vec<ArgGroup>,
    /// Subcommands.
    pub commands: Vec<CommandInfo>,
    /// Examples.
    pub examples: Vec<(String, String)>,
    /// Author information.
    pub author: Option<String>,
    /// Additional notes.
    pub notes: Vec<String>,
}

impl HelpInfo {
    /// Create new help info.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: None,
            about: None,
            description: None,
            usage: None,
            groups: Vec::new(),
            commands: Vec::new(),
            examples: Vec::new(),
            author: None,
            notes: Vec::new(),
        }
    }

    /// Set version.
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set about text.
    #[must_use]
    pub fn about(mut self, about: impl Into<String>) -> Self {
        self.about = Some(about.into());
        self
    }

    /// Set description.
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set usage string.
    #[must_use]
    pub fn usage(mut self, usage: impl Into<String>) -> Self {
        self.usage = Some(usage.into());
        self
    }

    /// Add an argument group.
    #[must_use]
    pub fn group(mut self, group: ArgGroup) -> Self {
        self.groups.push(group);
        self
    }

    /// Add a command.
    #[must_use]
    pub fn command(mut self, cmd: CommandInfo) -> Self {
        self.commands.push(cmd);
        self
    }

    /// Add an example.
    #[must_use]
    pub fn example(mut self, cmd: impl Into<String>, desc: impl Into<String>) -> Self {
        self.examples.push((cmd.into(), desc.into()));
        self
    }

    /// Set author.
    #[must_use]
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Add a note.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

/// Help display formatter.
#[derive(Debug, Clone)]
pub struct HelpDisplay {
    mode: OutputMode,
    theme: FastApiTheme,
    /// Maximum width for wrapping.
    pub max_width: usize,
    /// Show environment variables.
    pub show_env_vars: bool,
    /// Show default values.
    pub show_defaults: bool,
}

impl HelpDisplay {
    /// Create a new help display.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            max_width: 80,
            show_env_vars: true,
            show_defaults: true,
        }
    }

    /// Set the theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Render help information.
    #[must_use]
    pub fn render(&self, help: &HelpInfo) -> String {
        match self.mode {
            OutputMode::Plain => self.render_plain(help),
            OutputMode::Minimal => self.render_minimal(help),
            OutputMode::Rich => self.render_rich(help),
        }
    }

    fn render_plain(&self, help: &HelpInfo) -> String {
        let mut lines = Vec::new();

        // Header
        let mut header = help.name.clone();
        if let Some(version) = &help.version {
            let _ = write!(header, " {version}");
        }
        lines.push(header);

        if let Some(about) = &help.about {
            lines.push(about.clone());
        }

        // Usage
        if let Some(usage) = &help.usage {
            lines.push(String::new());
            lines.push("USAGE:".to_string());
            lines.push(format!("    {usage}"));
        }

        // Description
        if let Some(desc) = &help.description {
            lines.push(String::new());
            for line in Self::wrap_text(desc, self.max_width) {
                lines.push(line);
            }
        }

        // Argument groups
        for group in &help.groups {
            lines.push(String::new());
            lines.push(format!("{}:", group.name.to_uppercase()));

            for arg in &group.args {
                let name = arg.full_name();
                let value_part = arg
                    .value
                    .as_ref()
                    .map(|v| format!(" {v}"))
                    .unwrap_or_default();

                let mut line = format!("    {name}{value_part}");

                // Pad to align descriptions
                let padding = 30_usize.saturating_sub(line.len());
                line.push_str(&" ".repeat(padding));
                line.push_str(&arg.description);

                if self.show_defaults {
                    if let Some(default) = &arg.default {
                        let _ = write!(line, " [default: {default}]");
                    }
                }

                if self.show_env_vars {
                    if let Some(env) = &arg.env_var {
                        let _ = write!(line, " [env: {env}]");
                    }
                }

                lines.push(line);
            }
        }

        // Subcommands
        if !help.commands.is_empty() {
            lines.push(String::new());
            lines.push("COMMANDS:".to_string());

            for cmd in &help.commands {
                let aliases = if cmd.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", cmd.aliases.join(", "))
                };

                let mut line = format!("    {}{aliases}", cmd.name);
                let padding = 30_usize.saturating_sub(line.len());
                line.push_str(&" ".repeat(padding));
                line.push_str(&cmd.description);
                lines.push(line);
            }
        }

        // Examples
        if !help.examples.is_empty() {
            lines.push(String::new());
            lines.push("EXAMPLES:".to_string());
            for (cmd, desc) in &help.examples {
                lines.push(format!("    $ {cmd}"));
                lines.push(format!("      {desc}"));
                lines.push(String::new());
            }
        }

        // Notes
        for note in &help.notes {
            lines.push(String::new());
            lines.push(format!("NOTE: {note}"));
        }

        lines.join("\n")
    }

    fn render_minimal(&self, help: &HelpInfo) -> String {
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let header = self.theme.header.to_ansi_fg();
        let success = self.theme.success.to_ansi_fg();

        let mut lines = Vec::new();

        // Header
        let mut header_line = format!("{header}{ANSI_BOLD}{}{ANSI_RESET}", help.name);
        if let Some(version) = &help.version {
            let _ = write!(header_line, " {muted}{version}{ANSI_RESET}");
        }
        lines.push(header_line);

        if let Some(about) = &help.about {
            lines.push(format!("{muted}{about}{ANSI_RESET}"));
        }

        // Usage
        if let Some(usage) = &help.usage {
            lines.push(String::new());
            lines.push(format!("{header}USAGE:{ANSI_RESET}"));
            lines.push(format!("    {accent}{usage}{ANSI_RESET}"));
        }

        // Argument groups
        for group in &help.groups {
            lines.push(String::new());
            lines.push(format!(
                "{header}{}:{ANSI_RESET}",
                group.name.to_uppercase()
            ));

            for arg in &group.args {
                let name = arg.full_name();
                let value_part = arg
                    .value
                    .as_ref()
                    .map(|v| format!(" {accent}{v}{ANSI_RESET}"))
                    .unwrap_or_default();

                let line = format!("    {success}{name}{ANSI_RESET}{value_part}");
                lines.push(line);
                lines.push(format!("        {muted}{}{ANSI_RESET}", arg.description));

                if self.show_defaults {
                    if let Some(default) = &arg.default {
                        lines.push(format!("        {muted}Default: {default}{ANSI_RESET}"));
                    }
                }
            }
        }

        // Commands
        if !help.commands.is_empty() {
            lines.push(String::new());
            lines.push(format!("{header}COMMANDS:{ANSI_RESET}"));

            for cmd in &help.commands {
                lines.push(format!(
                    "    {success}{}{ANSI_RESET}  {muted}{}{ANSI_RESET}",
                    cmd.name, cmd.description
                ));
            }
        }

        lines.join("\n")
    }

    #[allow(clippy::too_many_lines)]
    fn render_rich(&self, help: &HelpInfo) -> String {
        let muted = self.theme.muted.to_ansi_fg();
        let accent = self.theme.accent.to_ansi_fg();
        let border = self.theme.border.to_ansi_fg();
        let header_style = self.theme.header.to_ansi_fg();
        let success = self.theme.success.to_ansi_fg();
        let info = self.theme.info.to_ansi_fg();

        let mut lines = Vec::new();

        // Title box
        let title_width = 60;
        lines.push(format!("{border}┌{}┐{ANSI_RESET}", "─".repeat(title_width)));

        // Name and version
        let mut name_line = format!("{ANSI_BOLD}{}{ANSI_RESET}", help.name);
        if let Some(version) = &help.version {
            let _ = write!(name_line, " {muted}v{version}{ANSI_RESET}");
        }
        let name_pad =
            (title_width - help.name.len() - help.version.as_ref().map_or(0, |v| v.len() + 2)) / 2;
        lines.push(format!(
            "{border}│{ANSI_RESET}{}{}{}",
            " ".repeat(name_pad),
            name_line,
            " ".repeat(
                title_width
                    - name_pad
                    - help.name.len()
                    - help.version.as_ref().map_or(0, |v| v.len() + 2)
            )
        ));

        if let Some(about) = &help.about {
            let about_pad = (title_width - about.len()) / 2;
            lines.push(format!(
                "{border}│{ANSI_RESET}{}{muted}{about}{ANSI_RESET}{}",
                " ".repeat(about_pad.max(1)),
                " ".repeat((title_width - about_pad - about.len()).max(1))
            ));
        }

        lines.push(format!("{border}└{}┘{ANSI_RESET}", "─".repeat(title_width)));

        // Usage section
        if let Some(usage) = &help.usage {
            lines.push(String::new());
            lines.push(format!("{header_style}{ANSI_BOLD}USAGE{ANSI_RESET}"));
            lines.push(format!("  {accent}${ANSI_RESET} {usage}"));
        }

        // Arguments
        for group in &help.groups {
            lines.push(String::new());
            lines.push(format!(
                "{header_style}{ANSI_BOLD}{}{ANSI_RESET}",
                group.name.to_uppercase()
            ));

            for arg in &group.args {
                let short = arg
                    .short
                    .as_ref()
                    .map(|s| format!("{success}{s}{ANSI_RESET}, "))
                    .unwrap_or_default();
                let long = arg
                    .long
                    .as_ref()
                    .map(|l| format!("{success}{l}{ANSI_RESET}"))
                    .unwrap_or_default();
                let value = arg
                    .value
                    .as_ref()
                    .map(|v| format!(" {accent}{v}{ANSI_RESET}"))
                    .unwrap_or_default();

                lines.push(format!("  {short}{long}{value}"));
                lines.push(format!("      {muted}{}{ANSI_RESET}", arg.description));

                let mut meta_parts = Vec::new();
                if self.show_defaults {
                    if let Some(default) = &arg.default {
                        meta_parts.push(format!("default: {info}{default}{ANSI_RESET}"));
                    }
                }
                if self.show_env_vars {
                    if let Some(env) = &arg.env_var {
                        meta_parts.push(format!("env: {info}{env}{ANSI_RESET}"));
                    }
                }
                if !meta_parts.is_empty() {
                    lines.push(format!(
                        "      {muted}[{}]{ANSI_RESET}",
                        meta_parts.join(", ")
                    ));
                }
            }
        }

        // Commands
        if !help.commands.is_empty() {
            lines.push(String::new());
            lines.push(format!("{header_style}{ANSI_BOLD}COMMANDS{ANSI_RESET}"));

            for cmd in &help.commands {
                let aliases = if cmd.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" {muted}({}){ANSI_RESET}", cmd.aliases.join(", "))
                };
                lines.push(format!("  {success}{}{ANSI_RESET}{aliases}", cmd.name));
                lines.push(format!("      {muted}{}{ANSI_RESET}", cmd.description));
            }
        }

        // Examples
        if !help.examples.is_empty() {
            lines.push(String::new());
            lines.push(format!("{header_style}{ANSI_BOLD}EXAMPLES{ANSI_RESET}"));

            for (cmd, desc) in &help.examples {
                lines.push(format!("  {accent}${ANSI_RESET} {cmd}"));
                lines.push(format!("    {muted}{desc}{ANSI_RESET}"));
            }
        }

        lines.join("\n")
    }

    fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current_line = String::new();

        for word in text.split_whitespace() {
            if current_line.is_empty() {
                current_line = word.to_string();
            } else if current_line.len() + 1 + word.len() <= max_width {
                current_line.push(' ');
                current_line.push_str(word);
            } else {
                lines.push(current_line);
                current_line = word.to_string();
            }
        }

        if !current_line.is_empty() {
            lines.push(current_line);
        }

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_help() -> HelpInfo {
        HelpInfo::new("myapp")
            .version("1.0.0")
            .about("A sample CLI application")
            .usage("myapp [OPTIONS] <COMMAND>")
            .group(
                ArgGroup::new("Options")
                    .arg(
                        ArgInfo::new("--host", "Host to bind to")
                            .short("-h")
                            .value("<HOST>")
                            .default("127.0.0.1")
                            .env("MYAPP_HOST"),
                    )
                    .arg(
                        ArgInfo::new("--port", "Port to listen on")
                            .short("-p")
                            .value("<PORT>")
                            .default("8000")
                            .env("MYAPP_PORT"),
                    )
                    .arg(ArgInfo::new("--verbose", "Enable verbose output").short("-v")),
            )
            .command(CommandInfo::new("serve", "Start the server").alias("s"))
            .command(CommandInfo::new("init", "Initialize configuration"))
            .example("myapp serve --port 3000", "Start server on port 3000")
            .example("myapp init", "Create default configuration")
    }

    #[test]
    fn test_arg_info_builder() {
        let arg = ArgInfo::new("--config", "Configuration file path")
            .short("-c")
            .value("<FILE>")
            .default("config.toml")
            .env("MYAPP_CONFIG");

        assert_eq!(arg.long, Some("--config".to_string()));
        assert_eq!(arg.short, Some("-c".to_string()));
        assert_eq!(arg.value, Some("<FILE>".to_string()));
    }

    #[test]
    fn test_arg_full_name() {
        let arg = ArgInfo::new("--verbose", "Enable verbose").short("-v");
        assert_eq!(arg.full_name(), "-v, --verbose");

        let positional = ArgInfo::positional("<INPUT>", "Input file");
        assert_eq!(positional.full_name(), "<INPUT>");
    }

    #[test]
    fn test_help_display_plain() {
        let display = HelpDisplay::new(OutputMode::Plain);
        let output = display.render(&sample_help());

        assert!(output.contains("myapp"));
        assert!(output.contains("1.0.0"));
        assert!(output.contains("USAGE:"));
        assert!(output.contains("--host"));
        assert!(output.contains("--port"));
        assert!(output.contains("COMMANDS:"));
        assert!(output.contains("serve"));
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn test_help_display_rich_has_ansi() {
        let display = HelpDisplay::new(OutputMode::Rich);
        let output = display.render(&sample_help());

        assert!(output.contains("\x1b["));
        assert!(output.contains("myapp"));
    }

    #[test]
    fn test_command_info_builder() {
        let cmd = CommandInfo::new("build", "Build the project")
            .alias("b")
            .alias("compile");

        assert_eq!(cmd.name, "build");
        assert_eq!(cmd.aliases, vec!["b", "compile"]);
    }
}
