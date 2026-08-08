//! JavaScript request hooks, run on an embedded QuickJS engine.

pub mod api;
pub mod convert;
pub mod engine;
pub mod types;

pub use types::{Effect, HookOutcome, HookStatus, LogLine, ScriptError, Severity, Stream};
