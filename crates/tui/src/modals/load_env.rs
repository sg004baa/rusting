use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};

use super::{Modal, ModalResult, centered, control, frame};
use crate::theme;
use crate::widgets::fuzzy;
use crate::widgets::popup::{Popup, PopupAction, PopupItem};
use crate::widgets::{Input, InputAction};

const PLACEHOLDER: &str = ".env, path/to/file.env, ~/rusting.env";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    value: String,
    is_directory: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Input,
    Load,
    Cancel,
}

/// Chooses an environment file. Existence and file-type validation remain the
/// application's responsibility when it consumes [`Self::path`].
pub struct LoadEnvModal {
    working_directory: PathBuf,
    config_directory: PathBuf,
    home_directory: Option<PathBuf>,
    input: Input,
    popup: Popup,
    candidates: Vec<Candidate>,
    focus: Focus,
    input_area: Rect,
}

impl LoadEnvModal {
    pub fn new(working_directory: PathBuf, config_directory: PathBuf) -> Self {
        Self {
            working_directory,
            config_directory,
            home_directory: std::env::home_dir(),
            input: Input::with_placeholder(PLACEHOLDER),
            popup: Popup::new(),
            candidates: Vec::new(),
            focus: Focus::Input,
            input_area: Rect::default(),
        }
    }

    pub fn path(&self) -> Option<PathBuf> {
        let raw = self.input.value().trim();
        if raw.is_empty() {
            return None;
        }
        Some(self.resolve(raw))
    }

    fn resolve(&self, raw: &str) -> PathBuf {
        let expanded = if raw == "~" {
            self.home_directory
                .clone()
                .unwrap_or_else(|| PathBuf::from(raw))
        } else if let Some(rest) = raw.strip_prefix("~/") {
            match &self.home_directory {
                Some(home) => home.join(rest),
                None => PathBuf::from(raw),
            }
        } else {
            PathBuf::from(raw)
        };
        if expanded.is_absolute() {
            normalize(expanded)
        } else {
            normalize(self.working_directory.join(expanded))
        }
    }

    fn completion_candidates(&self) -> Vec<Candidate> {
        let current = &self.input.value()[..self.input.cursor()];
        if current.is_empty() {
            return self.empty_candidates();
        }

        let (parent_fragment, needle) = match current.rsplit_once('/') {
            Some((parent, needle)) => (format!("{parent}/"), needle),
            None => (String::new(), current),
        };
        let directory = if parent_fragment.is_empty() {
            self.working_directory.clone()
        } else {
            self.resolve(&parent_fragment)
        };
        let candidates = directory_candidates(&directory);
        rank_candidates(needle, candidates)
    }

    fn empty_candidates(&self) -> Vec<Candidate> {
        let cwd = directory_candidates(&self.working_directory);
        let mut cwd_files = Vec::new();
        let mut cwd_directories = Vec::new();
        let mut seen = HashSet::new();
        for candidate in cwd {
            if candidate.is_directory {
                cwd_directories.push(candidate);
            } else {
                seen.insert(normalize(self.working_directory.join(&candidate.value)));
                cwd_files.push(candidate);
            }
        }

        let mut config_files = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.config_directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_file = entry
                    .file_type()
                    .map(|kind| kind.is_file())
                    .unwrap_or(false);
                if !is_file || !is_env_file(&path) {
                    continue;
                }
                let absolute = if path.is_absolute() {
                    normalize(path)
                } else {
                    normalize(self.working_directory.join(path))
                };
                if !seen.insert(absolute.clone()) {
                    continue;
                }
                config_files.push(Candidate {
                    value: absolute.to_string_lossy().into_owned(),
                    is_directory: false,
                });
            }
        }
        config_files.sort_by(candidate_order);
        cwd_files.extend(config_files);
        cwd_files.extend(cwd_directories);
        cwd_files
    }

    fn refresh_popup(&mut self) {
        self.candidates = self.completion_candidates();
        let items = self
            .candidates
            .iter()
            .map(|candidate| PopupItem {
                text: candidate.value.clone(),
                match_positions: Vec::new(),
                style: if candidate.is_directory {
                    Style::new().fg(theme::ACCENT)
                } else {
                    Style::new()
                },
            })
            .collect();
        self.popup.open(items);
    }

    fn apply_candidate(&mut self, index: usize) {
        let Some(candidate) = self.candidates.get(index) else {
            return;
        };
        let value = candidate.value.clone();
        let is_directory = candidate.is_directory;
        let cursor = self.input.cursor();
        let start = self.input.value()[..cursor]
            .rfind('/')
            .map_or(0, |slash| slash + 1);
        self.input.splice(start..cursor, &value);
        self.popup.close();
        if is_directory {
            self.refresh_popup();
        }
    }

    fn handle_popup_key(&mut self, key: KeyEvent) -> bool {
        match self.popup.handle_key(key) {
            PopupAction::Accepted(index) => {
                self.apply_candidate(index);
                true
            }
            PopupAction::Consumed => true,
            PopupAction::Dismissed => true,
            PopupAction::Ignored => false,
        }
    }

    fn activate(&self) -> ModalResult {
        if self.path().is_some() {
            ModalResult::Accepted
        } else {
            ModalResult::Cancelled
        }
    }

    fn move_focus(&mut self, backwards: bool) {
        self.popup.close();
        self.focus = match (self.focus, backwards) {
            (Focus::Input, false) | (Focus::Cancel, true) => Focus::Load,
            (Focus::Load, false) | (Focus::Input, true) => Focus::Cancel,
            (Focus::Cancel, false) | (Focus::Load, true) => Focus::Input,
        };
    }
}

impl Modal for LoadEnvModal {
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        if key.code == KeyCode::Esc {
            return ModalResult::Cancelled;
        }
        let backwards = key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT));
        if backwards {
            if self.focus == Focus::Input && self.popup.is_open() && self.handle_popup_key(key) {
                return ModalResult::Open;
            }
            self.move_focus(true);
            return ModalResult::Open;
        }
        if key.code == KeyCode::Tab {
            if self.focus == Focus::Input && self.popup.is_open() && self.handle_popup_key(key) {
                return ModalResult::Open;
            }
            self.move_focus(false);
            return ModalResult::Open;
        }

        if self.focus != Focus::Input {
            let plain = !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
            return match key.code {
                KeyCode::Enter | KeyCode::Char(' ') if plain => match self.focus {
                    Focus::Load => self.activate(),
                    Focus::Cancel => ModalResult::Cancelled,
                    Focus::Input => ModalResult::Open,
                },
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::Char('h' | 'H' | 'j' | 'J' | 'k' | 'K' | 'l' | 'L')
                    if plain =>
                {
                    self.focus = match self.focus {
                        Focus::Load => Focus::Cancel,
                        Focus::Cancel => Focus::Load,
                        Focus::Input => Focus::Load,
                    };
                    ModalResult::Open
                }
                _ => ModalResult::Open,
            };
        }

        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
            if !self.popup.is_open() {
                self.refresh_popup();
                return ModalResult::Open;
            }
            if self.handle_popup_key(key) {
                return ModalResult::Open;
            }
        }
        if key.code == KeyCode::Enter {
            if self.popup.is_open() && self.handle_popup_key(key) {
                return ModalResult::Open;
            }
            return self.activate();
        }

        match self.input.handle_key(key) {
            InputAction::Changed => {
                self.refresh_popup();
                ModalResult::Open
            }
            InputAction::Submitted => self.activate(),
            InputAction::LeaveUp | InputAction::LeaveDown => ModalResult::Open,
            InputAction::Ignored | InputAction::Consumed => ModalResult::Open,
        }
    }

    fn render(&mut self, screen: Rect, buffer: &mut Buffer) {
        let area = centered(screen, screen.width.clamp(30, 68), 11);
        let inner = frame("Load Environment File", area, buffer);
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(inner);
        Paragraph::new("Enter a path to an environment file:").render(rows[0], buffer);
        self.input_area = control(rows[1], buffer, self.focus == Focus::Input);
        self.input
            .render(self.input_area, buffer, self.focus == Focus::Input, &[]);
        Paragraph::new("Press [down] or type for suggestions")
            .style(Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM))
            .render(rows[2], buffer);

        let buttons = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[4]);
        render_button(
            "Load [Enter]",
            self.focus == Focus::Load,
            buttons[0],
            buffer,
        );
        render_button(
            "Cancel [ESC]",
            self.focus == Focus::Cancel,
            buttons[1],
            buffer,
        );
        if self.popup.is_open() {
            self.popup.render(self.input_area, screen, buffer);
        }
    }
}

fn is_env_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == ".env" || name.ends_with(".env") || name.starts_with(".env.")
}

fn directory_candidates(directory: &Path) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return candidates;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            candidates.push(Candidate {
                value: format!("{name}/"),
                is_directory: true,
            });
        } else if kind.is_file() && is_env_file(&entry.path()) {
            candidates.push(Candidate {
                value: name,
                is_directory: false,
            });
        }
    }
    candidates.sort_by(candidate_order);
    candidates
}

fn candidate_order(left: &Candidate, right: &Candidate) -> std::cmp::Ordering {
    let left_name = candidate_name(&left.value);
    let right_name = candidate_name(&right.value);
    (
        left.is_directory,
        !left_name.starts_with('.'),
        left_name.to_lowercase(),
    )
        .cmp(&(
            right.is_directory,
            !right_name.starts_with('.'),
            right_name.to_lowercase(),
        ))
}

fn candidate_name(value: &str) -> &str {
    let value = value.trim_end_matches('/');
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(value)
}

fn rank_candidates(needle: &str, candidates: Vec<Candidate>) -> Vec<Candidate> {
    if needle.is_empty() {
        return candidates;
    }
    let names: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.value.trim_end_matches('/'))
        .collect();
    fuzzy::rank(needle, &names)
        .into_iter()
        .map(|matched| candidates[matched.index].clone())
        .collect()
}

fn normalize(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn render_button(label: &str, focused: bool, area: Rect, buffer: &mut Buffer) {
    let style = if focused {
        theme::selection()
    } else {
        Style::new().fg(theme::MUTED)
    };
    let inner = control(area, buffer, focused);
    Paragraph::new(Line::from(Span::styled(format!(" {label} "), style)))
        .style(style)
        .centered()
        .render(inner, buffer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir, write};

    #[test]
    fn environment_file_name_rules_match_the_dialog_contract() {
        assert!(is_env_file(Path::new(".env")));
        assert!(is_env_file(Path::new("local.env")));
        assert!(is_env_file(Path::new(".env.production")));
        assert!(!is_env_file(Path::new("environment")));
        assert!(!is_env_file(Path::new("env.txt")));
    }

    #[test]
    fn empty_input_orders_cwd_files_config_files_then_directories() {
        let root = tempfile::tempdir().expect("temp directory");
        let config = tempfile::tempdir().expect("config directory");
        write(root.path().join("z.env"), "Z=1").expect("cwd env");
        write(root.path().join(".env"), "A=1").expect("dot env");
        create_dir(root.path().join("folder")).expect("cwd folder");
        write(config.path().join("config.env"), "C=1").expect("config env");

        let modal = LoadEnvModal::new(root.path().into(), config.path().into());
        let candidates = modal.empty_candidates();
        assert_eq!(candidates[0].value, ".env");
        assert_eq!(candidates[1].value, "z.env");
        assert_eq!(
            PathBuf::from(&candidates[2].value),
            config.path().join("config.env")
        );
        assert_eq!(candidates[3].value, "folder/");
    }

    #[test]
    fn relative_and_home_paths_are_resolved_without_requiring_existence() {
        let root = tempfile::tempdir().expect("temp directory");
        let mut modal = LoadEnvModal::new(root.path().into(), root.path().into());
        modal.input.set_value("nested/file.env");
        assert_eq!(modal.path(), Some(root.path().join("nested/file.env")));

        if let Some(home) = std::env::home_dir() {
            modal.input.set_value("~/rusting.env");
            assert_eq!(modal.path(), Some(home.join("rusting.env")));
        }
    }

    #[test]
    fn empty_submission_cancels_and_nonempty_submission_accepts() {
        let root = tempfile::tempdir().expect("temp directory");
        let mut modal = LoadEnvModal::new(root.path().into(), root.path().into());
        assert_eq!(
            modal.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ModalResult::Cancelled
        );
        modal.input.set_value("missing.env");
        assert_eq!(
            modal.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ModalResult::Accepted
        );
    }
}
