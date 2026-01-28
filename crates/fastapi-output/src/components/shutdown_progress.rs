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
    use crate::testing::{assert_contains, assert_has_ansi, assert_no_ansi};

    // =================================================================
    // ShutdownPhase Tests
    // =================================================================

    #[test]
    fn test_shutdown_phase_labels() {
        assert_eq!(ShutdownPhase::GracePeriod.label(), "Grace Period");
        assert_eq!(ShutdownPhase::ForceClose.label(), "Force Close");
        assert_eq!(ShutdownPhase::Complete.label(), "Complete");
    }

    #[test]
    fn test_shutdown_phase_equality() {
        assert_eq!(ShutdownPhase::GracePeriod, ShutdownPhase::GracePeriod);
        assert_ne!(ShutdownPhase::GracePeriod, ShutdownPhase::ForceClose);
        assert_ne!(ShutdownPhase::ForceClose, ShutdownPhase::Complete);
    }

    // =================================================================
    // ShutdownProgress Builder Tests
    // =================================================================

    #[test]
    fn test_shutdown_progress_new() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod);
        assert_eq!(progress.phase, ShutdownPhase::GracePeriod);
        assert_eq!(progress.total_connections, 0);
        assert_eq!(progress.drained_connections, 0);
        assert_eq!(progress.in_flight_requests, 0);
        assert_eq!(progress.background_tasks, 0);
        assert_eq!(progress.cleanup_done, 0);
        assert_eq!(progress.cleanup_total, 0);
        assert!(progress.notes.is_empty());
    }

    #[test]
    fn test_shutdown_progress_connections() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(5, 10);
        assert_eq!(progress.drained_connections, 5);
        assert_eq!(progress.total_connections, 10);
    }

    #[test]
    fn test_shutdown_progress_in_flight() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).in_flight(3);
        assert_eq!(progress.in_flight_requests, 3);
    }

    #[test]
    fn test_shutdown_progress_background_tasks() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).background_tasks(2);
        assert_eq!(progress.background_tasks, 2);
    }

    #[test]
    fn test_shutdown_progress_cleanup() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).cleanup(1, 5);
        assert_eq!(progress.cleanup_done, 1);
        assert_eq!(progress.cleanup_total, 5);
    }

    #[test]
    fn test_shutdown_progress_note() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod)
            .note("First note")
            .note("Second note");
        assert_eq!(progress.notes.len(), 2);
        assert_eq!(progress.notes[0], "First note");
        assert_eq!(progress.notes[1], "Second note");
    }

    #[test]
    fn test_shutdown_progress_full_builder() {
        let progress = ShutdownProgress::new(ShutdownPhase::ForceClose)
            .connections(8, 10)
            .in_flight(1)
            .background_tasks(2)
            .cleanup(3, 4)
            .note("Forcing connections");

        assert_eq!(progress.phase, ShutdownPhase::ForceClose);
        assert_eq!(progress.drained_connections, 8);
        assert_eq!(progress.total_connections, 10);
        assert_eq!(progress.in_flight_requests, 1);
        assert_eq!(progress.background_tasks, 2);
        assert_eq!(progress.cleanup_done, 3);
        assert_eq!(progress.cleanup_total, 4);
        assert_eq!(progress.notes.len(), 1);
    }

    // =================================================================
    // ShutdownProgressDisplay Configuration Tests
    // =================================================================

    #[test]
    fn test_display_custom_title() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain)
            .title(Some("Server Shutdown".to_string()));
        let output = display.render(&progress);

        assert_contains(&output, "Server Shutdown");
        assert!(!output.contains("Shutdown Progress"));
    }

    #[test]
    fn test_display_no_title() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain).title(None);
        let output = display.render(&progress);

        assert!(!output.contains("Shutdown Progress"));
        assert_contains(&output, "Phase:");
    }

    #[test]
    fn test_display_progress_width() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(5, 10);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain).progress_width(10);
        let output = display.render(&progress);

        // Bar should be visible with specified width
        assert_contains(&output, "[#####-----]");
    }

    #[test]
    fn test_display_progress_width_minimum() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(4, 8);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain).progress_width(2); // Should use min of 8
        let output = display.render(&progress);

        // Should use minimum width of 8
        assert!(output.contains("[####----]"));
    }

    // =================================================================
    // Rendering Mode Tests
    // =================================================================

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
    fn renders_rich_shutdown_progress() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(5, 10);
        let display = ShutdownProgressDisplay::new(OutputMode::Rich);
        let output = display.render(&progress);

        assert_has_ansi(&output);
        assert_contains(&output, "Grace Period");
        assert_contains(&output, "5/10 drained");
    }

    #[test]
    fn renders_complete_phase() {
        let progress = ShutdownProgress::new(ShutdownPhase::Complete);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);
        assert_contains(&output, "Shutdown complete");
    }

    #[test]
    fn renders_rich_complete_phase_with_ansi() {
        let progress = ShutdownProgress::new(ShutdownPhase::Complete);
        let display = ShutdownProgressDisplay::new(OutputMode::Rich);
        let output = display.render(&progress);

        assert_has_ansi(&output);
        assert_contains(&output, "Shutdown complete");
    }

    // =================================================================
    // Phase Rendering Tests
    // =================================================================

    #[test]
    fn test_grace_period_phase() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);

        assert_contains(&output, "Phase: Grace Period");
    }

    #[test]
    fn test_force_close_phase() {
        let progress = ShutdownProgress::new(ShutdownPhase::ForceClose);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);

        assert_contains(&output, "Phase: Force Close");
    }

    // =================================================================
    // Edge Cases Tests
    // =================================================================

    #[test]
    fn test_no_connections() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);

        assert_contains(&output, "Connections: none");
    }

    #[test]
    fn test_zero_total_connections() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(0, 0);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);

        // No bar when total is 0
        assert_contains(&output, "Connections: none");
    }

    #[test]
    fn test_all_connections_drained() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(10, 10);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain).progress_width(10);
        let output = display.render(&progress);

        assert_contains(&output, "10/10 drained");
        assert_contains(&output, "[##########]");
    }

    #[test]
    fn test_no_in_flight_requests_omitted() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).in_flight(0);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);

        assert!(!output.contains("In-flight"));
    }

    #[test]
    fn test_no_background_tasks_omitted() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).background_tasks(0);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);

        assert!(!output.contains("Background tasks"));
    }

    #[test]
    fn test_no_cleanup_omitted() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).cleanup(0, 0);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);

        assert!(!output.contains("Cleanup:"));
    }

    #[test]
    fn test_multiple_notes() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod)
            .note("Note 1")
            .note("Note 2")
            .note("Note 3");
        let display = ShutdownProgressDisplay::new(OutputMode::Plain);
        let output = display.render(&progress);

        assert_contains(&output, "Note: Note 1");
        assert_contains(&output, "Note: Note 2");
        assert_contains(&output, "Note: Note 3");
    }

    // =================================================================
    // Progress Bar Tests
    // =================================================================

    #[test]
    fn test_progress_bar_empty() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(0, 10);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain).progress_width(10);
        let output = display.render(&progress);

        assert_contains(&output, "[----------]");
    }

    #[test]
    fn test_progress_bar_half() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(5, 10);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain).progress_width(10);
        let output = display.render(&progress);

        assert_contains(&output, "[#####-----]");
    }

    #[test]
    fn test_progress_bar_full() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(10, 10);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain).progress_width(10);
        let output = display.render(&progress);

        assert_contains(&output, "[##########]");
    }

    #[test]
    fn test_progress_bar_one_of_many() {
        let progress = ShutdownProgress::new(ShutdownPhase::GracePeriod).connections(1, 100);
        let display = ShutdownProgressDisplay::new(OutputMode::Plain).progress_width(20);
        let output = display.render(&progress);

        // 1/100 at width 20 should be 0 filled (rounds down)
        assert!(output.contains("[--------------------]"));
    }
}
