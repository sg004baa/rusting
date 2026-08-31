//! URL-derived path parameters.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};
use rusting_core::{KeyValue, PathParam, RequestModel, Variables, urls};

use crate::panes::key_value::{KeyValueAction, KeyValueEditor, KeyValueField};

const EMPTY_MESSAGE: &str =
    "No path parameters in URL\nUse :param syntax to add them\ne.g. http://example.com/:foo/:bar";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathAction {
    Ignored,
    Consumed,
    Changed,
    Renamed {
        old: String,
        new: String,
    },
    OpenInEditor {
        field: KeyValueField,
        contents: String,
    },
    JumpToUrl(String),
    LeaveUp,
    LeaveDown,
}

pub struct PathTab {
    editor: KeyValueEditor,
}

impl PathTab {
    pub fn new() -> Self {
        let mut editor = KeyValueEditor::new(["Parameter", "Value"], "Update", "");
        editor.allow_add = false;
        editor.table.toggles = false;
        editor.table.removable = false;
        Self { editor }
    }

    pub fn sync_from_url(&mut self, url: &str) {
        let old = self.editor.rows();
        let rows = urls::path_param_names(url)
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                KeyValue::new(name, old.get(index).map_or("", |row| row.value.as_str()))
            })
            .collect();
        self.editor.set_rows(rows);
    }

    pub fn load(&mut self, request: &RequestModel) {
        let rows = urls::path_param_names(&request.url)
            .into_iter()
            .map(|name| {
                let value = request
                    .path_params
                    .iter()
                    .find(|parameter| parameter.name == name)
                    .map_or("", |parameter| parameter.value.as_str());
                KeyValue::new(name, value)
            })
            .collect();
        self.editor.set_rows(rows);
    }

    pub fn to_model(&self) -> Vec<PathParam> {
        self.editor
            .rows()
            .iter()
            .map(|row| PathParam {
                name: row.name.clone(),
                value: row.value.clone(),
            })
            .collect()
    }

    pub fn has_content(&self) -> bool {
        !self.editor.rows().is_empty()
    }

    pub fn is_editing(&self) -> bool {
        self.editor.is_editing()
    }

    pub fn focus_first_control(&mut self) {
        self.editor.focus_first_control();
    }

    pub fn focus_last_control(&mut self) {
        self.editor.focus_last_control();
    }

    pub fn apply_external_edit(&mut self, field: KeyValueField, text: &str) -> Result<(), String> {
        self.editor.apply_external_edit(field, text)
    }

    pub fn handle_key(&mut self, key: KeyEvent, variables: &Variables) -> PathAction {
        if key.code == KeyCode::Down && key.modifiers.contains(KeyModifiers::ALT) {
            return self
                .editor
                .table
                .selected()
                .map(|row| PathAction::JumpToUrl(row.name.clone()))
                .unwrap_or(PathAction::Consumed);
        }
        let before = self.editor.table.selected().cloned();
        match self.editor.handle_key(key, variables) {
            KeyValueAction::Ignored => PathAction::Ignored,
            KeyValueAction::OpenInEditor { field, contents } => {
                PathAction::OpenInEditor { field, contents }
            }
            KeyValueAction::Consumed | KeyValueAction::CopyRequested => PathAction::Consumed,
            KeyValueAction::LeaveUp => PathAction::LeaveUp,
            KeyValueAction::LeaveDown => PathAction::LeaveDown,
            KeyValueAction::Changed => {
                let after = self.editor.table.selected();
                match (before, after) {
                    (Some(old), Some(new)) if old.name != new.name => PathAction::Renamed {
                        old: old.name,
                        new: new.name.clone(),
                    },
                    _ => PathAction::Changed,
                }
            }
        }
    }

    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        focused: bool,
        variables: &Variables,
    ) {
        self.editor.render(area, buffer, focused, variables);
        if self.editor.rows().is_empty() {
            let table_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(3));
            let lines = EMPTY_MESSAGE
                .lines()
                .map(|line| {
                    Line::from(Span::styled(line, Style::new().fg(crate::theme::ACCENT))).centered()
                })
                .collect::<Vec<_>>();
            Paragraph::new(lines).centered().render(table_area, buffer);
        }
    }

    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        self.editor.render_overlay(screen, buffer);
    }

    pub(crate) fn selected(&self) -> Option<&KeyValue> {
        self.editor.table.selected()
    }
}

impl Default for PathTab {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_sync_preserves_values_by_position() {
        let mut tab = PathTab::new();
        tab.sync_from_url("https://example.test/:id/:part");
        tab.editor.table.selected_mut().expect("first row").value = "42".into();
        tab.sync_from_url("https://example.test/:userId/:part");
        assert_eq!(
            tab.to_model()[0],
            PathParam {
                name: "userId".into(),
                value: "42".into()
            }
        );
    }

    #[test]
    fn load_matches_saved_values_by_name() {
        let mut request = RequestModel {
            url: "https://example.test/:second/:first".into(),
            ..RequestModel::default()
        };
        request.path_params = vec![
            PathParam {
                name: "first".into(),
                value: "1".into(),
            },
            PathParam {
                name: "second".into(),
                value: "2".into(),
            },
        ];
        let mut tab = PathTab::new();
        tab.load(&request);
        assert_eq!(tab.to_model()[0].value, "2");
        assert_eq!(tab.to_model()[1].value, "1");
    }
}
