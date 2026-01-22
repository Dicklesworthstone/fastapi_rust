//! Graceful shutdown progress indicator component.
//!
//! Displays progress for connection draining, background task completion,
//! and cleanup stages with agent-friendly fallback output.

use crate::mode::OutputMode;
use crate::themes::FastApiTheme;

const ANSI_RESET: &str = "\x1b[0m";

/// Shutdown phase indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownPhase {
    /// Grace period while draining connections.
    GracePeriod,
    /// Force-close phase after grace timeout.
    ForceClose,
    /// Shutdown complete.
    Complete,
}

impl ShutdownPhase {
    /// Return a human-readable label for the phase.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GracePeriod => "Grace Period",
            Self::ForceClose => "Force Close",
            Self::Complete => "Complete",
        }
    }

    fn color(self, theme: &FastApiTheme) -> crate::themes::Color {
        match self {
            Self::GracePeriod => theme.info,
            Self::ForceClose => theme.error,
            Self::Complete => theme.success,
        }
    }
}

/// Shutdown progress snapshot.
#[derive(Debug, Clone)]
pub struct ShutdownProgress {
    /// Current shutdown phase.
    pub phase: ShutdownPhase,
    /// Total active connections at start.
    pub total_connections: usize,
    /// Connections drained so far.
    pub drained_connections: usize,
    /// In-flight requests remaining.
    pub in_flight_requests: usize,
    /// Background tasks still running.
    pub background_tasks: usize,
    /// Cleanup steps completed.
    pub cleanup_done: usize,
    /// Total cleanup steps.
    pub cleanup_total: usize,
    /// Optional notes for extra context.
    pub notes: Vec<String>,
}

impl ShutdownProgress {
    /// Create a new shutdown progress snapshot.
    #[must_use]
    pub fn new(phase: ShutdownPhase) -> Self {
        Self {
            phase,
            total_connections: 0,
            drained_connections: 0,
            in_flight_requests: 0,
            background_tasks: 0,
            cleanup_done: 0,
            cleanup_total: 0,
            notes: Vec::new(),
        }
    }

    /// Set connection counts.
    #[must_use]
    pub fn connections(mut self, drained: usize, total: usize) -> Self {
        self.drained_connections = drained;
        self.total_connections = total;
        self
    }

    /// Set in-flight request count.
    #[must_use]
    pub fn in_flight(mut self, in_flight: usize) -> Self {
        self.in_flight_requests = in_flight;
        self
    }

    /// Set background task count.
    #[must_use]
    pub fn background_tasks(mut self, tasks: usize) -> Self {
        self.background_tasks = tasks;
        self
    }

    /// Set cleanup step counts.
    #[must_use]
    pub fn cleanup(mut self, done: usize, total: usize) -> Self {
        self.cleanup_done = done;
        self.cleanup_total = total;
        self
    }

    /// Add a note line.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

/// Shutdown progress display.
#[derive(Debug, Clone)]
pub struct ShutdownProgressDisplay {
    mode: OutputMode,
    theme: FastApiTheme,
    progress_width: usize,
    title: Option<String>,
}

impl ShutdownProgressDisplay {
    /// Create a new shutdown progress display.
    #[must_use]
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            theme: FastApiTheme::default(),
            progress_width: 24,
            title: Some("Shutdown Progress".to_string()),
        }
    }

    /// Set a custom theme.
    #[must_use]
    pub fn theme(mut self, theme: FastApiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Set a custom progress bar width.
    #[must_use]
    pub fn progress_width(mut self, width: usize) -> Self {
        self.progress_width = width.max(8);
        self
    }

    /// Set a custom title (None to disable).
    #[must_use]
    pub fn title(mut self, title: Option<String>) -> Self {
        self.title = title;
        self
    }

    /// Render the progress snapshot.
    #[must_use]
    pub fn render(&self, progress: &ShutdownProgress) -> String {
        let mut lines = Vec::new();

        if let Some(title) = &self.title {
            lines.push(title.clone());
            lines.push("-".repeat(title.len()));
        }

        lines.push(self.render_phase(progress.phase));

        if progress.total_connections > 0 {
            lines.push(self.render_connections(progress));
        } else {
            lines.push("Connections: none".to_string());
        }

        if progress.in_flight_requests > 0 {
            lines.push(format!(
                "In-flight requests: {}",
                progress.in_flight_requests
            ));
        }

        if progress.background_tasks > 0 {
            lines.push(format!("Background tasks: {}", progress.background_tasks));
        }

        if progress.cleanup_total > 0 {
            lines.push(format!(
                "Cleanup: {}/{} steps",
                progress.cleanup_done, progress.cleanup_total
            ));
        }

        for note in &progress.notes {
            lines.push(format!("Note: {note}"));
        }

        if progress.phase == ShutdownPhase::Complete {
            lines.push(self.render_complete());
        }

        lines.join("\n")
    }

    fn render_phase(&self, phase: ShutdownPhase) -> String {
        if self.mode.uses_ansi() {
            let mut line = format!(
                "{}Phase:{} {}{}",
                self.theme.muted.to_ansi_fg(),
                ANSI_RESET,
                phase.color(&self.theme).to_ansi_fg(),
                phase.label()
            );
            line.push_str(ANSI_RESET);
            line
        } else {
            format!("Phase: {}", phase.label())
        }
    }

    fn render_connections(&self, progress: &ShutdownProgress) -> String {
        let bar = shutdown_bar(
            progress.drained_connections,
            progress.total_connections,
            self.progress_width,
            self.mode,
            &self.theme,
        );
        format!(
            "Connections: {}/{} drained {bar}",
            progress.drained_connections, progress.total_connections
        )
    }

    fn render_complete(&self) -> String {
        if self.mode.uses_ansi() {
            format!(
                "{}Shutdown complete{}",
                self.theme.success.to_ansi_fg(),
                ANSI_RESET
            )
        } else {
            "Shutdown complete".to_string()
        }
    }
}

fn shutdown_bar(
    drained: usize,
    total: usize,
    width: usize,
    mode: OutputMode,
    theme: &FastApiTheme,
) -> String {
    if total == 0 {
        return String::new();
    }

    let width = width.max(8);
    let filled = drained.saturating_mul(width) / total;
    let filled = filled.min(width);
    let remaining = width.saturating_sub(filled);

    let mut bar = String::new();
    bar.push('[');

    if mode.uses_ansi() {
        if filled > 0 {
            bar.push_str(&theme.success.to_ansi_fg());
            bar.push_str(&"#".repeat(filled));
            bar.push_str(ANSI_RESET);
        }
        if remaining > 0 {
            bar.push_str(&theme.muted.to_ansi_fg());
            bar.push_str(&"-".repeat(remaining));
            bar.push_str(ANSI_RESET);
        }
    } else {
        bar.push_str(&"#".repeat(filled));
        bar.push_str(&"-".repeat(remaining));
    }

    bar.push(']');
    bar
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_contains, assert_no_ansi};

    #[test]
    fn renders_plain_shutdown_progress() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod)
            .connections(3, 10)
            .in_flight(2)
            .background_tasks(1)
            .cleanup(1, 3)
            .note("Waiting for DB pool");
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);

        assert_contains(&output, "Shutdown Progress");
        assert_contains(&output, "Phase: Grace Period");
        assert_contains(&output, "Connections: 3/10 drained");
        assert_contains(&output, "In-flight requests: 2");
        assert_contains(&output, "Background tasks: 1");
        assert_contains(&output, "Cleanup: 1/3 steps");
        assert_contains(&output, "Note: Waiting for DB pool");
        assert_no_ansi(&output);
    }

    #[test]
    fn renders_complete_phase() {
        let progress = ShutdownProgress::new(ShutdownPhase::Complete);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);
        assert_contains(&output, "Shutdown complete");
    }
}
