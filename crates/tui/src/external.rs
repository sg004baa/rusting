use std::{
    fs,
    io::{self, Write as _},
    path::Path,
    process::{Command, ExitStatus},
};

use anyhow::{Context as _, bail};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        DisableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use tempfile::Builder;

pub fn edit_in_external(
    command: &str,
    contents: &str,
    extension: Option<&str>,
) -> anyhow::Result<String> {
    let suffix = extension
        .filter(|value| !value.is_empty())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    let mut file = Builder::new()
        .prefix("rusting-edit-")
        .suffix(&suffix)
        .tempfile()
        .context("could not create a temporary editor file")?;
    file.write_all(contents.as_bytes())
        .context("could not write the temporary editor file")?;
    file.flush()
        .context("could not flush the temporary editor file")?;
    run_with_terminal_handoff(command, file.path())?;
    fs::read_to_string(file.path()).context("could not read the edited temporary file")
}

pub fn view_in_pager(command: &str, contents: &str, extension: Option<&str>) -> anyhow::Result<()> {
    let suffix = extension
        .filter(|value| !value.is_empty())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    let mut file = Builder::new()
        .prefix("rusting-view-")
        .suffix(&suffix)
        .tempfile()
        .context("could not create a temporary pager file")?;
    file.write_all(contents.as_bytes())
        .context("could not write the temporary pager file")?;
    file.flush()
        .context("could not flush the temporary pager file")?;
    run_with_terminal_handoff(command, file.path())
}

/// Opens an existing file directly. This is used for collection-relative script
/// paths: editing must never copy a script through a temporary file.
pub fn edit_path_in_external(command: &str, path: &Path) -> anyhow::Result<()> {
    if !path.is_file() {
        bail!("cannot edit missing file {}", path.display());
    }
    run_with_terminal_handoff(command, path)
}

/// Opens an existing file directly in the configured pager.
pub fn view_path_in_pager(command: &str, path: &Path) -> anyhow::Result<()> {
    if !path.is_file() {
        bail!("cannot page missing file {}", path.display());
    }
    run_with_terminal_handoff(command, path)
}

fn run_with_terminal_handoff(command: &str, path: &Path) -> anyhow::Result<()> {
    let (program, arguments) = split_command(command)?;
    let suspend_result = suspend_terminal();
    if let Err(suspend_error) = suspend_result {
        let restore_result = restore_terminal();
        return match restore_result {
            Ok(()) => Err(suspend_error),
            Err(restore_error) => Err(anyhow::anyhow!(
                "could not suspend terminal: {suspend_error:#}; terminal restoration also failed: {restore_error:#}"
            )),
        };
    }

    let child_result = Command::new(&program)
        .args(&arguments)
        .arg(path)
        .status()
        .with_context(|| format!("could not start external command {program:?}"));
    let restore_result = restore_terminal();

    match (child_result, restore_result) {
        (Ok(status), Ok(())) => ensure_success(status, command),
        (Err(child_error), Ok(())) => Err(child_error),
        (Ok(status), Err(restore_error)) => {
            if status.success() {
                Err(restore_error
                    .context("external command finished but terminal restoration failed"))
            } else {
                Err(anyhow::anyhow!(
                    "external command {command:?} exited with {status}; terminal restoration also failed: {restore_error:#}"
                ))
            }
        }
        (Err(child_error), Err(restore_error)) => Err(anyhow::anyhow!(
            "external command failed: {child_error:#}; terminal restoration also failed: {restore_error:#}"
        )),
    }
}

fn split_command(command: &str) -> anyhow::Result<(String, Vec<String>)> {
    let mut words = shell_words::split(command).context("external command has invalid quoting")?;
    if words.is_empty() {
        bail!("external command is empty");
    }
    let program = words.remove(0);
    Ok((program, words))
}

fn ensure_success(status: ExitStatus, command: &str) -> anyhow::Result<()> {
    if status.success() {
        Ok(())
    } else {
        bail!("external command {command:?} exited with {status}")
    }
}

fn suspend_terminal() -> anyhow::Result<()> {
    let mut stdout = io::stdout();
    let mut first_error = None;
    if let Err(error) = execute!(stdout, PopKeyboardEnhancementFlags) {
        first_error = Some(anyhow::Error::from(error));
    }
    if let Err(error) = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen, Show)
        && first_error.is_none()
    {
        first_error = Some(anyhow::Error::from(error));
    }
    if let Err(error) = disable_raw_mode()
        && first_error.is_none()
    {
        first_error = Some(anyhow::Error::from(error));
    }
    if let Err(error) = stdout.flush()
        && first_error.is_none()
    {
        first_error = Some(anyhow::Error::from(error));
    }
    match first_error {
        Some(error) => Err(error.context("could not suspend terminal for external command")),
        None => Ok(()),
    }
}

fn restore_terminal() -> anyhow::Result<()> {
    let mut stdout = io::stdout();
    let mut first_error = enable_raw_mode().err().map(anyhow::Error::from);
    if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide)
        && first_error.is_none()
    {
        first_error = Some(anyhow::Error::from(error));
    }
    let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
    // Unsupported terminals may reject the protocol. The terminal is otherwise
    // fully restored, so this capability negotiation is intentionally optional.
    let _ = execute!(stdout, PushKeyboardEnhancementFlags(flags));
    if let Err(error) = stdout.flush()
        && first_error.is_none()
    {
        first_error = Some(anyhow::Error::from(error));
    }
    match first_error {
        Some(error) => Err(error.context("could not restore terminal after external command")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parser_preserves_quoted_arguments() {
        let (program, arguments) = split_command("editor --flag 'two words'").unwrap();
        assert_eq!(program, "editor");
        assert_eq!(arguments, ["--flag", "two words"]);
        assert!(split_command("  ").is_err());
        assert!(split_command("'unterminated").is_err());
    }
}
