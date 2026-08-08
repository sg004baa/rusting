//! Editable URL query parameters.

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use rusting_core::{KeyValue, RequestModel, Variables};

use crate::panes::key_value::{KeyValueAction, KeyValueEditor};

pub struct QueryTab {
    editor: KeyValueEditor,
}

impl QueryTab {
    pub fn new() -> Self {
        Self {
            editor: KeyValueEditor::new(["Key", "Value"], "Add", "No query parameters"),
        }
    }

    pub fn load(&mut self, request: &RequestModel) {
        self.editor.set_rows(request.params.clone());
    }

    pub fn to_model(&self) -> Vec<KeyValue> {
        self.editor.rows().to_vec()
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

    pub fn handle_key(&mut self, key: KeyEvent, variables: &Variables) -> KeyValueAction {
        self.editor.handle_key(key, variables)
    }

    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        focused: bool,
        variables: &Variables,
    ) {
        self.editor.render(area, buffer, focused, variables);
    }

    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        self.editor.render_overlay(screen, buffer);
    }

    pub(crate) fn selected(&self) -> Option<&KeyValue> {
        self.editor.table.selected()
    }
}

impl Default for QueryTab {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_replaces_rows() {
        let request = RequestModel {
            params: vec![KeyValue::new("page", "2")],
            ..RequestModel::default()
        };
        let mut tab = QueryTab::new();
        tab.load(&request);
        assert_eq!(tab.to_model(), request.params);
    }
}
