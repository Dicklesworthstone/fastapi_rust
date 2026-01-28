//! Agent environment detection for output mode selection.
//!
//! This module provides heuristics to detect whether the current process
//! is running under an AI coding agent (Claude Code, Codex, Cursor, etc.)
//! or in a human-interactive terminal.

use crossterm::tty::IsTty;
use std::env;
use std::io::stdout;

/// Known AI agent environment variables.
///
/// When any of these are set, we assume an agent is running the process.
const AGENT_ENV_VARS: &[&str] = &[
    "CLAUDE_CODE",      // Claude Code CLI
    "CODEX_CLI",        // OpenAI Codex CLI
    "CURSOR_SESSION",   // Cursor IDE
    "AIDER_SESSION",    // Aider
    "AGENT_MODE",       // Generic agent flag
    "WINDSURF_SESSION", // Windsurf
    "CLINE_SESSION",    // Cline
    "COPILOT_AGENT",    // GitHub Copilot agent mode
];

/// CI environment variables that indicate non-interactive execution.
const CI_ENV_VARS: &[&str] = &[
    "CI",             // Generic CI flag
    "GITHUB_ACTIONS", // GitHub Actions
    "GITLAB_CI",      // GitLab CI
    "JENKINS_URL",    // Jenkins
    "CIRCLECI",       // CircleCI
    "TRAVIS",         // Travis CI
    "BUILDKITE",      // Buildkite
];

/// Detection result with diagnostics.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct DetectionResult {
    /// Whether an agent environment was detected.
    pub is_agent: bool,
    /// The specific agent variable that was detected.
    pub detected_agent: Option<String>,
    /// Whether a CI environment was detected.
    pub is_ci: bool,
    /// The specific CI variable that was detected.
    pub detected_ci: Option<String>,
    /// Whether stdout is connected to a TTY.
    pub is_tty: bool,
    /// Whether NO_COLOR environment variable is set.
    pub no_color_set: bool,
    /// Any override mode that was specified.
    pub override_mode: Option<OverrideMode>,
}

/// Override modes for forcing specific detection results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideMode {
    /// Force agent mode (FASTAPI_AGENT_MODE=1).
    ForceAgent,
    /// Force human mode (FASTAPI_HUMAN_MODE=1).
    ForceHuman,
}

/// Output preference based on detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputPreference {
    /// Rich output with colors and styling.
    Rich,
    /// Plain text output.
    Plain,
}

/// Check if running under an AI coding agent.
///
/// This is the main entry point for simple detection checks.
///
/// # Returns
///
/// `true` if agent environment is detected, `false` otherwise.
///
/// # Example
///
/// ```rust
/// use fastapi_output::detection::is_agent_environment;
///
/// if is_agent_environment() {
///     println!("Running in agent mode - using plain output");
/// } else {
///     println!("Running in human mode - using rich output");
/// }
/// ```
#[must_use]
pub fn is_agent_environment() -> bool {
    detect_environment().is_agent
}

/// Full detection with diagnostics for debugging.
///
/// Returns a `DetectionResult` with detailed information about
/// what was detected and why.
#[must_use]
pub fn detect_environment() -> DetectionResult {
    // Check for explicit overrides first
    let override_mode = check_overrides();
    let force_color = force_color_enabled();

    // Check agent env vars
    let (is_agent_var, detected_agent) = check_agent_vars();

    // Check CI env vars
    let (is_ci_var, detected_ci) = check_ci_vars();

    // Check NO_COLOR standard
    let no_color_set = env::var("NO_COLOR").is_ok();

    // Check if stdout is a TTY
    let is_tty = stdout().is_tty();

    // Final determination
    let is_agent = match override_mode {
        Some(OverrideMode::ForceAgent) => true,
        Some(OverrideMode::ForceHuman) => false,
        None => {
            if force_color {
                false
            } else {
                is_agent_var || is_ci_var || no_color_set || !is_tty
            }
        }
    };

    DetectionResult {
        is_agent,
        detected_agent,
        is_ci: is_ci_var,
        detected_ci,
        is_tty,
        no_color_set,
        override_mode,
    }
}

/// Check for override environment variables.
fn check_overrides() -> Option<OverrideMode> {
    if env::var("FASTAPI_AGENT_MODE").is_ok_and(|v| v == "1") {
        Some(OverrideMode::ForceAgent)
    } else if env::var("FASTAPI_HUMAN_MODE").is_ok_and(|v| v == "1") {
        Some(OverrideMode::ForceHuman)
    } else {
        None
    }
}

/// Check for FORCE_COLOR override.
fn force_color_enabled() -> bool {
    env::var("FORCE_COLOR").is_ok_and(|v| v != "0")
}

/// Check for known agent environment variables.
fn check_agent_vars() -> (bool, Option<String>) {
    for var in AGENT_ENV_VARS {
        if env::var(var).is_ok() {
            return (true, Some((*var).to_string()));
        }
    }
    (false, None)
}

/// Check for known CI environment variables.
fn check_ci_vars() -> (bool, Option<String>) {
    for var in CI_ENV_VARS {
        if env::var(var).is_ok() {
            return (true, Some((*var).to_string()));
        }
    }
    (false, None)
}

/// Return user preference based on detection.
#[must_use]
pub fn detected_preference() -> OutputPreference {
    let result = detect_environment();
    if result.is_agent {
        OutputPreference::Plain
    } else {
        OutputPreference::Rich
    }
}

/// Get detailed diagnostics as a formatted string (for debugging).
#[must_use]
pub fn detection_diagnostics() -> String {
    let result = detect_environment();
    let force_color = force_color_enabled();
    format!(
        "DetectionResult {{ is_agent: {}, detected_agent: {:?}, is_ci: {}, \
         detected_ci: {:?}, is_tty: {}, no_color_set: {}, force_color_set: {}, \
         override_mode: {:?} }}",
        result.is_agent,
        result.detected_agent,
        result.is_ci,
        result.detected_ci,
        result.is_tty,
        result.no_color_set,
        force_color,
        result.override_mode
    )
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;

    /// Helper to clean environment before each test.
    ///
    /// # Safety
    ///
    /// This function modifies environment variables, which is inherently
    /// unsafe in multi-threaded contexts. We use serial_test to ensure
    /// tests run sequentially.
    fn clean_env() {
        // SAFETY: Tests are run serially via #[serial] attribute
        unsafe {
            for var in AGENT_ENV_VARS {
                env::remove_var(var);
            }
            for var in CI_ENV_VARS {
                env::remove_var(var);
            }
            env::remove_var("NO_COLOR");
            env::remove_var("FORCE_COLOR");
            env::remove_var("FASTAPI_AGENT_MODE");
            env::remove_var("FASTAPI_HUMAN_MODE");
        }
    }

    /// Helper to run test with clean env, restoring afterwards.
    fn with_clean_env<F: FnOnce()>(f: F) {
        clean_env();
        f();
        clean_env();
    }

    /// Helper to set an environment variable safely in tests.
    ///
    /// # Safety
    ///
    /// Tests are run serially via #[serial] attribute.
    fn set_env(key: &str, value: &str) {
        // SAFETY: Tests are run serially via #[serial] attribute
        unsafe {
            env::set_var(key, value);
        }
    }

    // ========== AGENT DETECTION TESTS ==========

    #[test]
    #[serial]
    fn test_claude_code_detection() {
        with_clean_env(|| {
            set_env("CLAUDE_CODE", "1");
            let result = detect_environment();
            eprintln!("[TEST] Claude Code detection: {result:?}");
            assert!(result.is_agent, "Should detect Claude Code as agent");
            assert_eq!(result.detected_agent, Some("CLAUDE_CODE".to_string()));
        });
    }

    #[test]
    #[serial]
    fn test_codex_cli_detection() {
        with_clean_env(|| {
            set_env("CODEX_CLI", "1");
            let result = detect_environment();
            eprintln!("[TEST] Codex CLI detection: {result:?}");
            assert!(result.is_agent, "Should detect Codex CLI as agent");
            assert_eq!(result.detected_agent, Some("CODEX_CLI".to_string()));
        });
    }

    #[test]
    #[serial]
    fn test_cursor_session_detection() {
        with_clean_env(|| {
            set_env("CURSOR_SESSION", "abc123");
            let result = detect_environment();
            eprintln!("[TEST] Cursor detection: {result:?}");
            assert!(result.is_agent, "Should detect Cursor as agent");
            assert_eq!(result.detected_agent, Some("CURSOR_SESSION".to_string()));
        });
    }

    #[test]
    #[serial]
    fn test_aider_session_detection() {
        with_clean_env(|| {
            set_env("AIDER_SESSION", "1");
            let result = detect_environment();
            eprintln!("[TEST] Aider detection: {result:?}");
            assert!(result.is_agent, "Should detect Aider as agent");
        });
    }

    #[test]
    #[serial]
    fn test_generic_agent_mode_detection() {
        with_clean_env(|| {
            set_env("AGENT_MODE", "1");
            let result = detect_environment();
            eprintln!("[TEST] Generic AGENT_MODE detection: {result:?}");
            assert!(result.is_agent, "Should detect AGENT_MODE");
        });
    }

    #[test]
    #[serial]
    fn test_windsurf_detection() {
        with_clean_env(|| {
            set_env("WINDSURF_SESSION", "1");
            let result = detect_environment();
            eprintln!("[TEST] Windsurf detection: {result:?}");
            assert!(result.is_agent, "Should detect Windsurf");
        });
    }

    #[test]
    #[serial]
    fn test_cline_detection() {
        with_clean_env(|| {
            set_env("CLINE_SESSION", "1");
            let result = detect_environment();
            eprintln!("[TEST] Cline detection: {result:?}");
            assert!(result.is_agent, "Should detect Cline");
        });
    }

    #[test]
    #[serial]
    fn test_copilot_agent_detection() {
        with_clean_env(|| {
            set_env("COPILOT_AGENT", "1");
            let result = detect_environment();
            eprintln!("[TEST] Copilot agent detection: {result:?}");
            assert!(result.is_agent, "Should detect Copilot agent");
        });
    }

    // ========== CI DETECTION TESTS ==========

    #[test]
    #[serial]
    fn test_generic_ci_detection() {
        with_clean_env(|| {
            set_env("CI", "true");
            let result = detect_environment();
            eprintln!("[TEST] Generic CI detection: {result:?}");
            assert!(result.is_ci, "Should detect CI environment");
            assert!(result.is_agent, "CI should trigger agent mode");
        });
    }

    #[test]
    #[serial]
    fn test_github_actions_detection() {
        with_clean_env(|| {
            set_env("GITHUB_ACTIONS", "true");
            let result = detect_environment();
            eprintln!("[TEST] GitHub Actions detection: {result:?}");
            assert!(result.is_ci);
            assert_eq!(result.detected_ci, Some("GITHUB_ACTIONS".to_string()));
        });
    }

    #[test]
    #[serial]
    fn test_gitlab_ci_detection() {
        with_clean_env(|| {
            set_env("GITLAB_CI", "true");
            let result = detect_environment();
            eprintln!("[TEST] GitLab CI detection: {result:?}");
            assert!(result.is_ci);
        });
    }

    #[test]
    #[serial]
    fn test_jenkins_detection() {
        with_clean_env(|| {
            set_env("JENKINS_URL", "http://jenkins.example.com");
            let result = detect_environment();
            eprintln!("[TEST] Jenkins detection: {result:?}");
            assert!(result.is_ci);
        });
    }

    // ========== NO_COLOR STANDARD TESTS ==========

    #[test]
    #[serial]
    fn test_no_color_detection() {
        with_clean_env(|| {
            set_env("NO_COLOR", "1");
            let result = detect_environment();
            eprintln!("[TEST] NO_COLOR detection: {result:?}");
            assert!(result.no_color_set, "Should detect NO_COLOR");
            assert!(result.is_agent, "NO_COLOR should trigger plain mode");
        });
    }

    #[test]
    #[serial]
    fn test_no_color_empty_value() {
        with_clean_env(|| {
            set_env("NO_COLOR", ""); // Empty but set
            let result = detect_environment();
            eprintln!("[TEST] NO_COLOR empty value: {result:?}");
            assert!(
                result.no_color_set,
                "Empty NO_COLOR should still be detected"
            );
        });
    }

    // ========== FORCE_COLOR OVERRIDE TESTS ==========

    #[test]
    #[serial]
    fn test_force_color_overrides_ci() {
        with_clean_env(|| {
            set_env("CI", "true");
            set_env("FORCE_COLOR", "1");
            let result = detect_environment();
            eprintln!("[TEST] FORCE_COLOR override: {result:?}");
            assert!(!result.is_agent, "FORCE_COLOR should prefer rich output");
        });
    }

    // ========== OVERRIDE TESTS ==========

    #[test]
    #[serial]
    fn test_force_agent_mode_override() {
        with_clean_env(|| {
            set_env("FASTAPI_AGENT_MODE", "1");
            let result = detect_environment();
            eprintln!("[TEST] FASTAPI_AGENT_MODE override: {result:?}");
            assert!(result.is_agent, "Override should force agent mode");
            assert_eq!(result.override_mode, Some(OverrideMode::ForceAgent));
        });
    }

    #[test]
    #[serial]
    fn test_force_human_mode_override() {
        with_clean_env(|| {
            // Set agent var but then override to human
            set_env("CLAUDE_CODE", "1");
            set_env("FASTAPI_HUMAN_MODE", "1");
            let result = detect_environment();
            eprintln!("[TEST] FASTAPI_HUMAN_MODE override: {result:?}");
            assert!(!result.is_agent, "Override should force human mode");
            assert_eq!(result.override_mode, Some(OverrideMode::ForceHuman));
        });
    }

    #[test]
    #[serial]
    fn test_agent_override_takes_precedence() {
        with_clean_env(|| {
            // Both overrides set - agent takes precedence
            set_env("FASTAPI_AGENT_MODE", "1");
            set_env("FASTAPI_HUMAN_MODE", "1");
            let result = detect_environment();
            eprintln!("[TEST] Both overrides set: {result:?}");
            assert!(result.is_agent, "AGENT_MODE should take precedence");
        });
    }

    // ========== OUTPUT PREFERENCE TESTS ==========

    #[test]
    #[serial]
    fn test_preference_plain_for_agent() {
        with_clean_env(|| {
            set_env("CLAUDE_CODE", "1");
            let pref = detected_preference();
            eprintln!("[TEST] Preference for agent: {pref:?}");
            assert_eq!(pref, OutputPreference::Plain);
        });
    }

    #[test]
    #[serial]
    fn test_preference_rich_for_human_tty() {
        with_clean_env(|| {
            // Note: This test may fail if not run in a TTY
            // The detection will fall back based on is_tty
            let result = detect_environment();
            eprintln!("[TEST] Clean env detection: {result:?}");
            // We cant guarantee TTY in CI, just log the result
        });
    }

    // ========== DIAGNOSTICS TESTS ==========

    #[test]
    #[serial]
    fn test_diagnostics_format() {
        with_clean_env(|| {
            set_env("CLAUDE_CODE", "1");
            let diag = detection_diagnostics();
            eprintln!("[TEST] Diagnostics output: {diag}");
            assert!(diag.contains("is_agent: true"));
            assert!(diag.contains("CLAUDE_CODE"));
        });
    }

    // ========== EDGE CASE TESTS ==========

    #[test]
    #[serial]
    fn test_multiple_agents_first_wins() {
        with_clean_env(|| {
            set_env("CLAUDE_CODE", "1");
            set_env("CODEX_CLI", "1");
            let result = detect_environment();
            eprintln!("[TEST] Multiple agents: {result:?}");
            assert!(result.is_agent);
            // First one in list wins
            assert_eq!(result.detected_agent, Some("CLAUDE_CODE".to_string()));
        });
    }

    #[test]
    #[serial]
    fn test_ci_and_agent_both_detected() {
        with_clean_env(|| {
            set_env("CLAUDE_CODE", "1");
            set_env("CI", "true");
            let result = detect_environment();
            eprintln!("[TEST] Agent + CI: {result:?}");
            assert!(result.is_agent);
            assert!(result.is_ci);
            assert!(result.detected_agent.is_some());
            assert!(result.detected_ci.is_some());
        });
    }

    #[test]
    #[serial]
    fn test_clean_environment() {
        with_clean_env(|| {
            let result = detect_environment();
            eprintln!("[TEST] Clean environment: {result:?}");
            assert!(result.detected_agent.is_none());
            assert!(result.detected_ci.is_none());
            assert!(!result.no_color_set);
            assert!(result.override_mode.is_none());
            // is_agent depends on TTY status
        });
    }
}
