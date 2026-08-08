//! Collection-relative JavaScript hook paths.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Widget as _};
use rusting_core::{RequestModel, ScriptHook, ScriptRef, Scripts};

use crate::theme;
use crate::widgets::fuzzy;
use crate::widgets::input::{Input, InputAction};
use crate::widgets::popup::{Popup, PopupAction, PopupItem};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptsAction {
    Ignored,
    Consumed,
    Changed,
    OpenInEditor(PathBuf),
    OpenInPager(PathBuf),
    LeaveUp,
    LeaveDown,
}

pub struct ScriptsTab {
    collection_root: PathBuf,
    inputs: [Input; 3],
    candidates: Vec<String>,
    focus: usize,
    popup: Popup,
    popup_anchor: Rect,
    completion_function: Option<String>,
}

impl ScriptsTab {
    pub fn new(collection_root: PathBuf) -> Self {
        let mut tab = Self {
            collection_root,
            inputs: [
                Input::with_placeholder("Collection-relative path to setup script"),
                Input::with_placeholder("Collection-relative path to pre-request script"),
                Input::with_placeholder("Collection-relative path to post-response script"),
            ],
            candidates: Vec::new(),
            focus: 0,
            popup: Popup::new(),
            popup_anchor: Rect::ZERO,
            completion_function: None,
        };
        tab.refresh_candidates();
        tab
    }

    pub fn refresh_candidates(&mut self) {
        self.candidates.clear();
        collect_javascript(
            &self.collection_root,
            &self.collection_root,
            &mut self.candidates,
        );
        self.candidates.sort_by_key(|path| path.to_lowercase());
        self.candidates.dedup();
    }

    pub fn load(&mut self, request: &RequestModel) {
        self.inputs[0].set_value(request.scripts.setup.clone().unwrap_or_default());
        self.inputs[1].set_value(request.scripts.on_request.clone().unwrap_or_default());
        self.inputs[2].set_value(request.scripts.on_response.clone().unwrap_or_default());
        self.focus = 0;
        self.close_popup();
    }

    pub fn to_model(&self) -> Scripts {
        Scripts {
            setup: non_empty(self.inputs[0].value()),
            on_request: non_empty(self.inputs[1].value()),
            on_response: non_empty(self.inputs[2].value()),
        }
    }

    pub fn focus_first_control(&mut self) {
        self.focus = 0;
        self.close_popup();
    }

    pub fn focus_last_control(&mut self) {
        self.focus = self.inputs.len() - 1;
        self.close_popup();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ScriptsAction {
        if key.code == KeyCode::Tab {
            return self.move_down();
        }
        if key.code == KeyCode::BackTab {
            return self.move_up();
        }
        if self.popup.is_open() {
            match self.popup.handle_key(key) {
                PopupAction::Accepted(index) => {
                    self.accept_completion(index);
                    return ScriptsAction::Changed;
                }
                PopupAction::Dismissed => {
                    self.close_popup();
                    return ScriptsAction::Consumed;
                }
                PopupAction::Consumed => return ScriptsAction::Consumed,
                PopupAction::Ignored => {}
            }
        }

        if key.code == KeyCode::Char('e') && key.modifiers == KeyModifiers::CONTROL {
            return self
                .resolved_path()
                .map(ScriptsAction::OpenInEditor)
                .unwrap_or(ScriptsAction::Consumed);
        }
        if key.code == KeyCode::Char('p') && key.modifiers == KeyModifiers::ALT {
            return self
                .resolved_path()
                .map(ScriptsAction::OpenInPager)
                .unwrap_or(ScriptsAction::Consumed);
        }
        if key.code == KeyCode::Down && key.modifiers.is_empty() {
            self.refresh_completion();
            if self.popup.is_open() {
                return ScriptsAction::Consumed;
            }
        }

        match self.inputs[self.focus].handle_key(key) {
            InputAction::Changed => {
                self.refresh_completion();
                ScriptsAction::Changed
            }
            InputAction::Submitted | InputAction::Consumed => ScriptsAction::Consumed,
            InputAction::LeaveUp => self.move_up(),
            InputAction::LeaveDown => self.move_down(),
            InputAction::Ignored => ScriptsAction::Ignored,
        }
    }

    fn move_up(&mut self) -> ScriptsAction {
        self.close_popup();
        if self.focus == 0 {
            ScriptsAction::LeaveUp
        } else {
            self.focus -= 1;
            ScriptsAction::Consumed
        }
    }

    fn move_down(&mut self) -> ScriptsAction {
        self.close_popup();
        if self.focus + 1 == self.inputs.len() {
            ScriptsAction::LeaveDown
        } else {
            self.focus += 1;
            ScriptsAction::Consumed
        }
    }

    fn hook(&self) -> ScriptHook {
        ScriptHook::ALL[self.focus]
    }

    fn resolved_path(&self) -> Option<PathBuf> {
        let reference = ScriptRef::parse(self.inputs[self.focus].value(), self.hook())?;
        // ScriptRef::parse removes the optional `:function` suffix. The action
        // always addresses the JavaScript file itself.
        Some(if reference.path.is_absolute() {
            reference.path
        } else {
            self.collection_root.join(reference.path)
        })
    }

    fn refresh_completion(&mut self) {
        let raw = self.inputs[self.focus].value();
        let parsed = ScriptRef::parse(raw, self.hook());
        let (needle, explicit_function) = match parsed {
            Some(reference) => {
                let explicit = raw
                    .rsplit_once(':')
                    .filter(|(path, function)| !path.is_empty() && !function.is_empty())
                    .map(|(_, function)| function.to_owned());
                (reference.path.to_string_lossy().into_owned(), explicit)
            }
            None => (raw.to_owned(), None),
        };
        let refs: Vec<&str> = self.candidates.iter().map(String::as_str).collect();
        let items = fuzzy::rank(&needle, &refs)
            .into_iter()
            .map(|matched| PopupItem {
                text: refs[matched.index].to_owned(),
                match_positions: matched.positions,
                style: theme::variable(true),
            })
            .collect();
        self.completion_function = explicit_function;
        self.popup.open(items);
    }

    fn accept_completion(&mut self, index: usize) {
        let Some(item) = self.popup.items().get(index) else {
            return;
        };
        let mut replacement = item.text.clone();
        if let Some(function) = self.completion_function.take() {
            replacement.push(':');
            replacement.push_str(&function);
        }
        self.inputs[self.focus].set_value(replacement);
        self.popup.close();
    }

    fn close_popup(&mut self) {
        self.popup.close();
        self.completion_function = None;
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        if area.is_empty() {
            return;
        }
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(area);
        for (index, hook) in ScriptHook::ALL.into_iter().enumerate() {
            let Some(row) = rows.get(index).copied() else {
                continue;
            };
            let input_focused = focused && self.focus == index;
            let block = Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(theme::border(input_focused))
                .title(Line::from(hook.label()).style(theme::border_title(input_focused)));
            let inner = block.inner(row);
            block.render(row, buffer);
            self.inputs[index].render(inner, buffer, input_focused, &[]);
            if input_focused {
                self.popup_anchor = Rect::new(
                    inner.x + self.inputs[index].caret_column(inner.width as usize),
                    inner.y,
                    1,
                    1,
                );
            }
        }
    }

    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        self.popup.render(self.popup_anchor, screen, buffer);
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_owned())
}

fn collect_javascript(root: &Path, directory: &Path, output: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
    for entry in entries {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_javascript(root, &path, output);
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("js"))
            && let Ok(relative) = path.strip_prefix(root)
        {
            output.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_relative_and_actions_strip_function_suffix() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::create_dir(directory.path().join("scripts")).expect("scripts dir");
        fs::write(directory.path().join("scripts/hooks.js"), "export {};").expect("script");
        let mut tab = ScriptsTab::new(directory.path().to_owned());
        assert_eq!(tab.candidates, vec!["scripts/hooks.js"]);
        tab.inputs[0].set_value("scripts/hooks.js:custom");
        assert_eq!(
            tab.resolved_path(),
            Some(directory.path().join("scripts/hooks.js"))
        );
        assert_eq!(
            tab.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            ScriptsAction::OpenInEditor(directory.path().join("scripts/hooks.js"))
        );
        assert_eq!(
            tab.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT)),
            ScriptsAction::OpenInPager(directory.path().join("scripts/hooks.js"))
        );
        assert_eq!(
            tab.handle_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)),
            ScriptsAction::Ignored
        );
        assert_eq!(
            tab.handle_key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE)),
            ScriptsAction::Ignored
        );
    }

    #[test]
    fn empty_inputs_are_absent_from_model() {
        let tab = ScriptsTab::new(PathBuf::from("."));
        assert!(tab.to_model().is_empty());
    }
}
