//! Types crossing the scripting boundary. The TUI never sees `rquickjs`.

use std::path::PathBuf;

/// One line a script wrote, shown in the response Scripts tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub stream: Stream,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// `console.log` / `console.info`.
    Out,
    /// `console.error` / `console.warn`.
    Err,
}

/// Whether a hook ran, and how it went.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HookStatus {
    /// No script is configured for this hook.
    #[default]
    NotConfigured,
    Success,
    /// The message is shown in the status cell's tooltip and as a notification.
    Error(String),
}

/// A side effect a script asked for that only the app can perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    SetVariable { name: String, value: String },
    ClearVariable { name: String },
    ClearAllVariables,
    Notify { message: String, severity: Severity },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    #[default]
    Information,
    Warning,
    Error,
}

/// What one hook invocation produced.
#[derive(Debug, Clone, Default)]
pub struct HookOutcome {
    pub status: HookStatus,
    pub logs: Vec<LogLine>,
    /// Applied by the caller in order, after the hook returns.
    pub effects: Vec<Effect>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("Script not found: {0}")]
    NotFound(PathBuf),
    #[error("Script path escapes the collection: {0}")]
    OutsideCollection(PathBuf),
    #[error("{path}: no exported function named '{function}'")]
    MissingFunction { path: PathBuf, function: String },
    #[error("{path} could not be loaded: {message}")]
    Load { path: PathBuf, message: String },
    #[error("{function} threw: {message}")]
    Threw { function: String, message: String },
    #[error("Script exceeded its {0}ms budget.")]
    TimedOut(u64),
}

/// How long a single hook may run before it is interrupted.
///
/// Scripts run on the send path, so a runaway loop would hang the request. The
/// engine's interrupt handler enforces this.
pub const HOOK_BUDGET_MS: u64 = 5_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hook_status_is_not_configured() {
        assert_eq!(HookStatus::default(), HookStatus::NotConfigured);
        assert!(HookOutcome::default().logs.is_empty());
        assert!(HookOutcome::default().effects.is_empty());
    }

    #[test]
    fn errors_render_the_offending_path() {
        let error = ScriptError::MissingFunction {
            path: PathBuf::from("scripts/hooks.js"),
            function: "on_request".into(),
        };
        let rendered = error.to_string();
        assert!(rendered.contains("scripts/hooks.js"), "{rendered}");
        assert!(rendered.contains("on_request"), "{rendered}");
    }
}
