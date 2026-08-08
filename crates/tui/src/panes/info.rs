//! Request name, description and on-disk path metadata.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Widget as _};
use rusting_core::RequestModel;

use crate::theme;
use crate::widgets::editor::{Editor, EditorAction};
use crate::widgets::input::{Input, InputAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoAction {
    Ignored,
    Consumed,
    Changed,
    OpenInPager,
    OpenInEditor,
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Name,
    Description,
    Path,
}

pub struct InfoTab {
    name: Input,
    description: Editor,
    path: Input,
    focus: Focus,
}

impl InfoTab {
    pub fn new() -> Self {
        let mut description = Editor::new();
        description.set_show_line_numbers(false);
        let mut path = Input::new();
        path.read_only = true;
        Self {
            name: Input::with_placeholder("Request name"),
            description,
            path,
            focus: Focus::Name,
        }
    }

    pub fn load(&mut self, request: &RequestModel) {
        self.name.set_value(&request.name);
        self.description.set_text(&request.description);
        self.path.set_value(
            request
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Request not saved to disk.".to_owned()),
        );
        self.focus = Focus::Name;
    }
    pub fn to_model(&self) -> (String, String) {
        (self.name.value().to_owned(), self.description.text())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> InfoAction {
        if key.code == KeyCode::Tab {
            return self.move_down();
        }
        if key.code == KeyCode::BackTab {
            return self.move_up();
        }
        match self.focus {
            Focus::Name => map_input(
                self.name.handle_key(key),
                &mut self.focus,
                Focus::Description,
            ),
            Focus::Description => match self.description.handle_key(key) {
                EditorAction::Ignored => InfoAction::Ignored,
                EditorAction::Consumed => InfoAction::Consumed,
                EditorAction::Changed => InfoAction::Changed,
                EditorAction::OpenInPager => InfoAction::OpenInPager,
                EditorAction::OpenInEditor => InfoAction::OpenInEditor,
                EditorAction::LeaveUp => {
                    self.focus = Focus::Name;
                    InfoAction::Consumed
                }
                EditorAction::LeaveDown => {
                    self.focus = Focus::Path;
                    InfoAction::Consumed
                }
            },
            Focus::Path => match self.path.handle_key(key) {
                InputAction::LeaveUp => {
                    self.focus = Focus::Description;
                    InfoAction::Consumed
                }
                InputAction::LeaveDown => InfoAction::LeaveDown,
                InputAction::Ignored => InfoAction::Ignored,
                InputAction::Consumed | InputAction::Changed | InputAction::Submitted => {
                    InfoAction::Consumed
                }
            },
        }
    }

    fn move_up(&mut self) -> InfoAction {
        self.focus = match self.focus {
            Focus::Name => return InfoAction::LeaveUp,
            Focus::Description => Focus::Name,
            Focus::Path => Focus::Description,
        };
        InfoAction::Consumed
    }

    fn move_down(&mut self) -> InfoAction {
        self.focus = match self.focus {
            Focus::Name => Focus::Description,
            Focus::Description => Focus::Path,
            Focus::Path => return InfoAction::LeaveDown,
        };
        InfoAction::Consumed
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        if area.is_empty() {
            return;
        }
        let [name_area, description_area, path_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .areas(area);
        let name_focused = focused && self.focus == Focus::Name;
        let description_focused = focused && self.focus == Focus::Description;
        let path_focused = focused && self.focus == Focus::Path;
        let name_inner = bordered(name_area, buffer, name_focused, "Name");
        let description_inner =
            bordered(description_area, buffer, description_focused, "Description");
        let path_inner = bordered(path_area, buffer, path_focused, "Path");
        self.name.render(name_inner, buffer, name_focused, &[]);
        self.description
            .render(description_inner, buffer, description_focused);
        self.path.render(path_inner, buffer, path_focused, &[]);
    }

    pub fn render_overlay(&mut self, _screen: Rect, _buffer: &mut Buffer) {}

    pub(crate) fn editor_text(&self) -> String {
        self.description.text()
    }

    pub(crate) fn apply_external_edit(&mut self, text: &str) {
        self.description.set_text(text);
    }
}

impl Default for InfoTab {
    fn default() -> Self {
        Self::new()
    }
}

fn map_input(action: InputAction, focus: &mut Focus, next: Focus) -> InfoAction {
    match action {
        InputAction::Changed => InfoAction::Changed,
        InputAction::Consumed | InputAction::Submitted => InfoAction::Consumed,
        InputAction::LeaveUp => InfoAction::LeaveUp,
        InputAction::LeaveDown => {
            *focus = next;
            InfoAction::Consumed
        }
        InputAction::Ignored => InfoAction::Ignored,
    }
}

fn bordered(area: Rect, buffer: &mut Buffer, focused: bool, title: &str) -> Rect {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::border(focused))
        .title(Line::from(title).style(theme::border_title(focused)));
    let inner = block.inner(area);
    block.render(area, buffer);
    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trip_and_unsaved_path() {
        let request = RequestModel {
            name: "List users".into(),
            description: "Returns users".into(),
            ..RequestModel::default()
        };
        let mut tab = InfoTab::new();
        tab.load(&request);
        assert_eq!(tab.to_model(), (request.name, request.description));
        assert_eq!(tab.path.value(), "Request not saved to disk.");
    }
}
