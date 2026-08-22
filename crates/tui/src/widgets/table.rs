//! The two-column name/value table behind headers, query params, form data,
//! path params, response headers and cookies.
//!
//! Rows are [`KeyValue`]s so the table can hand them straight to the model. The
//! table owns navigation, the enable toggle and scrolling; it does **not** own
//! editing — it reports [`TableAction::EditKey`] / [`TableAction::EditValue`]
//! and the surrounding editor drives the inputs.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget as _};
use rusting_core::KeyValue;

use crate::theme;

/// Glyphs for the enable toggle. A disabled row shows a blank rather than a
/// cross so the column reads as a checklist.
const CHECKED: &str = "\u{2714}\u{fe0e}";
const UNCHECKED: &str = " ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAction {
    /// Not a table key.
    Ignored,
    /// Handled, nothing for the caller to do.
    Consumed,
    /// `enter` — edit the name cell of the selected row.
    EditKey,
    /// `v` — edit the value cell of the selected row.
    EditValue,
    /// `backspace` — the selected row was removed.
    Removed,
    /// `space` — the selected row's enable flag was flipped.
    Toggled,
    /// `c` / `y` — the caller opens the copy modal.
    Copy,
    /// The cursor tried to leave the table.
    LeaveUp,
    LeaveDown,
}

/// How the cursor behaves at the ends of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EdgeBehaviour {
    /// Report `LeaveUp` / `LeaveDown` so focus moves on. Used by the editable
    /// request tables, where the row inputs sit below the table.
    #[default]
    Escape,
    /// Wrap to the other end. Used by the read-only response tables, which have
    /// nothing to move to.
    Wrap,
}

#[derive(Debug, Clone)]
pub struct KeyValueTable {
    rows: Vec<KeyValue>,
    cursor: usize,
    state: TableState,
    pub columns: [&'static str; 2],
    pub show_header: bool,
    /// Render the enable-toggle column and honour `space`.
    pub toggles: bool,
    /// Allow `backspace` to remove rows.
    pub removable: bool,
    pub edge: EdgeBehaviour,
    /// Shown centred when there are no rows.
    pub empty_message: String,
}

impl KeyValueTable {
    pub fn new(columns: [&'static str; 2]) -> Self {
        Self {
            rows: Vec::new(),
            cursor: 0,
            state: TableState::default(),
            columns,
            show_header: false,
            toggles: true,
            removable: true,
            edge: EdgeBehaviour::Escape,
            empty_message: String::new(),
        }
    }

    /// A read-only table: no toggles, no removal, cursor wraps.
    pub fn read_only(columns: [&'static str; 2]) -> Self {
        Self {
            toggles: false,
            removable: false,
            edge: EdgeBehaviour::Wrap,
            ..Self::new(columns)
        }
    }

    pub fn rows(&self) -> &[KeyValue] {
        &self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Replaces every row. The cursor is clamped, not reset, so a programmatic
    /// refresh of a derived table (the Path tab) does not jump the selection.
    pub fn set_rows(&mut self, rows: Vec<KeyValue>) {
        self.rows = rows;
        self.clamp_cursor();
    }

    pub fn push(&mut self, row: KeyValue) {
        self.rows.push(row);
        self.cursor = self.rows.len() - 1;
    }

    /// Inserts at the cursor's position, keeping the cursor on the new row.
    pub fn insert(&mut self, index: usize, row: KeyValue) {
        let index = index.min(self.rows.len());
        self.rows.insert(index, row);
        self.cursor = index;
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
        self.clamp_cursor();
    }

    pub fn selected(&self) -> Option<&KeyValue> {
        self.rows.get(self.cursor)
    }

    pub fn selected_mut(&mut self) -> Option<&mut KeyValue> {
        self.rows.get_mut(self.cursor)
    }

    pub fn remove_selected(&mut self) -> Option<KeyValue> {
        if self.cursor >= self.rows.len() {
            return None;
        }
        let removed = self.rows.remove(self.cursor);
        self.clamp_cursor();
        Some(removed)
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.cursor = 0;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> TableAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Home => {
                    self.cursor = 0;
                    TableAction::Consumed
                }
                KeyCode::End => {
                    self.cursor = self.rows.len().saturating_sub(1);
                    TableAction::Consumed
                }
                _ => TableAction::Ignored,
            };
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Home | KeyCode::Char('g') => {
                self.cursor = 0;
                TableAction::Consumed
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.cursor = self.rows.len().saturating_sub(1);
                TableAction::Consumed
            }
            KeyCode::Enter if !self.rows.is_empty() => TableAction::EditKey,
            KeyCode::Char('v') if !self.rows.is_empty() => TableAction::EditValue,
            KeyCode::Char(' ') if self.toggles && !self.rows.is_empty() => {
                if let Some(row) = self.rows.get_mut(self.cursor) {
                    row.enabled = !row.enabled;
                }
                TableAction::Toggled
            }
            KeyCode::Backspace if self.removable && !self.rows.is_empty() => {
                self.remove_selected();
                TableAction::Removed
            }
            KeyCode::Char('c' | 'y') if !self.rows.is_empty() => TableAction::Copy,
            _ => TableAction::Ignored,
        }
    }

    fn move_up(&mut self) -> TableAction {
        if self.cursor > 0 {
            self.cursor -= 1;
            return TableAction::Consumed;
        }
        match self.edge {
            EdgeBehaviour::Escape => TableAction::LeaveUp,
            EdgeBehaviour::Wrap => {
                self.cursor = self.rows.len().saturating_sub(1);
                TableAction::Consumed
            }
        }
    }

    fn move_down(&mut self) -> TableAction {
        if self.cursor + 1 < self.rows.len() {
            self.cursor += 1;
            return TableAction::Consumed;
        }
        match self.edge {
            EdgeBehaviour::Escape => TableAction::LeaveDown,
            EdgeBehaviour::Wrap => {
                self.cursor = 0;
                TableAction::Consumed
            }
        }
    }

    fn clamp_cursor(&mut self) {
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    /// Extra styling applied to one row, on top of the enable state. Used to
    /// mark the row currently being edited.
    pub fn render_with(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        focused: bool,
        row_overrides: &dyn Fn(usize) -> Option<Style>,
    ) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        if self.rows.is_empty() {
            self.render_empty(area, buffer);
            return;
        }

        let rows: Vec<Row<'_>> = self
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let mut style = if row.enabled {
                    Style::new()
                } else {
                    theme::disabled()
                };
                if let Some(override_style) = row_overrides(index) {
                    style = style.patch(override_style);
                }
                let mut cells = Vec::with_capacity(3);
                if self.toggles {
                    cells.push(Cell::from(if row.enabled { CHECKED } else { UNCHECKED }));
                }
                cells.push(Cell::from(row.name.as_str()));
                cells.push(Cell::from(row.value.as_str()));
                Row::new(cells).style(style)
            })
            .collect();

        let mut widths = Vec::with_capacity(3);
        if self.toggles {
            widths.push(Constraint::Length(2));
        }
        // The name column is the narrower of the two: values are long and
        // matter more once the name is recognisable.
        widths.push(Constraint::Percentage(35));
        widths.push(Constraint::Fill(1));

        let mut table = Table::new(rows, widths).row_highlight_style(if focused {
            theme::selection()
        } else {
            Style::new()
        });
        if self.show_header {
            let mut header = Vec::with_capacity(3);
            if self.toggles {
                header.push(Cell::from(""));
            }
            header.push(Cell::from(self.columns[0]));
            header.push(Cell::from(self.columns[1]));
            table = table.header(Row::new(header).style(Style::new().fg(theme::MUTED)));
        }

        self.state.select(Some(self.cursor));
        StatefulWidget::render(table, area, buffer, &mut self.state);
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        self.render_with(area, buffer, focused, &|_| None);
    }

    /// Centres the empty-state message vertically and horizontally.
    fn render_empty(&self, area: Rect, buffer: &mut Buffer) {
        if self.empty_message.is_empty() {
            return;
        }
        let lines: Vec<Line<'_>> = self
            .empty_message
            .lines()
            .map(|line| Line::from(Span::styled(line, theme::placeholder())).centered())
            .collect();
        let height = (lines.len() as u16).min(area.height);
        let y = area.y + area.height.saturating_sub(height) / 2;
        Paragraph::new(lines).render(Rect::new(area.x, y, area.width, height), buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn table_with(count: usize) -> KeyValueTable {
        let mut table = KeyValueTable::new(["Header", "Value"]);
        table.set_rows(
            (0..count)
                .map(|index| KeyValue::new(format!("n{index}"), format!("v{index}")))
                .collect(),
        );
        table.set_cursor(0);
        table
    }

    #[test]
    fn vertical_motion_stops_at_the_ends_and_reports_escape() {
        let mut table = table_with(2);
        assert_eq!(table.handle_key(key(KeyCode::Up)), TableAction::LeaveUp);
        assert_eq!(table.cursor(), 0);
        assert_eq!(table.handle_key(key(KeyCode::Down)), TableAction::Consumed);
        assert_eq!(table.cursor(), 1);
        assert_eq!(table.handle_key(key(KeyCode::Down)), TableAction::LeaveDown);
        assert_eq!(table.cursor(), 1);
    }

    #[test]
    fn a_wrapping_table_never_escapes() {
        let mut table = table_with(3);
        table.edge = EdgeBehaviour::Wrap;
        assert_eq!(table.handle_key(key(KeyCode::Up)), TableAction::Consumed);
        assert_eq!(table.cursor(), 2);
        assert_eq!(table.handle_key(key(KeyCode::Down)), TableAction::Consumed);
        assert_eq!(table.cursor(), 0);
    }

    #[test]
    fn vim_keys_mirror_the_arrows() {
        let mut table = table_with(3);
        table.handle_key(key(KeyCode::Char('j')));
        assert_eq!(table.cursor(), 1);
        table.handle_key(key(KeyCode::Char('k')));
        assert_eq!(table.cursor(), 0);
        table.handle_key(key(KeyCode::Char('G')));
        assert_eq!(table.cursor(), 2);
        table.handle_key(key(KeyCode::Char('g')));
        assert_eq!(table.cursor(), 0);
    }

    #[test]
    fn space_toggles_the_selected_row_only() {
        let mut table = table_with(2);
        assert_eq!(
            table.handle_key(key(KeyCode::Char(' '))),
            TableAction::Toggled
        );
        assert!(!table.rows()[0].enabled);
        assert!(table.rows()[1].enabled);
        table.handle_key(key(KeyCode::Char(' ')));
        assert!(table.rows()[0].enabled);
    }

    #[test]
    fn a_table_without_toggles_ignores_space_and_backspace() {
        let mut table = KeyValueTable::read_only(["Header", "Value"]);
        table.set_rows(vec![KeyValue::new("a", "b")]);
        assert_eq!(
            table.handle_key(key(KeyCode::Char(' '))),
            TableAction::Ignored
        );
        assert_eq!(
            table.handle_key(key(KeyCode::Backspace)),
            TableAction::Ignored
        );
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn backspace_removes_and_keeps_the_cursor_in_range() {
        let mut table = table_with(2);
        table.set_cursor(1);
        assert_eq!(
            table.handle_key(key(KeyCode::Backspace)),
            TableAction::Removed
        );
        assert_eq!(table.len(), 1);
        assert_eq!(table.cursor(), 0);
        table.handle_key(key(KeyCode::Backspace));
        assert!(table.is_empty());
        assert_eq!(table.cursor(), 0);
    }

    #[test]
    fn edit_and_copy_keys_are_reported_not_handled() {
        let mut table = table_with(1);
        assert_eq!(table.handle_key(key(KeyCode::Enter)), TableAction::EditKey);
        assert_eq!(
            table.handle_key(key(KeyCode::Char('v'))),
            TableAction::EditValue
        );
        assert_eq!(table.handle_key(key(KeyCode::Char('c'))), TableAction::Copy);
        assert_eq!(table.handle_key(key(KeyCode::Char('y'))), TableAction::Copy);
    }

    #[test]
    fn an_empty_table_reports_nothing_actionable() {
        let mut table = table_with(0);
        for code in [
            KeyCode::Enter,
            KeyCode::Char('v'),
            KeyCode::Char(' '),
            KeyCode::Backspace,
            KeyCode::Char('c'),
        ] {
            assert_eq!(
                table.handle_key(key(code)),
                TableAction::Ignored,
                "{code:?}"
            );
        }
    }

    #[test]
    fn set_rows_clamps_rather_than_resets_the_cursor() {
        let mut table = table_with(5);
        table.set_cursor(4);
        table.set_rows(vec![KeyValue::new("a", "b"), KeyValue::new("c", "d")]);
        assert_eq!(table.cursor(), 1);
        table.set_rows(vec![KeyValue::new("a", "b"), KeyValue::new("c", "d")]);
        assert_eq!(
            table.cursor(),
            1,
            "an equal-length refresh keeps the cursor"
        );
    }

    #[test]
    fn insert_puts_the_cursor_on_the_new_row() {
        let mut table = table_with(2);
        table.insert(1, KeyValue::new("new", "row"));
        assert_eq!(table.cursor(), 1);
        assert_eq!(table.selected().unwrap().name, "new");
    }

    fn render(table: &mut KeyValueTable, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        table.render(area, &mut buffer, true);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn selection_style_is_only_visible_while_focused() {
        let mut table = table_with(1);
        let area = Rect::new(0, 0, 24, 1);

        let mut unfocused = Buffer::empty(area);
        table.render(area, &mut unfocused, false);
        let unfocused_style = unfocused[(4, 0)].style();
        assert!(!unfocused_style.add_modifier.contains(Modifier::REVERSED));
        assert_ne!(unfocused_style.bg, theme::selection().bg);

        let mut focused = Buffer::empty(area);
        table.render(area, &mut focused, true);
        let focused_style = focused[(4, 0)].style();
        assert_eq!(focused_style.bg, theme::selection().bg);
        assert!(
            focused_style
                .add_modifier
                .contains(theme::selection().add_modifier)
        );
    }

    #[test]
    fn renders_the_toggle_column_and_the_two_values() {
        let mut table = table_with(2);
        table.rows[1].enabled = false;
        let lines = render(&mut table, 24, 2);
        assert!(lines[0].contains(CHECKED), "{lines:?}");
        assert!(lines[0].contains("n0"), "{lines:?}");
        assert!(lines[0].contains("v0"), "{lines:?}");
        assert!(!lines[1].contains(CHECKED), "disabled row: {lines:?}");
    }

    #[test]
    fn a_disabled_row_renders_dim() {
        let mut table = table_with(2);
        table.rows[1].enabled = false;
        let area = Rect::new(0, 0, 24, 2);
        let mut buffer = Buffer::empty(area);
        table.render(area, &mut buffer, true);
        assert!(buffer[(4, 1)].style().add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn the_empty_message_is_centred() {
        let mut table = table_with(0);
        table.empty_message = "No headers".into();
        let lines = render(&mut table, 20, 3);
        assert!(lines[1].contains("No headers"), "{lines:?}");
        assert!(lines[1].starts_with("     "), "{lines:?}");
    }

    #[test]
    fn a_zero_sized_area_is_not_a_panic() {
        let mut table = table_with(3);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        table.render(Rect::new(0, 0, 0, 0), &mut buffer, true);
    }
}
