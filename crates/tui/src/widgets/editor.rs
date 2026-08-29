//! A rope-backed multiline text editor.

use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Clear, Widget as _};
use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

use crate::theme;
use crate::widgets::{
    clipboard::Clipboard,
    syntax::{Highlighter, Language},
};

const INDENT: &str = "  ";
const MAX_UNDO: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    Ignored,
    Consumed,
    Changed,
    OpenInPager,
    OpenInEditor,
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Backspace,
    Delete,
}

#[derive(Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
    visual: bool,
}

struct RenderCell {
    text: String,
    width: usize,
    char_start: usize,
    char_end: usize,
    local_byte: usize,
}

struct VisualRow {
    line: usize,
    first: bool,
    cells: Vec<RenderCell>,
    caret: Option<usize>,
}

pub struct Editor {
    rope: Rope,
    cursor: usize,
    anchor: Option<usize>,
    visual: bool,
    read_only: bool,
    language: Option<Language>,
    soft_wrap: bool,
    show_line_numbers: bool,
    scroll_y: usize,
    scroll_x: usize,
    viewport_height: usize,
    preferred_column: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    edit_group: Option<EditKind>,
    error: Option<String>,
    highlighter: Highlighter,
    clipboard: Clipboard,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            rope: Rope::new(),
            cursor: 0,
            anchor: None,
            visual: false,
            read_only: false,
            language: None,
            soft_wrap: true,
            show_line_numbers: false,
            scroll_y: 0,
            scroll_x: 0,
            viewport_height: 1,
            preferred_column: None,
            undo: Vec::new(),
            redo: Vec::new(),
            edit_group: None,
            error: None,
            highlighter: Highlighter::new(),
            clipboard: Clipboard::default(),
        }
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn set_read_only(&mut self, value: bool) {
        self.read_only = value;
        self.end_edit_group();
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn set_text(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
        self.cursor = 0;
        self.anchor = None;
        self.visual = false;
        self.scroll_y = 0;
        self.scroll_x = 0;
        self.preferred_column = None;
        self.undo.clear();
        self.redo.clear();
        self.edit_group = None;
        self.error = None;
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
    }

    pub fn language(&self) -> Option<Language> {
        self.language
    }

    pub fn set_language(&mut self, language: Option<Language>) {
        self.language = language;
    }

    pub fn soft_wrap(&self) -> bool {
        self.soft_wrap
    }

    pub fn set_soft_wrap(&mut self, value: bool) {
        self.soft_wrap = value;
        self.scroll_x = 0;
        self.scroll_y = 0;
    }

    pub fn show_line_numbers(&self) -> bool {
        self.show_line_numbers
    }

    pub fn set_show_line_numbers(&mut self, value: bool) {
        self.show_line_numbers = value;
    }

    /// One-based line and Unicode scalar column.
    pub fn cursor_display(&self) -> (usize, usize) {
        let (line, column) = self.cursor_location();
        (line + 1, column + 1)
    }

    pub fn visual_mode(&self) -> bool {
        self.visual
    }

    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection()?;
        Some(self.rope.slice(range).to_string())
    }

    pub fn copy_target(&self) -> String {
        self.selected_text().unwrap_or_else(|| self.text())
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> EditorAction {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let command_mode = self.read_only || self.visual || self.selection().is_some();
        if key.code == KeyCode::Esc && self.anchor.is_some() {
            self.visual = false;
            self.anchor = None;
            return EditorAction::Consumed;
        }

        if key.code == KeyCode::Char('p') && key.modifiers == KeyModifiers::ALT {
            self.end_edit_group();
            return EditorAction::OpenInPager;
        }
        if key.code == KeyCode::Char('e') && key.modifiers == KeyModifiers::CONTROL {
            self.end_edit_group();
            return EditorAction::OpenInEditor;
        }

        match key.code {
            KeyCode::Left if ctrl => self.move_word_left(shift),
            KeyCode::Right if ctrl => self.move_word_right(shift),
            KeyCode::Left => self.move_left(shift),
            KeyCode::Right => self.move_right(shift),
            KeyCode::Up => return self.move_vertical(-1, shift),
            KeyCode::Down => return self.move_vertical(1, shift),
            KeyCode::Home => self.move_line_start(shift),
            KeyCode::End => self.move_line_end(shift),
            KeyCode::PageUp => self.move_page(-(self.viewport_height as isize), shift),
            KeyCode::PageDown => self.move_page(self.viewport_height as isize, shift),
            KeyCode::Char('r') if ctrl => return self.redo(),
            KeyCode::Char('f') if ctrl => {
                self.move_page(self.viewport_height as isize, shift);
            }
            KeyCode::Char('b') if ctrl => {
                self.move_page(-(self.viewport_height as isize), shift);
            }
            KeyCode::Char('d') if ctrl => {
                self.move_page((self.viewport_height / 2).max(1) as isize, shift);
            }
            KeyCode::Char('u') if ctrl => {
                self.move_page(-((self.viewport_height / 2).max(1) as isize), shift);
            }
            KeyCode::Enter => return self.insert_newline(),
            KeyCode::Backspace => return self.backspace(),
            KeyCode::Delete => return self.delete_forward(),
            KeyCode::Tab if !ctrl && !alt => return self.insert_text(INDENT, EditKind::Insert),
            KeyCode::Char('v') if !ctrl && !alt => {
                self.toggle_visual();
                return EditorAction::Consumed;
            }
            KeyCode::Char('V') if !ctrl && !alt => {
                self.select_line();
                return EditorAction::Consumed;
            }
            KeyCode::Char('y' | 'c') if !ctrl && !alt && command_mode => {
                return self.copy_to_clipboard();
            }
            KeyCode::Char('%') if !ctrl && !alt => {
                self.jump_to_matching_bracket();
                return EditorAction::Consumed;
            }
            KeyCode::Char('u') if !ctrl && !alt => return self.undo(),
            KeyCode::Char('h') if !ctrl && !alt && command_mode => self.move_left(shift),
            KeyCode::Char('l') if !ctrl && !alt && command_mode => self.move_right(shift),
            KeyCode::Char('k') if !ctrl && !alt && command_mode => {
                return self.move_vertical(-1, shift);
            }
            KeyCode::Char('j') if !ctrl && !alt && command_mode => {
                return self.move_vertical(1, shift);
            }
            KeyCode::Char('H') if !ctrl && !alt && command_mode => self.move_left(true),
            KeyCode::Char('L') if !ctrl && !alt && command_mode => self.move_right(true),
            KeyCode::Char('K') if !ctrl && !alt && command_mode => {
                return self.move_vertical(-1, true);
            }
            KeyCode::Char('J') if !ctrl && !alt && command_mode => {
                return self.move_vertical(1, true);
            }
            KeyCode::Char('w') if !ctrl && !alt && command_mode => self.move_word_right(shift),
            KeyCode::Char('b') if !ctrl && !alt && command_mode => self.move_word_left(shift),
            KeyCode::Char('0' | '^') if !ctrl && !alt && command_mode => {
                self.move_line_start(shift);
            }
            KeyCode::Char('$') if !ctrl && !alt && command_mode => self.move_line_end(shift),
            KeyCode::Char('g') if !ctrl && !alt && command_mode => self.move_to(0, shift),
            KeyCode::Char('G') if !ctrl && !alt && command_mode => {
                self.move_to(self.rope.len_chars(), shift);
            }
            KeyCode::Char(c) if !ctrl && !alt => return self.insert_character(c),
            _ => return EditorAction::Ignored,
        }
        EditorAction::Consumed
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        Clear.render(area, buffer);
        self.viewport_height = area.height as usize;

        let line_count = self.rope.len_lines();
        let number_width = if self.show_line_numbers {
            (line_count.max(1).ilog10() as usize + 2).min((area.width as usize).saturating_sub(1))
        } else {
            0
        };
        let content_width = (area.width as usize).saturating_sub(number_width).max(1);
        let syntax = self
            .highlighter
            .highlight(&self.rope.to_string(), self.language);
        let mut rows = self.visual_rows(content_width);
        let cursor_row = rows.iter().position(|row| row.caret.is_some()).unwrap_or(0);

        let viewport_height = area.height as usize;
        let bottom_margin = usize::from(viewport_height > 1);
        if cursor_row < self.scroll_y {
            self.scroll_y = cursor_row;
        } else if cursor_row + bottom_margin >= self.scroll_y + viewport_height {
            self.scroll_y = cursor_row + bottom_margin + 1 - viewport_height;
        }
        let max_scroll = rows
            .len()
            .saturating_add(bottom_margin)
            .saturating_sub(viewport_height);
        self.scroll_y = self.scroll_y.min(max_scroll);

        if !self.soft_wrap {
            let caret = rows.get(cursor_row).and_then(|row| row.caret).unwrap_or(0);
            if caret < self.scroll_x {
                self.scroll_x = caret;
            } else if caret >= self.scroll_x + content_width {
                self.scroll_x = caret + 1 - content_width;
            }
        } else {
            self.scroll_x = 0;
        }

        let selection = self.selection();
        let bracket_pair = self.matching_bracket_pair();
        for (screen_row, row) in rows
            .drain(self.scroll_y..)
            .take(area.height as usize)
            .enumerate()
        {
            let y = area.y + screen_row as u16;
            if self.show_line_numbers && number_width > 0 && row.first {
                let label = format!("{:>width$} ", row.line + 1, width = number_width - 1);
                let style = Style::new().fg(if row.line == self.cursor_location().0 {
                    theme::FOREGROUND
                } else {
                    theme::MUTED
                });
                buffer.set_stringn(area.x, y, label, number_width, style);
            }

            let content_x = area.x + number_width.min(area.width as usize) as u16;
            let cursor_on_cell = row
                .cells
                .iter()
                .any(|cell| self.cursor >= cell.char_start && self.cursor < cell.char_end);
            let mut display_column = 0usize;
            for cell in row.cells {
                let right = display_column + cell.width;
                if right <= self.scroll_x {
                    display_column = right;
                    continue;
                }
                if display_column < self.scroll_x {
                    display_column = right;
                    continue;
                }
                let x = display_column - self.scroll_x;
                if x >= content_width || x + cell.width > content_width {
                    break;
                }

                let mut style = syntax
                    .get(row.line)
                    .and_then(|items| {
                        items
                            .iter()
                            .find(|item| item.range.contains(&cell.local_byte))
                    })
                    .map_or_else(Style::new, |item| item.style);
                if bracket_pair.is_some_and(|(first, second)| {
                    cell.char_start == first || cell.char_start == second
                }) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if focused && self.cursor >= cell.char_start && self.cursor < cell.char_end {
                    style = style.patch(theme::cursor());
                }
                if selection
                    .as_ref()
                    .is_some_and(|range| cell.char_start < range.end && cell.char_end > range.start)
                {
                    style = style.patch(theme::selection());
                }
                buffer.set_stringn(content_x + x as u16, y, &cell.text, cell.width, style);
                display_column = right;
            }

            if focused
                && !cursor_on_cell
                && let Some(caret) = row.caret
                && caret >= self.scroll_x
                && caret - self.scroll_x < content_width
            {
                let x = content_x + (caret - self.scroll_x) as u16;
                buffer.set_stringn(x, y, " ", 1, theme::cursor());
            }
        }
    }

    fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then_some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    fn cursor_location(&self) -> (usize, usize) {
        let line = self.rope.char_to_line(self.cursor);
        (line, self.cursor - self.rope.line_to_char(line))
    }

    fn line_len(&self, line: usize) -> usize {
        let slice = self.rope.line(line);
        let mut len = slice.len_chars();
        if len > 0 && slice.char(len - 1) == '\n' {
            len -= 1;
        }
        if len > 0 && slice.char(len - 1) == '\r' {
            len -= 1;
        }
        len
    }

    fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }

    fn move_to(&mut self, target: usize, extend: bool) {
        self.end_edit_group();
        let target = target.min(self.rope.len_chars());
        let extend = extend || self.visual;
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = target;
        self.preferred_column = None;
    }

    fn move_left(&mut self, extend: bool) {
        let target = self.previous_grapheme(self.cursor);
        self.move_to(target, extend);
    }

    fn move_right(&mut self, extend: bool) {
        let target = self.next_grapheme(self.cursor);
        self.move_to(target, extend);
    }

    fn move_vertical(&mut self, delta: isize, extend: bool) -> EditorAction {
        self.end_edit_group();
        let (line, column) = self.cursor_location();
        let last = self.rope.len_lines().saturating_sub(1);
        if delta < 0 && line == 0 {
            return EditorAction::LeaveUp;
        }
        if delta > 0 && line == last {
            return EditorAction::LeaveDown;
        }
        let target_line = line.saturating_add_signed(delta).min(last);
        let preferred = self.preferred_column.unwrap_or(column);
        let target = self.line_start(target_line) + preferred.min(self.line_len(target_line));
        let should_extend = extend || self.visual;
        if should_extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = target;
        self.preferred_column = Some(preferred);
        EditorAction::Consumed
    }

    fn move_page(&mut self, delta: isize, extend: bool) {
        let (line, column) = self.cursor_location();
        let target_line = line
            .saturating_add_signed(delta)
            .min(self.rope.len_lines().saturating_sub(1));
        let target = self.line_start(target_line) + column.min(self.line_len(target_line));
        self.move_to(target, extend);
    }

    fn move_line_start(&mut self, extend: bool) {
        let (line, _) = self.cursor_location();
        self.move_to(self.line_start(line), extend);
    }

    fn move_line_end(&mut self, extend: bool) {
        let (line, _) = self.cursor_location();
        self.move_to(self.line_start(line) + self.line_len(line), extend);
    }

    fn move_word_left(&mut self, extend: bool) {
        let chars: Vec<char> = self.rope.chars().collect();
        let mut index = self.cursor;
        while index > 0 && !is_word(chars[index - 1]) {
            index -= 1;
        }
        while index > 0 && is_word(chars[index - 1]) {
            index -= 1;
        }
        self.move_to(index, extend);
    }

    fn move_word_right(&mut self, extend: bool) {
        let chars: Vec<char> = self.rope.chars().collect();
        let mut index = self.cursor;
        while index < chars.len() && is_word(chars[index]) {
            index += 1;
        }
        while index < chars.len() && !is_word(chars[index]) {
            index += 1;
        }
        self.move_to(index, extend);
    }

    fn previous_grapheme(&self, cursor: usize) -> usize {
        if cursor == 0 {
            return 0;
        }
        let text = self.rope.to_string();
        let byte = self.rope.char_to_byte(cursor);
        text[..byte]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| text[..index].chars().count())
    }

    fn next_grapheme(&self, cursor: usize) -> usize {
        if cursor >= self.rope.len_chars() {
            return self.rope.len_chars();
        }
        let text = self.rope.to_string();
        let byte = self.rope.char_to_byte(cursor);
        let rest = &text[byte..];
        rest.graphemes(true)
            .next()
            .map_or(cursor, |grapheme| cursor + grapheme.chars().count())
    }

    fn toggle_visual(&mut self) {
        self.end_edit_group();
        self.visual = !self.visual;
        if self.visual {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
    }

    fn select_line(&mut self) {
        self.end_edit_group();
        let (line, _) = self.cursor_location();
        let start = self.line_start(line);
        let end = if line + 1 < self.rope.len_lines() {
            self.line_start(line + 1)
        } else {
            start + self.line_len(line)
        };
        self.anchor = Some(start);
        self.cursor = end;
        self.visual = true;
        self.preferred_column = None;
    }

    fn copy_to_clipboard(&mut self) -> EditorAction {
        self.end_edit_group();
        self.error = None;
        let target = self.copy_target();
        let result = self.clipboard.set_text(target);
        if let Err(error) = result {
            self.error = Some(error.to_string());
        }
        self.visual = false;
        self.anchor = None;
        EditorAction::Consumed
    }

    fn insert_character(&mut self, character: char) -> EditorAction {
        if self.read_only {
            return EditorAction::Consumed;
        }
        if let Some(closer) = closing_for(character) {
            self.begin_edit(EditKind::Insert);
            self.delete_selection_without_checkpoint();
            let pair = format!("{character}{closer}");
            self.rope.insert(self.cursor, &pair);
            self.cursor += 1;
            return EditorAction::Changed;
        }
        if opening_for(character).is_some()
            && self.cursor < self.rope.len_chars()
            && self.rope.char(self.cursor) == character
        {
            self.end_edit_group();
            self.cursor += 1;
            self.anchor = None;
            return EditorAction::Consumed;
        }
        let mut encoded = [0; 4];
        self.insert_text(character.encode_utf8(&mut encoded), EditKind::Insert)
    }

    fn insert_text(&mut self, text: &str, kind: EditKind) -> EditorAction {
        if self.read_only {
            return EditorAction::Consumed;
        }
        self.begin_edit(kind);
        self.delete_selection_without_checkpoint();
        self.rope.insert(self.cursor, text);
        self.cursor += text.chars().count();
        self.anchor = None;
        self.visual = false;
        self.preferred_column = None;
        EditorAction::Changed
    }

    fn insert_newline(&mut self) -> EditorAction {
        if self.read_only {
            return EditorAction::Consumed;
        }
        self.begin_edit(EditKind::Insert);
        self.delete_selection_without_checkpoint();
        let (line, column) = self.cursor_location();
        let line_text = self.line_text(line);
        let indent_len = line_text
            .chars()
            .take_while(|character| character.is_whitespace())
            .count();
        let before = if column == 0 {
            None
        } else {
            self.rope.get_char(self.cursor - 1)
        };
        let after = self.rope.get_char(self.cursor);

        if before.is_some_and(|character| closing_for(character).is_some()) {
            let inner_indent = " ".repeat(indent_len + INDENT.len());
            if before.and_then(closing_for) == after {
                let outer_indent = " ".repeat(indent_len);
                let insertion = format!("\n{inner_indent}\n{outer_indent}");
                self.rope.insert(self.cursor, &insertion);
                self.cursor += 1 + inner_indent.chars().count();
            } else {
                let insertion = format!("\n{inner_indent}");
                self.rope.insert(self.cursor, &insertion);
                self.cursor += insertion.chars().count();
            }
        } else {
            let indent = " ".repeat(indent_len);
            let insertion = format!("\n{indent}");
            self.rope.insert(self.cursor, &insertion);
            self.cursor += insertion.chars().count();
        }
        self.anchor = None;
        self.visual = false;
        self.preferred_column = None;
        EditorAction::Changed
    }

    fn backspace(&mut self) -> EditorAction {
        if self.read_only {
            return EditorAction::Consumed;
        }
        if self.selection().is_some() {
            self.begin_edit(EditKind::Backspace);
            self.delete_selection_without_checkpoint();
            return EditorAction::Changed;
        }
        if self.cursor == 0 {
            self.end_edit_group();
            return EditorAction::Consumed;
        }
        self.begin_edit(EditKind::Backspace);
        let previous = self.previous_grapheme(self.cursor);
        self.rope.remove(previous..self.cursor);
        self.cursor = previous;
        self.preferred_column = None;
        EditorAction::Changed
    }

    fn delete_forward(&mut self) -> EditorAction {
        if self.read_only {
            return EditorAction::Consumed;
        }
        if self.selection().is_some() {
            self.begin_edit(EditKind::Delete);
            self.delete_selection_without_checkpoint();
            return EditorAction::Changed;
        }
        if self.cursor >= self.rope.len_chars() {
            self.end_edit_group();
            return EditorAction::Consumed;
        }
        self.begin_edit(EditKind::Delete);
        let next = self.next_grapheme(self.cursor);
        self.rope.remove(self.cursor..next);
        self.preferred_column = None;
        EditorAction::Changed
    }

    fn delete_selection_without_checkpoint(&mut self) -> bool {
        let Some(range) = self.selection() else {
            return false;
        };
        self.cursor = range.start;
        self.rope.remove(range);
        self.anchor = None;
        self.visual = false;
        true
    }

    fn begin_edit(&mut self, kind: EditKind) {
        if self.edit_group != Some(kind) {
            let snapshot = self.snapshot();
            self.undo.push(snapshot);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.edit_group = Some(kind);
        self.redo.clear();
    }

    fn end_edit_group(&mut self) {
        self.edit_group = None;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.rope.to_string(),
            cursor: self.cursor,
            anchor: self.anchor,
            visual: self.visual,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.rope = Rope::from_str(&snapshot.text);
        self.cursor = snapshot.cursor.min(self.rope.len_chars());
        self.anchor = snapshot
            .anchor
            .map(|anchor| anchor.min(self.rope.len_chars()));
        self.visual = snapshot.visual;
        self.preferred_column = None;
        self.scroll_x = 0;
        self.scroll_y = 0;
    }

    fn undo(&mut self) -> EditorAction {
        if self.read_only {
            return EditorAction::Consumed;
        }
        self.end_edit_group();
        let Some(snapshot) = self.undo.pop() else {
            return EditorAction::Consumed;
        };
        let current = self.snapshot();
        self.redo.push(current);
        self.restore(snapshot);
        EditorAction::Changed
    }

    fn redo(&mut self) -> EditorAction {
        if self.read_only {
            return EditorAction::Consumed;
        }
        self.end_edit_group();
        let Some(snapshot) = self.redo.pop() else {
            return EditorAction::Consumed;
        };
        let current = self.snapshot();
        self.undo.push(current);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.restore(snapshot);
        EditorAction::Changed
    }

    fn jump_to_matching_bracket(&mut self) {
        self.end_edit_group();
        let pair = if self.cursor < self.rope.len_chars() && is_bracket(self.rope.char(self.cursor))
        {
            self.match_from(self.cursor)
                .map(|matched| (self.cursor, matched))
        } else {
            let (line, _) = self.cursor_location();
            let end = self.line_start(line) + self.line_len(line);
            (self.cursor..end).find_map(|index| {
                is_bracket(self.rope.char(index))
                    .then(|| self.match_from(index).map(|matched| (index, matched)))
                    .flatten()
            })
        };
        if let Some((_, matched)) = pair {
            let extend = self.visual;
            self.move_to(matched, extend);
        }
    }

    fn matching_bracket_pair(&self) -> Option<(usize, usize)> {
        if self.cursor >= self.rope.len_chars() || !is_bracket(self.rope.char(self.cursor)) {
            return None;
        }
        self.match_from(self.cursor)
            .map(|matched| (self.cursor, matched))
    }

    fn match_from(&self, start: usize) -> Option<usize> {
        let bracket = self.rope.char(start);
        if let Some(close) = closing_for(bracket) {
            let mut depth = 0usize;
            for index in start + 1..self.rope.len_chars() {
                let character = self.rope.char(index);
                if character == bracket {
                    depth += 1;
                } else if character == close {
                    if depth == 0 {
                        return Some(index);
                    }
                    depth -= 1;
                }
            }
        } else if let Some(open) = opening_for(bracket) {
            let mut depth = 0usize;
            for index in (0..start).rev() {
                let character = self.rope.char(index);
                if character == bracket {
                    depth += 1;
                } else if character == open {
                    if depth == 0 {
                        return Some(index);
                    }
                    depth -= 1;
                }
            }
        }
        None
    }

    fn line_text(&self, line: usize) -> String {
        let mut text = self.rope.line(line).to_string();
        if text.ends_with('\n') {
            text.pop();
        }
        if text.ends_with('\r') {
            text.pop();
        }
        text
    }

    fn visual_rows(&self, width: usize) -> Vec<VisualRow> {
        let mut rows = Vec::new();
        for line in 0..self.rope.len_lines() {
            let text = self.line_text(line);
            let line_start = self.line_start(line);
            let line_end = line_start + text.chars().count();
            let mut cells = Vec::new();
            let mut char_index = line_start;
            for (local_byte, grapheme) in text.grapheme_indices(true) {
                let char_end = char_index + grapheme.chars().count();
                let (display, cell_width) = if grapheme == "\t" {
                    (INDENT.to_owned(), INDENT.len())
                } else {
                    (grapheme.to_owned(), grapheme.width().max(1))
                };
                cells.push(RenderCell {
                    text: display,
                    width: cell_width,
                    char_start: char_index,
                    char_end,
                    local_byte,
                });
                char_index = char_end;
            }

            if !self.soft_wrap {
                let caret = (self.cursor >= line_start && self.cursor <= line_end)
                    .then(|| width_before_cursor(&cells, self.cursor));
                rows.push(VisualRow {
                    line,
                    first: true,
                    cells,
                    caret,
                });
                continue;
            }

            let mut segment = Vec::new();
            let mut segment_width = 0usize;
            let mut segment_start = line_start;
            let mut first = true;
            for cell in cells {
                if !segment.is_empty() && segment_width + cell.width > width {
                    let caret = (self.cursor >= segment_start && self.cursor < cell.char_start)
                        .then(|| width_before_cursor(&segment, self.cursor));
                    rows.push(VisualRow {
                        line,
                        first,
                        cells: segment,
                        caret,
                    });
                    segment = Vec::new();
                    segment_width = 0;
                    segment_start = cell.char_start;
                    first = false;
                }
                segment_width += cell.width;
                segment.push(cell);
            }
            let mut caret = (self.cursor >= segment_start && self.cursor <= line_end)
                .then(|| width_before_cursor(&segment, self.cursor));
            if caret == Some(width) && !segment.is_empty() {
                caret = None;
                rows.push(VisualRow {
                    line,
                    first,
                    cells: segment,
                    caret,
                });
                rows.push(VisualRow {
                    line,
                    first: false,
                    cells: Vec::new(),
                    caret: Some(0),
                });
            } else {
                rows.push(VisualRow {
                    line,
                    first,
                    cells: segment,
                    caret,
                });
            }
        }
        rows
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

fn width_before_cursor(cells: &[RenderCell], cursor: usize) -> usize {
    cells
        .iter()
        .take_while(|cell| cell.char_start < cursor)
        .map(|cell| cell.width)
        .sum()
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn closing_for(character: char) -> Option<char> {
    match character {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

fn opening_for(character: char) -> Option<char> {
    match character {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        _ => None,
    }
}

fn is_bracket(character: char) -> bool {
    closing_for(character).is_some() || opening_for(character).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn modified(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn set_text_resets_the_cursor_and_public_options_round_trip() {
        let mut editor = Editor::new();
        editor.set_text("one\ntwo");
        editor.set_language(Some(Language::Json));
        editor.set_soft_wrap(false);
        editor.set_show_line_numbers(true);
        editor.set_read_only(true);
        assert_eq!(editor.cursor_display(), (1, 1));
        assert_eq!(editor.text(), "one\ntwo");
        assert_eq!(editor.language(), Some(Language::Json));
        assert!(!editor.soft_wrap());
        assert!(editor.show_line_numbers());
        assert!(editor.read_only());
    }

    #[test]
    fn unicode_graphemes_move_and_delete_as_units() {
        let mut editor = Editor::new();
        editor.set_text("a\u{301}界z");
        editor.handle_key(key(KeyCode::Right));
        assert_eq!(editor.cursor_display(), (1, 3));
        editor.handle_key(key(KeyCode::Right));
        assert_eq!(editor.cursor_display(), (1, 4));
        assert_eq!(
            editor.handle_key(key(KeyCode::Backspace)),
            EditorAction::Changed
        );
        assert_eq!(editor.text(), "a\u{301}z");
    }

    #[test]
    fn vertical_movement_keeps_the_preferred_unicode_column_and_leaves_at_edges() {
        let mut editor = Editor::new();
        editor.set_text("abcd\nx\nwxyz");
        editor.set_read_only(true);
        editor.handle_key(key(KeyCode::End));
        editor.handle_key(key(KeyCode::Down));
        assert_eq!(editor.cursor_display(), (2, 2));
        editor.handle_key(key(KeyCode::Down));
        assert_eq!(editor.cursor_display(), (3, 5));
        assert_eq!(
            editor.handle_key(key(KeyCode::Down)),
            EditorAction::LeaveDown
        );
        editor.handle_key(key(KeyCode::Home));
        editor.handle_key(key(KeyCode::Char('g')));
        assert_eq!(editor.handle_key(key(KeyCode::Up)), EditorAction::LeaveUp);
    }

    #[test]
    fn words_document_edges_and_page_movements_work() {
        let mut editor = Editor::new();
        editor.set_text("alpha beta\nsecond\nthird\nfourth");
        editor.set_read_only(true);
        editor.handle_key(key(KeyCode::Char('w')));
        assert_eq!(editor.cursor_display(), (1, 7));
        editor.handle_key(key(KeyCode::Char('b')));
        assert_eq!(editor.cursor_display(), (1, 1));
        editor.viewport_height = 2;
        editor.handle_key(modified(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert_eq!(editor.cursor_display(), (3, 1));
        editor.handle_key(modified(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(editor.cursor_display(), (2, 1));
        editor.handle_key(key(KeyCode::Char('G')));
        assert_eq!(editor.cursor_display(), (4, 7));
    }

    #[test]
    fn escape_clears_visual_and_shift_selections_but_is_otherwise_ignored() {
        let mut editor = Editor::new();
        editor.set_text("abcd");

        editor.handle_key(key(KeyCode::Char('v')));
        editor.handle_key(key(KeyCode::Right));
        assert!(editor.visual_mode());
        assert_eq!(editor.selected_text().as_deref(), Some("a"));
        assert_eq!(editor.handle_key(key(KeyCode::Esc)), EditorAction::Consumed);
        assert!(!editor.visual_mode());
        assert_eq!(editor.selected_text(), None);
        assert_eq!(editor.text(), "abcd");
        assert_eq!(editor.cursor_display(), (1, 2));
        assert_eq!(editor.handle_key(key(KeyCode::Esc)), EditorAction::Ignored);

        editor.handle_key(modified(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(editor.selected_text().as_deref(), Some("b"));
        assert_eq!(editor.handle_key(key(KeyCode::Esc)), EditorAction::Consumed);
        assert_eq!(editor.selected_text(), None);
        assert_eq!(editor.cursor_display(), (1, 3));
    }

    #[test]
    fn line_selection_includes_the_line_break_and_copy_falls_back_to_all_text() {
        let mut editor = Editor::new();
        editor.set_text("first\nsecond");
        assert_eq!(editor.copy_target(), "first\nsecond");
        editor.handle_key(key(KeyCode::Char('V')));
        assert_eq!(editor.selected_text().as_deref(), Some("first\n"));
        assert_eq!(editor.copy_target(), "first\n");
    }

    #[test]
    fn bracket_jump_uses_cursor_bracket_or_next_bracket_on_the_line() {
        let mut editor = Editor::new();
        editor.set_text("x = ([{}])");
        editor.handle_key(key(KeyCode::Char('%')));
        assert_eq!(editor.cursor_display(), (1, 10));
        editor.handle_key(key(KeyCode::Char('%')));
        assert_eq!(editor.cursor_display(), (1, 5));
    }

    #[test]
    fn pairs_brackets_and_steps_over_an_existing_closer() {
        let mut editor = Editor::new();
        assert_eq!(
            editor.handle_key(key(KeyCode::Char('{'))),
            EditorAction::Changed
        );
        assert_eq!(editor.text(), "{}");
        assert_eq!(editor.cursor_display(), (1, 2));
        assert_eq!(
            editor.handle_key(key(KeyCode::Char('}'))),
            EditorAction::Consumed
        );
        assert_eq!(editor.text(), "{}");
        assert_eq!(editor.cursor_display(), (1, 3));
    }

    #[test]
    fn enter_between_a_pair_indents_inside_and_keeps_closer_outside() {
        let mut editor = Editor::new();
        editor.handle_key(key(KeyCode::Char('{')));
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(editor.text(), "{\n  \n}");
        assert_eq!(editor.cursor_display(), (2, 3));
        editor.handle_key(key(KeyCode::Char('x')));
        editor.handle_key(key(KeyCode::Enter));
        assert_eq!(editor.text(), "{\n  x\n  \n}");
    }

    #[test]
    fn tab_inserts_two_spaces_and_selection_is_replaced_by_input() {
        let mut editor = Editor::new();
        editor.handle_key(key(KeyCode::Tab));
        assert_eq!(editor.text(), "  ");
        editor.set_text("old");
        editor.handle_key(modified(KeyCode::Right, KeyModifiers::SHIFT));
        editor.handle_key(key(KeyCode::Char('n')));
        assert_eq!(editor.text(), "nld");
    }

    #[test]
    fn continuous_edits_share_a_checkpoint_and_redo_restores_them() {
        let mut editor = Editor::new();
        for character in ['a', 'b', 'c'] {
            editor.handle_key(key(KeyCode::Char(character)));
        }
        assert_eq!(editor.text(), "abc");
        assert_eq!(
            editor.handle_key(key(KeyCode::Char('u'))),
            EditorAction::Changed
        );
        assert_eq!(editor.text(), "");
        assert_eq!(
            editor.handle_key(modified(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            EditorAction::Changed
        );
        assert_eq!(editor.text(), "abc");
    }

    #[test]
    fn read_only_consumes_mutations_without_changing_text() {
        let mut editor = Editor::new();
        editor.set_text("fixed");
        editor.set_read_only(true);
        for code in [
            KeyCode::Char('x'),
            KeyCode::Enter,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Tab,
        ] {
            assert_ne!(editor.handle_key(key(code)), EditorAction::Changed);
        }
        assert_eq!(editor.text(), "fixed");
    }

    #[test]
    fn external_program_actions_use_the_configured_defaults_without_function_key_aliases() {
        let mut editor = Editor::new();
        assert_eq!(
            editor.handle_key(modified(KeyCode::Char('p'), KeyModifiers::ALT)),
            EditorAction::OpenInPager
        );
        assert_eq!(
            editor.handle_key(modified(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            EditorAction::OpenInEditor
        );
        for number in [3, 4] {
            assert_eq!(
                editor.handle_key(key(KeyCode::F(number))),
                EditorAction::Ignored
            );
        }
    }

    #[test]
    fn render_draws_line_numbers_syntax_selection_cursor_and_matching_brackets() {
        let mut editor = Editor::new();
        editor.set_text("{\"x\": 1}");
        editor.set_language(Some(Language::Json));
        editor.set_show_line_numbers(true);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 20, 2));
        editor.render(Rect::new(0, 0, 20, 2), &mut buffer, true);
        assert_eq!(buffer[(0, 0)].symbol(), "1");
        assert_eq!(buffer[(2, 0)].style().bg, theme::cursor().bg);
        assert!(
            buffer[(2, 0)]
                .style()
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        assert_eq!(
            buffer[(3, 0)].style().fg,
            theme::syntax::for_capture("property").fg
        );

        editor.handle_key(modified(KeyCode::Right, KeyModifiers::SHIFT));
        editor.render(Rect::new(0, 0, 20, 2), &mut buffer, false);
        assert_eq!(buffer[(2, 0)].style().bg, theme::selection().bg);
    }

    #[test]
    fn soft_wrap_and_horizontal_scroll_keep_the_cursor_visible() {
        let mut editor = Editor::new();
        editor.set_text("123456789");
        editor.handle_key(key(KeyCode::End));
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 3));
        editor.render(Rect::new(0, 0, 4, 3), &mut buffer, true);
        assert!(editor.scroll_y > 0);

        editor.set_soft_wrap(false);
        editor.render(Rect::new(0, 0, 4, 1), &mut buffer, true);
        assert!(editor.scroll_x > 0);
        assert_eq!(buffer[(3, 0)].style().bg, theme::cursor().bg);
    }

    #[test]
    fn clipboard_errors_can_be_taken_without_reporting_a_change() {
        let mut editor = Editor::new();
        editor.set_text("copy me");
        editor.set_read_only(true);
        let action = editor.handle_key(key(KeyCode::Char('y')));
        assert_eq!(action, EditorAction::Consumed);
        let _ = editor.take_error();
        assert_eq!(editor.take_error(), None);
    }
}
