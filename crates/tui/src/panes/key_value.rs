//! Editable name/value rows with add, update and completion controls.

use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Widget as _};
use rusting_core::{KeyValue, Variables, variables};

use crate::theme;
use crate::widgets::fuzzy;
use crate::widgets::highlight;
use crate::widgets::input::{Input, InputAction};
use crate::widgets::popup::{Popup, PopupAction, PopupItem};
use crate::widgets::table::{KeyValueTable, TableAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyValueAction {
    Ignored,
    Consumed,
    Changed,
    CopyRequested,
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Table,
    Key,
    Value,
    Button,
}

#[derive(Debug, Clone)]
enum Mode {
    Idle,
    Adding,
    Editing { index: usize, original: KeyValue },
}

#[derive(Debug, Clone)]
enum Completion {
    Key,
    Value,
    Variable { range: Range<usize>, braced: bool },
}

fn no_values(_: &str) -> Vec<String> {
    Vec::new()
}

fn plain_style(_: &str) -> Style {
    Style::new()
}

pub struct KeyValueEditor {
    pub table: KeyValueTable,
    pub key_candidates: Vec<String>,
    pub value_candidates: fn(&str) -> Vec<String>,
    /// Derived tables such as path parameters can edit but cannot append rows.
    pub allow_add: bool,
    /// Per-candidate styling, used to mark experimental request headers.
    pub key_candidate_style: fn(&str) -> Style,
    add_label: &'static str,
    key: Input,
    value: Input,
    focus: Focus,
    mode: Mode,
    popup: Popup,
    completion: Option<Completion>,
    popup_anchor: Rect,
}

impl KeyValueEditor {
    pub fn new(columns: [&'static str; 2], add_label: &'static str, empty_message: &str) -> Self {
        let mut table = KeyValueTable::new(columns);
        table.empty_message = empty_message.to_owned();
        Self {
            table,
            key_candidates: Vec::new(),
            value_candidates: no_values,
            allow_add: true,
            key_candidate_style: plain_style,
            add_label,
            key: Input::with_placeholder(columns[0]),
            value: Input::with_placeholder(columns[1]),
            focus: Focus::Table,
            mode: Mode::Idle,
            popup: Popup::new(),
            completion: None,
            popup_anchor: Rect::ZERO,
        }
    }

    pub fn rows(&self) -> &[KeyValue] {
        self.table.rows()
    }

    pub fn set_rows(&mut self, rows: Vec<KeyValue>) {
        self.cancel_edit();
        self.table.set_rows(rows);
    }

    pub fn is_editing(&self) -> bool {
        !matches!(self.mode, Mode::Idle)
    }

    /// The row currently being updated, if any.
    pub fn editing(&self) -> Option<&KeyValue> {
        match self.mode {
            Mode::Editing { index, .. } => self.table.rows().get(index),
            Mode::Idle | Mode::Adding => None,
        }
    }

    /// Start idle traversal at the first existing row, or at the key input
    /// when empty. An in-progress add or edit keeps its draft.
    pub fn focus_first_control(&mut self) {
        if self.allow_add
            && matches!(self.mode, Mode::Adding)
            && self.key.is_empty()
            && self.value.is_empty()
        {
            self.mode = Mode::Idle;
        }

        if self.allow_add && matches!(self.mode, Mode::Idle) && self.table.selected().is_some() {
            self.table.set_cursor(0);
            self.focus = Focus::Table;
            self.close_popup();
        } else if self.allow_add {
            if matches!(self.mode, Mode::Idle) {
                self.mode = Mode::Adding;
            }
            self.focus = Focus::Key;
            self.close_popup();
        } else if self.table.selected().is_some() {
            if matches!(self.mode, Mode::Editing { .. }) {
                self.focus = Focus::Key;
                self.close_popup();
            } else {
                self.begin_editing(true);
            }
        } else {
            self.focus = Focus::Table;
        }
    }

    pub fn focus_last_control(&mut self) {
        if self.allow_add {
            self.ensure_adding();
            self.focus = Focus::Button;
        } else if self.table.selected().is_some() {
            if matches!(self.mode, Mode::Editing { .. }) {
                self.focus = Focus::Value;
            } else {
                self.begin_editing(false);
            }
        } else {
            self.focus = Focus::Table;
        }
        self.close_popup();
    }

    pub fn handle_key(&mut self, key: KeyEvent, variables: &Variables) -> KeyValueAction {
        let backward = key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab
                && key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::SHIFT));
        if self.popup.is_open() && !backward {
            match self.popup.handle_key(key) {
                PopupAction::Accepted(index) => {
                    self.accept_completion(index);
                    return KeyValueAction::Consumed;
                }
                PopupAction::Dismissed => {
                    self.close_popup();
                    return KeyValueAction::Consumed;
                }
                PopupAction::Consumed => return KeyValueAction::Consumed,
                PopupAction::Ignored => {}
            }
        }

        if key.code == KeyCode::Esc && self.is_editing() {
            self.cancel_edit();
            return KeyValueAction::Consumed;
        }

        match self.focus {
            Focus::Table => self.handle_table_key(key),
            Focus::Key => self.handle_input_key(key, true, variables),
            Focus::Value => self.handle_input_key(key, false, variables),
            Focus::Button => self.handle_button_key(key),
        }
    }

    fn handle_table_key(&mut self, key: KeyEvent) -> KeyValueAction {
        if key.code == KeyCode::Tab {
            if self.allow_add {
                self.begin_adding();
                return KeyValueAction::Consumed;
            }
            if self.table.selected().is_some() {
                self.begin_editing(true);
                return KeyValueAction::Consumed;
            }
            return KeyValueAction::LeaveDown;
        }
        if key.code == KeyCode::BackTab {
            return KeyValueAction::LeaveUp;
        }

        match self.table.handle_key(key) {
            TableAction::Ignored => KeyValueAction::Ignored,
            TableAction::Consumed => KeyValueAction::Consumed,
            TableAction::Removed | TableAction::Toggled => KeyValueAction::Changed,
            TableAction::Copy => KeyValueAction::CopyRequested,
            TableAction::LeaveUp => KeyValueAction::LeaveUp,
            TableAction::LeaveDown => {
                if self.allow_add {
                    self.begin_adding();
                    KeyValueAction::Consumed
                } else {
                    KeyValueAction::LeaveDown
                }
            }
            TableAction::EditKey => {
                self.begin_editing(true);
                KeyValueAction::Consumed
            }
            TableAction::EditValue => {
                self.begin_editing(false);
                KeyValueAction::Consumed
            }
        }
    }

    fn handle_input_key(
        &mut self,
        key: KeyEvent,
        is_key: bool,
        variables: &Variables,
    ) -> KeyValueAction {
        if key.code == KeyCode::Tab {
            self.close_popup();
            if is_key {
                self.focus = Focus::Value;
                return KeyValueAction::Consumed;
            }
            if self.allow_add {
                self.focus = Focus::Button;
                return KeyValueAction::Consumed;
            }
            return KeyValueAction::LeaveDown;
        }
        if key.code == KeyCode::BackTab {
            self.close_popup();
            if is_key {
                return KeyValueAction::LeaveUp;
            }
            self.focus = Focus::Key;
            return KeyValueAction::Consumed;
        }
        if key.code == KeyCode::Down && key.modifiers.is_empty() {
            self.refresh_completions(is_key, variables);
            if self.popup.is_open() {
                return KeyValueAction::Consumed;
            }
        }

        let action = if is_key {
            self.key.handle_key(key)
        } else {
            self.value.handle_key(key)
        };
        match action {
            InputAction::Changed => {
                self.ensure_adding();
                self.refresh_completions(is_key, variables);
                KeyValueAction::Consumed
            }
            InputAction::Submitted => self.submit_from_input(is_key),
            InputAction::LeaveUp => {
                self.close_popup();
                if is_key {
                    self.focus = Focus::Table;
                } else {
                    self.focus = Focus::Key;
                }
                KeyValueAction::Consumed
            }
            InputAction::LeaveDown => {
                self.close_popup();
                if is_key {
                    self.focus = Focus::Value;
                } else if self.allow_add {
                    self.focus = Focus::Button;
                } else {
                    return KeyValueAction::LeaveDown;
                }
                KeyValueAction::Consumed
            }
            InputAction::Consumed => KeyValueAction::Consumed,
            InputAction::Ignored => KeyValueAction::Ignored,
        }
    }

    fn handle_button_key(&mut self, key: KeyEvent) -> KeyValueAction {
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => self.commit(),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.focus = Focus::Value;
                KeyValueAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => KeyValueAction::LeaveDown,
            _ => KeyValueAction::Ignored,
        }
    }

    fn submit_from_input(&mut self, is_key: bool) -> KeyValueAction {
        if matches!(self.mode, Mode::Editing { .. }) && !self.key.is_empty() {
            return self.commit();
        }
        if !self.key.is_empty() && !self.value.is_empty() {
            return self.commit();
        }
        if is_key && !self.key.is_empty() {
            self.focus = Focus::Value;
        } else if !is_key && !self.value.is_empty() {
            self.focus = Focus::Key;
        }
        KeyValueAction::Consumed
    }

    fn commit(&mut self) -> KeyValueAction {
        let valid = match self.mode {
            Mode::Editing { .. } => !self.key.is_empty(),
            Mode::Adding | Mode::Idle => !self.key.is_empty() && !self.value.is_empty(),
        };
        if !valid {
            self.focus = if self.key.is_empty() {
                Focus::Key
            } else {
                Focus::Value
            };
            return KeyValueAction::Consumed;
        }
        let row = KeyValue::new(self.key.value(), self.value.value());
        match self.mode.clone() {
            Mode::Editing { index, original } => {
                let mut row = row;
                row.enabled = original.enabled;
                if self.table.cursor() == index
                    && let Some(current) = self.table.selected_mut()
                {
                    *current = row;
                }
                self.finish_to_idle();
            }
            Mode::Adding | Mode::Idle if self.allow_add => {
                self.table.push(row);
                self.key.clear();
                self.value.clear();
                self.mode = Mode::Adding;
                self.focus = Focus::Key;
                self.close_popup();
            }
            Mode::Adding => return KeyValueAction::Consumed,
            Mode::Idle => return KeyValueAction::Consumed,
        }
        KeyValueAction::Changed
    }

    fn begin_adding(&mut self) {
        self.mode = Mode::Adding;
        self.key.clear();
        self.value.clear();
        self.focus = Focus::Key;
        self.close_popup();
    }

    fn ensure_adding(&mut self) {
        if matches!(self.mode, Mode::Idle) && self.allow_add {
            self.mode = Mode::Adding;
        }
    }

    fn begin_editing(&mut self, key_first: bool) {
        let index = self.table.cursor();
        let Some(row) = self.table.selected().cloned() else {
            return;
        };
        self.key.set_value(&row.name);
        self.value.set_value(&row.value);
        self.mode = Mode::Editing {
            index,
            original: row,
        };
        self.focus = if key_first { Focus::Key } else { Focus::Value };
        self.close_popup();
    }

    fn cancel_edit(&mut self) {
        if let Mode::Editing {
            index,
            ref original,
        } = self.mode
        {
            self.table.set_cursor(index);
            if let Some(row) = self.table.selected_mut() {
                *row = original.clone();
            }
        }
        self.finish_to_idle();
    }

    fn finish_to_idle(&mut self) {
        self.key.clear();
        self.value.clear();
        self.mode = Mode::Idle;
        self.focus = Focus::Table;
        self.close_popup();
    }

    fn close_popup(&mut self) {
        self.popup.close();
        self.completion = None;
    }

    fn refresh_completions(&mut self, is_key: bool, variables_map: &Variables) {
        let input = if is_key { &self.key } else { &self.value };
        if let Some(token) = variables::variable_at_cursor(input.value(), input.cursor()) {
            let candidates: Vec<&str> = variables_map.keys().map(String::as_str).collect();
            let items = fuzzy::rank(&token.name, &candidates)
                .into_iter()
                .map(|matched| PopupItem {
                    text: format!("${}", candidates[matched.index]),
                    match_positions: matched
                        .positions
                        .into_iter()
                        .map(|position| position + 1)
                        .collect(),
                    style: theme::variable(true),
                })
                .collect();
            self.completion = Some(Completion::Variable {
                range: token.start..token.end,
                braced: token.braced,
            });
            self.popup.open(items);
            return;
        }

        let candidates = if is_key {
            self.key_candidates.clone()
        } else {
            (self.value_candidates)(self.key.value())
        };
        let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        let needle = input.value();
        let style_for = self.key_candidate_style;
        let items = fuzzy::rank(needle, &refs)
            .into_iter()
            .map(|matched| PopupItem {
                text: refs[matched.index].to_owned(),
                match_positions: matched.positions,
                style: if is_key {
                    style_for(refs[matched.index])
                } else {
                    Style::new()
                },
            })
            .collect();
        self.completion = Some(if is_key {
            Completion::Key
        } else {
            Completion::Value
        });
        self.popup.open(items);
        if !self.popup.is_open() {
            self.completion = None;
        }
    }

    fn accept_completion(&mut self, index: usize) {
        let Some(item) = self.popup.items().get(index) else {
            return;
        };
        let text = item.text.clone();
        match self.completion.take() {
            Some(Completion::Key) => self.key.set_value(text),
            Some(Completion::Value) => self.value.set_value(text),
            Some(Completion::Variable { range, braced }) => {
                let name = text.strip_prefix('$').unwrap_or(&text);
                let replacement = if braced {
                    format!("${{{name}}}")
                } else {
                    format!("${name}")
                };
                let input = if self.focus == Focus::Key {
                    &mut self.key
                } else {
                    &mut self.value
                };
                input.splice(range, &replacement);
            }
            None => {}
        }
        self.popup.close();
    }

    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        focused: bool,
        variables_map: &Variables,
    ) {
        if area.is_empty() {
            return;
        }
        let controls_height = if area.height >= 3 { 3 } else { area.height };
        let [table_area, controls] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(controls_height)]).areas(area);
        let editing_index = match self.mode {
            Mode::Editing { index, .. } => Some(index),
            Mode::Idle | Mode::Adding => None,
        };
        self.table.render_with(
            table_area,
            buffer,
            focused && self.focus == Focus::Table,
            &|index| {
                (Some(index) == editing_index).then(|| {
                    Style::new()
                        .fg(theme::WARNING)
                        .add_modifier(Modifier::ITALIC)
                })
            },
        );

        let button_width = self.add_label.len().max("Update".len()) as u16 + 4;
        let widths = if self.allow_add {
            [
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Length(button_width),
            ]
        } else {
            [
                Constraint::Percentage(50),
                Constraint::Percentage(50),
                Constraint::Length(0),
            ]
        };
        let [key_area, value_area, button_area] = Layout::horizontal(widths).areas(controls);
        let key_focused = focused && self.focus == Focus::Key;
        let value_focused = focused && self.focus == Focus::Value;
        let key_inner = render_control(key_area, buffer, key_focused, None);
        let value_inner = render_control(value_area, buffer, value_focused, None);
        let key_highlights = highlight::variables(
            self.key.value(),
            variables_map,
            key_focused.then(|| self.key.cursor()),
        );
        let value_highlights = highlight::variables(
            self.value.value(),
            variables_map,
            value_focused.then(|| self.value.cursor()),
        );
        self.key
            .render(key_inner, buffer, key_focused, &key_highlights);
        self.value
            .render(value_inner, buffer, value_focused, &value_highlights);

        if self.allow_add && !button_area.is_empty() {
            let button_focused = focused && self.focus == Focus::Button;
            let label = if matches!(self.mode, Mode::Editing { .. }) {
                "Update"
            } else {
                self.add_label
            };
            let inner = render_control(button_area, buffer, button_focused, None);
            Line::from(label).centered().render(inner, buffer);
        }

        let (input, inner) = if self.focus == Focus::Key {
            (&self.key, key_inner)
        } else {
            (&self.value, value_inner)
        };
        self.popup_anchor = Rect::new(
            inner.x + input.caret_column(inner.width as usize),
            inner.y,
            1,
            1,
        );
    }

    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        self.popup.render(self.popup_anchor, screen, buffer);
    }
}

fn render_control(area: Rect, buffer: &mut Buffer, focused: bool, title: Option<&str>) -> Rect {
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::border(focused));
    if let Some(title) = title {
        block = block.title(title);
    }
    let inner = block.inner(area);
    block.render(area, buffer);
    inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn adds_updates_and_cancels_rows() {
        let vars = Variables::new();
        let mut editor = KeyValueEditor::new(["Key", "Value"], "Add", "empty");
        assert_eq!(
            editor.handle_key(key(KeyCode::Down), &vars),
            KeyValueAction::Consumed
        );
        for c in "name".chars() {
            editor.handle_key(key(KeyCode::Char(c)), &vars);
        }
        editor.handle_key(key(KeyCode::Enter), &vars);
        for c in "value".chars() {
            editor.handle_key(key(KeyCode::Char(c)), &vars);
        }
        assert_eq!(
            editor.handle_key(key(KeyCode::Enter), &vars),
            KeyValueAction::Changed
        );
        assert_eq!(editor.rows(), &[KeyValue::new("name", "value")]);

        editor.finish_to_idle();
        editor.handle_key(key(KeyCode::Enter), &vars);
        editor.key.set_value("changed");
        editor.handle_key(key(KeyCode::Esc), &vars);
        assert_eq!(editor.rows()[0].name, "name");

        editor.handle_key(key(KeyCode::Enter), &vars);
        editor.key.set_value("changed");
        editor.value.set_value("again");
        assert_eq!(
            editor.handle_key(key(KeyCode::Enter), &vars),
            KeyValueAction::Changed
        );
        assert_eq!(editor.rows()[0], KeyValue::new("changed", "again"));
    }

    #[test]
    fn focus_first_control_prefers_an_existing_row_over_adding() {
        let vars = Variables::new();
        let mut populated = KeyValueEditor::new(["Key", "Value"], "Add", "empty");
        populated.set_rows(vec![
            KeyValue::new("first", "one"),
            KeyValue::new("second", "two"),
        ]);
        populated.table.set_cursor(1);

        populated.focus_first_control();
        assert_eq!(populated.focus, Focus::Table);
        assert_eq!(populated.table.cursor(), 0);
        assert_eq!(
            populated.handle_key(key(KeyCode::Enter), &vars),
            KeyValueAction::Consumed
        );
        assert_eq!(populated.focus, Focus::Key);
        assert_eq!(populated.editing().unwrap().name, "first");
        populated.key.set_value("changed");
        populated.focus = Focus::Value;
        populated.focus_first_control();
        assert_eq!(populated.focus, Focus::Key);
        assert_eq!(populated.key.value(), "changed");
        assert_eq!(
            populated.handle_key(key(KeyCode::Enter), &vars),
            KeyValueAction::Changed
        );
        assert_eq!(populated.rows()[0], KeyValue::new("changed", "one"));
        assert_eq!(populated.rows()[1], KeyValue::new("second", "two"));

        let mut empty = KeyValueEditor::new(["Key", "Value"], "Add", "empty");
        empty.focus_first_control();
        assert_eq!(empty.focus, Focus::Key);
        assert!(matches!(empty.mode, Mode::Adding));
    }

    #[test]
    fn focus_first_control_preserves_an_active_add_draft() {
        let vars = Variables::new();
        let mut editor = KeyValueEditor::new(["Key", "Value"], "Add", "empty");
        editor.focus_first_control();
        editor.key.set_value("saved");
        editor.value.set_value("row");
        assert_eq!(
            editor.handle_key(key(KeyCode::Enter), &vars),
            KeyValueAction::Changed
        );

        editor.key.set_value("draft");
        editor.focus = Focus::Button;
        editor.focus_first_control();

        assert_eq!(editor.focus, Focus::Key);
        assert!(matches!(editor.mode, Mode::Adding));
        assert_eq!(editor.key.value(), "draft");
        assert!(editor.value.is_empty());
        assert_eq!(editor.rows(), &[KeyValue::new("saved", "row")]);
    }

    #[test]
    fn focus_first_control_returns_blank_post_add_state_to_existing_rows() {
        let vars = Variables::new();
        let mut editor = KeyValueEditor::new(["Key", "Value"], "Add", "empty");
        editor.focus_first_control();
        editor.key.set_value("first");
        editor.value.set_value("one");
        assert_eq!(
            editor.handle_key(key(KeyCode::Enter), &vars),
            KeyValueAction::Changed
        );
        editor.key.set_value("second");
        editor.value.set_value("two");
        assert_eq!(
            editor.handle_key(key(KeyCode::Enter), &vars),
            KeyValueAction::Changed
        );
        assert!(matches!(editor.mode, Mode::Adding));
        assert!(editor.key.is_empty());
        assert!(editor.value.is_empty());

        editor.focus_first_control();

        assert_eq!(editor.focus, Focus::Table);
        assert!(matches!(editor.mode, Mode::Idle));
        assert_eq!(editor.table.cursor(), 0);
        assert_eq!(
            editor.handle_key(key(KeyCode::Char('j')), &vars),
            KeyValueAction::Consumed
        );
        assert_eq!(editor.focus, Focus::Table);
        assert_eq!(editor.table.cursor(), 1);
    }

    #[test]
    fn tab_traverses_key_value_add_then_leaves() {
        let vars = Variables::new();
        let mut editor = KeyValueEditor::new(["Key", "Value"], "Add", "empty");
        editor.focus_first_control();
        assert_eq!(editor.focus, Focus::Key);
        assert_eq!(
            editor.handle_key(key(KeyCode::Tab), &vars),
            KeyValueAction::Consumed
        );
        assert_eq!(editor.focus, Focus::Value);
        assert_eq!(
            editor.handle_key(key(KeyCode::Tab), &vars),
            KeyValueAction::Consumed
        );
        assert_eq!(editor.focus, Focus::Button);
        assert_eq!(
            editor.handle_key(key(KeyCode::Tab), &vars),
            KeyValueAction::LeaveDown
        );
        assert_eq!(editor.focus, Focus::Button);
        assert_eq!(
            editor.handle_key(key(KeyCode::BackTab), &vars),
            KeyValueAction::Consumed
        );
        assert_eq!(editor.focus, Focus::Value);
    }

    #[test]
    fn backtab_traverses_backward_without_accepting_an_open_completion() {
        let vars = Variables::new();
        let mut editor = KeyValueEditor::new(["Key", "Value"], "Add", "empty");
        editor.key_candidates = vec!["Authorization".to_owned()];
        editor.focus_first_control();
        for character in "Auth".chars() {
            editor.handle_key(key(KeyCode::Char(character)), &vars);
        }
        assert!(editor.popup.is_open());

        assert_eq!(
            editor.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT), &vars,),
            KeyValueAction::LeaveUp
        );
        assert_eq!(editor.key.value(), "Auth");
        assert!(!editor.popup.is_open());
    }

    #[test]
    fn add_button_is_compact_and_commit_returns_to_key() {
        let vars = Variables::new();
        let mut editor = KeyValueEditor::new(["Key", "Value"], "Add", "empty");
        editor.focus_first_control();
        editor.key.set_value("name");
        editor.value.set_value("value");
        editor.focus = Focus::Button;
        assert_eq!(
            editor.handle_key(key(KeyCode::Enter), &vars),
            KeyValueAction::Changed
        );
        assert_eq!(editor.focus, Focus::Key);

        let area = Rect::new(0, 0, 80, 10);
        let mut buffer = Buffer::empty(area);
        editor.render(area, &mut buffer, true, &vars);
        assert_eq!(buffer[(70, 7)].symbol(), "╭");
        assert_eq!(buffer[(79, 7)].symbol(), "╮");
    }

    #[test]
    fn variable_completion_replaces_the_token() {
        let vars = [("TOKEN".to_owned(), "secret".to_owned())]
            .into_iter()
            .collect();
        let mut editor = KeyValueEditor::new(["Key", "Value"], "Add", "");
        editor.begin_adding();
        editor.key.set_value("x");
        editor.value.set_value("$TO");
        editor.focus = Focus::Value;
        editor.refresh_completions(false, &vars);
        assert!(matches!(
            editor.completion,
            Some(Completion::Variable { .. })
        ));
        editor.accept_completion(0);
        assert_eq!(editor.value.value(), "$TOKEN");
    }
}
