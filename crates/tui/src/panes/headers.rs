//! Editable request headers with MDN-backed name and value completion.

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use rusting_core::{KeyValue, RequestModel, Variables, header_names};

use crate::panes::key_value::{KeyValueAction, KeyValueEditor};
use crate::theme;

pub struct HeadersTab {
    editor: KeyValueEditor,
}

impl HeadersTab {
    pub fn new() -> Self {
        let mut editor = KeyValueEditor::new(["Header", "Value"], "Add", "No headers");
        editor.key_candidates = header_names::REQUEST_HEADERS
            .iter()
            .map(|header| header.name.to_owned())
            .collect();
        editor.value_candidates = header_values;
        editor.key_candidate_style = header_style;
        Self { editor }
    }

    pub fn load(&mut self, request: &RequestModel) {
        self.editor.set_rows(request.headers.clone());
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

impl Default for HeadersTab {
    fn default() -> Self {
        Self::new()
    }
}

fn header_values(name: &str) -> Vec<String> {
    header_names::values_for(name)
        .iter()
        .map(|value| (*value).to_owned())
        .collect()
}

fn header_style(name: &str) -> Style {
    if header_names::REQUEST_HEADERS
        .iter()
        .any(|header| header.experimental && header.name.eq_ignore_ascii_case(name))
    {
        Style::new().fg(theme::WARNING)
    } else {
        Style::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_round_trip_preserves_enabled_rows() {
        let mut request = RequestModel::default();
        let mut disabled = KeyValue::new("X-Test", "one");
        disabled.enabled = false;
        request.headers = vec![
            disabled.clone(),
            KeyValue::new("Accept", "application/json"),
        ];
        let mut tab = HeadersTab::new();
        tab.load(&request);
        assert_eq!(tab.to_model(), request.headers);
        assert!(tab.has_content());
    }
}
