//! Editable request headers with MDN-backed name and value completion.

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use rusting_core::{KeyValue, RequestModel, Variables, header_names};

use crate::panes::key_value::{KeyValueAction, KeyValueEditor, KeyValueField};
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

    pub fn focus_first_control(&mut self) {
        self.editor.focus_first_control();
    }

    pub fn focus_last_control(&mut self) {
        self.editor.focus_last_control();
    }

    pub fn handle_key(&mut self, key: KeyEvent, variables: &Variables) -> KeyValueAction {
        self.editor.handle_key(key, variables)
    }

    pub fn apply_external_edit(&mut self, field: KeyValueField, text: &str) -> Result<(), String> {
        self.editor.apply_external_edit(field, text)
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
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn complete_authorization_value(prefix: &str, expected: &str) {
        let variables = Variables::new();
        let mut tab = HeadersTab::new();
        tab.focus_first_control();
        for character in "Autho".chars() {
            tab.handle_key(key(KeyCode::Char(character)), &variables);
        }
        assert_eq!(
            tab.handle_key(key(KeyCode::Tab), &variables),
            KeyValueAction::Consumed
        );
        assert_eq!(
            tab.handle_key(key(KeyCode::Tab), &variables),
            KeyValueAction::Consumed
        );
        for character in prefix.chars() {
            tab.handle_key(key(KeyCode::Char(character)), &variables);
        }
        assert_eq!(
            tab.handle_key(key(KeyCode::Tab), &variables),
            KeyValueAction::Consumed
        );
        assert_eq!(
            tab.handle_key(key(KeyCode::Enter), &variables),
            KeyValueAction::Changed
        );
        assert_eq!(
            tab.to_model(),
            vec![KeyValue::new("Authorization", expected)]
        );
    }

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

    #[test]
    fn typing_and_tab_complete_authorization_and_bearer() {
        complete_authorization_value("Bea", "Bearer ");
    }

    #[test]
    fn typing_and_tab_complete_basic_and_digest_authorization_values() {
        complete_authorization_value("Bas", "Basic ");
        complete_authorization_value("Dig", "Digest ");
    }
}
