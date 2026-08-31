//! A single-line text input.
//!
//! ratatui has no input widget, so this owns the whole job: the text, the
//! caret, the selection, horizontal scrolling, and the styled render. Every
//! editable single-line field in the app is one of these.
//!
//! Offsets are byte offsets into `value` and always sit on a `char` boundary.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget as _;
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

use crate::theme;
use crate::widgets::highlight::{self, Highlight};

/// Stand-in glyph for a masked value.
const MASK: char = '\u{2022}';

/// What the caller must do after a key was handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    /// Not an input key; the caller should try its own bindings.
    Ignored,
    /// Handled, nothing else to do.
    Consumed,
    /// Handled and the value changed.
    Changed,
    /// `Enter`.
    Submitted,
    /// The caret tried to leave the field. The caller decides where focus goes.
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, Default)]
pub struct Input {
    value: String,
    /// Byte offset of the caret, in `0..=value.len()`.
    cursor: usize,
    /// Byte offset the selection was started from, if a selection is active.
    anchor: Option<usize>,
    /// Byte offset of the leftmost rendered grapheme.
    scroll: usize,
    pub placeholder: String,
    /// Render every grapheme as `•`.
    pub password: bool,
    pub read_only: bool,
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_placeholder(placeholder: impl Into<String>) -> Self {
        Self {
            placeholder: placeholder.into(),
            ..Self::default()
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Replaces the value and puts the caret at the end, which is what every
    /// programmatic load wants.
    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
        self.anchor = None;
        self.scroll = 0;
    }

    pub fn clear(&mut self) {
        self.set_value(String::new());
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Moves the caret, clamping to a char boundary inside the value.
    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = self.clamp_boundary(cursor);
        self.scroll = self.scroll.min(self.cursor);
        self.anchor = None;
    }

    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.value.len();
    }

    /// Replaces the byte range with `text` and leaves the caret after it. Used
    /// by autocompletion to splice a candidate over a partial variable name.
    pub fn splice(&mut self, range: std::ops::Range<usize>, text: &str) {
        let start = self.clamp_boundary(range.start);
        let end = self.clamp_boundary(range.end).max(start);
        self.value.replace_range(start..end, text);
        self.cursor = start + text.len();
        self.scroll = self.scroll.min(self.cursor);
        self.anchor = None;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> InputAction {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        match key.code {
            KeyCode::Char(c) if !ctrl && !alt => {
                if self.read_only {
                    return InputAction::Consumed;
                }
                self.delete_selection();
                self.value.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                InputAction::Changed
            }
            KeyCode::Backspace if ctrl => self.delete_word_before(),
            KeyCode::Backspace => {
                if self.read_only {
                    return InputAction::Consumed;
                }
                if self.delete_selection() {
                    return InputAction::Changed;
                }
                if self.cursor == 0 {
                    return InputAction::Consumed;
                }
                let previous = self.previous_boundary(self.cursor);
                self.value.replace_range(previous..self.cursor, "");
                self.cursor = previous;
                self.scroll = self.scroll.min(self.cursor);
                InputAction::Changed
            }
            KeyCode::Delete => {
                if self.read_only {
                    return InputAction::Consumed;
                }
                if self.delete_selection() {
                    return InputAction::Changed;
                }
                if self.cursor >= self.value.len() {
                    return InputAction::Consumed;
                }
                let next = self.next_boundary(self.cursor);
                self.value.replace_range(self.cursor..next, "");
                InputAction::Changed
            }
            KeyCode::Left if ctrl => {
                self.move_to(self.word_start(), shift);
                InputAction::Consumed
            }
            KeyCode::Right if ctrl => {
                self.move_to(self.word_end(), shift);
                InputAction::Consumed
            }
            KeyCode::Left => {
                self.move_to(self.previous_boundary(self.cursor), shift);
                InputAction::Consumed
            }
            KeyCode::Right => {
                self.move_to(self.next_boundary(self.cursor), shift);
                InputAction::Consumed
            }
            KeyCode::Home => {
                self.move_to(0, shift);
                InputAction::Consumed
            }
            KeyCode::End => {
                self.move_to(self.value.len(), shift);
                InputAction::Consumed
            }
            KeyCode::Char('a') if ctrl => {
                self.move_to(0, shift);
                InputAction::Consumed
            }
            KeyCode::Char('e') if ctrl => {
                // `ctrl+e` is the app-wide "open in editor" binding, so it is
                // deliberately not bound to end-of-line here.
                InputAction::Ignored
            }
            KeyCode::Char('u') if ctrl => {
                if self.read_only || self.cursor == 0 {
                    return InputAction::Consumed;
                }
                self.value.replace_range(..self.cursor, "");
                self.cursor = 0;
                self.scroll = 0;
                self.anchor = None;
                InputAction::Changed
            }
            KeyCode::Char('k') if ctrl => {
                if self.read_only || self.cursor >= self.value.len() {
                    return InputAction::Consumed;
                }
                self.value.truncate(self.cursor);
                self.anchor = None;
                InputAction::Changed
            }
            KeyCode::Enter => InputAction::Submitted,
            KeyCode::Up => InputAction::LeaveUp,
            KeyCode::Down => InputAction::LeaveDown,
            _ => InputAction::Ignored,
        }
    }

    /// Renders into `area`, which must be exactly one row high.
    ///
    /// `highlights` may overlap; they are flattened here. Pass an empty slice
    /// for a plain field.
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        focused: bool,
        highlights: &[Highlight],
    ) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let width = area.width as usize;

        if self.value.is_empty() {
            if !self.placeholder.is_empty() {
                Line::from(Span::styled(
                    truncate_to_width(&self.placeholder, width),
                    theme::placeholder(),
                ))
                .render(area, buffer);
            }
            if focused {
                self.render_caret_cell(area, buffer, 0);
            }
            return;
        }

        self.reflow(width, focused);
        let display = self.display_text();
        let spans = self.build_spans(&display, highlights, focused, width);
        Line::from(spans).render(area, buffer);
    }

    /// The caret's column within `area`, for terminals where a real cursor is
    /// preferred over a painted block.
    pub fn caret_column(&self, width: usize) -> u16 {
        let visible = &self.value[self.scroll..self.cursor.max(self.scroll)];
        (visible.width().min(width.saturating_sub(1))) as u16
    }

    fn render_caret_cell(&self, area: Rect, buffer: &mut Buffer, column: u16) {
        if column >= area.width {
            return;
        }
        buffer[(area.x + column, area.y)].set_style(theme::cursor());
    }

    /// Keeps the caret inside the visible window.
    ///
    /// A focused input reserves one cell for the caret, which may sit one past
    /// the last character; an unfocused one does not, so a value that exactly
    /// fills the field stays fully visible.
    fn reflow(&mut self, width: usize, focused: bool) {
        if width == 0 {
            return;
        }
        if self.scroll > self.cursor {
            self.scroll = self.cursor;
        }
        let budget = if focused {
            width.saturating_sub(1)
        } else {
            width
        };
        while self.width_between(self.scroll, self.cursor) > budget && self.scroll < self.cursor {
            self.scroll = self.next_boundary(self.scroll);
        }
        // Pull the window back as the caret moves left or text is deleted.
        // Without this, `scroll` only ever advances (unless the caret crosses
        // it), leaving stale empty space while the hidden prefix stays hidden.
        while self.scroll > 0 {
            let previous = self.previous_boundary(self.scroll);
            if self.width_between(previous, self.cursor) > budget {
                break;
            }
            self.scroll = previous;
        }
        self.scroll = self.clamp_boundary(self.scroll);
    }

    /// Rendered width of a byte range of `value`. In password mode every
    /// grapheme renders as one single-width mask character.
    fn width_between(&self, from: usize, to: usize) -> usize {
        let slice = &self.value[from.min(to)..to];
        if self.password {
            slice.graphemes(true).count()
        } else {
            slice.width()
        }
    }

    fn display_text(&self) -> String {
        if self.password {
            MASK.to_string().repeat(self.value.graphemes(true).count())
        } else {
            self.value.clone()
        }
    }

    /// Maps a byte offset in `value` to the equivalent offset in the rendered
    /// text. They differ in password mode, where each grapheme becomes one
    /// three-byte mask character.
    fn to_display_offset(&self, offset: usize) -> usize {
        if !self.password {
            return offset;
        }
        let offset = self.clamp_boundary(offset);
        self.value[..offset].graphemes(true).count() * MASK.len_utf8()
    }

    fn build_spans<'a>(
        &self,
        display: &'a str,
        highlights: &[Highlight],
        focused: bool,
        width: usize,
    ) -> Vec<Span<'a>> {
        // A masked value has no meaningful spans, and leaking span boundaries
        // would leak the value's structure.
        let flat = if self.password {
            Vec::new()
        } else {
            highlight::flatten(display, highlights)
        };
        let selection = self
            .selection()
            .map(|range| self.to_display_offset(range.start)..self.to_display_offset(range.end));
        let caret = self.to_display_offset(self.cursor);

        let mut spans: Vec<Span<'a>> = Vec::new();
        let mut used = 0usize;
        let mut index = self.to_display_offset(self.scroll).min(display.len());
        while index < display.len() && used < width {
            let style = style_at(&flat, index);
            let mut end = index;
            while end < display.len() && style_at(&flat, end) == style {
                end = next_boundary_in(display, end);
            }
            // Break the run wherever selection or caret styling changes.
            let mut stop = end;
            for boundary in [
                selection.as_ref().map(|s| s.start),
                selection.as_ref().map(|s| s.end),
                focused.then_some(caret),
                focused.then(|| next_boundary_in(display, caret)),
            ]
            .into_iter()
            .flatten()
            {
                if boundary > index && boundary < stop {
                    stop = boundary;
                }
            }
            let text = &display[index..stop];
            let truncated = truncate_to_width(text, width - used);
            used += truncated.width();
            let mut style = style;
            if selection
                .as_ref()
                .is_some_and(|s| index >= s.start && index < s.end)
            {
                style = style.patch(theme::selection());
            }
            if focused && index == caret {
                style = style.patch(theme::cursor());
            }
            spans.push(Span::styled(truncated.to_owned(), style));
            index = stop;
        }
        // The caret past the last character needs its own painted cell.
        if focused && caret >= display.len() && used < width {
            spans.push(Span::styled(" ", theme::cursor()));
        }
        spans
    }

    fn move_to(&mut self, target: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = self.clamp_boundary(target);
        self.scroll = self.scroll.min(self.cursor);
    }

    fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            return false;
        };
        if self.read_only {
            return false;
        }
        self.value.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.scroll = self.scroll.min(self.cursor);
        self.anchor = None;
        true
    }

    fn delete_word_before(&mut self) -> InputAction {
        if self.read_only {
            return InputAction::Consumed;
        }
        if self.delete_selection() {
            return InputAction::Changed;
        }
        let start = self.word_start();
        if start == self.cursor {
            return InputAction::Consumed;
        }
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.scroll = self.scroll.min(self.cursor);
        InputAction::Changed
    }

    /// Start of the word before the caret. Whitespace immediately before the
    /// caret is skipped first, so repeated presses keep making progress.
    fn word_start(&self) -> usize {
        let mut index = self.cursor;
        while index > 0 && is_space_before(&self.value, index) {
            index = self.previous_boundary(index);
        }
        while index > 0 && !is_space_before(&self.value, index) {
            index = self.previous_boundary(index);
        }
        index
    }

    fn word_end(&self) -> usize {
        let mut index = self.cursor;
        while index < self.value.len() && is_space_at(&self.value, index) {
            index = self.next_boundary(index);
        }
        while index < self.value.len() && !is_space_at(&self.value, index) {
            index = self.next_boundary(index);
        }
        index
    }

    fn clamp_boundary(&self, index: usize) -> usize {
        let mut index = index.min(self.value.len());
        while index > 0 && !self.value.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn previous_boundary(&self, index: usize) -> usize {
        if index == 0 {
            return 0;
        }
        let mut index = index - 1;
        while index > 0 && !self.value.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    fn next_boundary(&self, index: usize) -> usize {
        next_boundary_in(&self.value, index)
    }
}

fn next_boundary_in(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut index = index + 1;
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn style_at(flat: &[Highlight], index: usize) -> Style {
    flat.iter()
        .find(|h| h.range.start <= index && index < h.range.end)
        .map(|h| h.style)
        .unwrap_or_default()
}

fn is_space_at(text: &str, index: usize) -> bool {
    text[index..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
}

fn is_space_before(text: &str, index: usize) -> bool {
    text[..index]
        .chars()
        .next_back()
        .is_some_and(char::is_whitespace)
}

/// Truncates on a grapheme boundary so a wide character is never cut in half.
fn truncate_to_width(text: &str, width: usize) -> &str {
    if text.width() <= width {
        return text;
    }
    let mut used = 0usize;
    for (offset, grapheme) in text.grapheme_indices(true) {
        let next = used + grapheme.width();
        if next > width {
            return &text[..offset];
        }
        used = next;
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn typed(text: &str) -> Input {
        let mut input = Input::new();
        for c in text.chars() {
            input.handle_key(key(KeyCode::Char(c)));
        }
        input
    }

    #[test]
    fn typing_inserts_at_the_caret() {
        let mut input = typed("abc");
        assert_eq!(input.value(), "abc");
        input.set_cursor(1);
        assert_eq!(
            input.handle_key(key(KeyCode::Char('X'))),
            InputAction::Changed
        );
        assert_eq!(input.value(), "aXbc");
        assert_eq!(input.cursor(), 2);
    }

    #[test]
    fn backspace_and_delete_respect_the_caret() {
        let mut input = typed("abc");
        input.set_cursor(2);
        input.handle_key(key(KeyCode::Backspace));
        assert_eq!(input.value(), "ac");
        input.handle_key(key(KeyCode::Delete));
        assert_eq!(input.value(), "a");
        // Both are no-ops at the boundaries.
        input.set_cursor(0);
        assert_eq!(
            input.handle_key(key(KeyCode::Backspace)),
            InputAction::Consumed
        );
        input.set_cursor(1);
        assert_eq!(
            input.handle_key(key(KeyCode::Delete)),
            InputAction::Consumed
        );
        assert_eq!(input.value(), "a");
    }

    #[test]
    fn multibyte_editing_never_splits_a_character() {
        let mut input = typed("日本語");
        assert_eq!(input.cursor(), 9);
        input.handle_key(key(KeyCode::Left));
        assert_eq!(input.cursor(), 6);
        input.handle_key(key(KeyCode::Backspace));
        assert_eq!(input.value(), "日語");
    }

    #[test]
    fn shift_arrows_build_a_selection_that_typing_replaces() {
        let mut input = typed("hello");
        input.set_cursor(0);
        input.handle_key(with(KeyCode::Right, KeyModifiers::SHIFT));
        input.handle_key(with(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(input.selection(), Some(0..2));
        input.handle_key(key(KeyCode::Char('X')));
        assert_eq!(input.value(), "Xllo");
        assert_eq!(input.selection(), None);
    }

    #[test]
    fn backspace_clears_a_full_selection() {
        let mut input = typed("existing value");
        input.select_all();

        assert_eq!(
            input.handle_key(key(KeyCode::Backspace)),
            InputAction::Changed
        );
        assert!(input.is_empty());
        assert_eq!(input.cursor(), 0);
        assert_eq!(input.selection(), None);
    }

    #[test]
    fn word_motions_skip_whitespace_then_the_word() {
        let mut input = typed("alpha beta gamma");
        input.handle_key(with(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(&input.value()[input.cursor()..], "gamma");
        input.handle_key(with(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(&input.value()[input.cursor()..], "beta gamma");
        input.handle_key(with(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(&input.value()[..input.cursor()], "alpha beta");
    }

    #[test]
    fn ctrl_backspace_deletes_a_word() {
        let mut input = typed("alpha beta");
        assert_eq!(
            input.handle_key(with(KeyCode::Backspace, KeyModifiers::CONTROL)),
            InputAction::Changed
        );
        assert_eq!(input.value(), "alpha ");
    }

    #[test]
    fn ctrl_u_and_ctrl_k_cut_to_the_line_ends() {
        let mut input = typed("alpha beta");
        input.set_cursor(6);
        input.handle_key(with(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(input.value(), "beta");
        input.set_cursor(2);
        input.handle_key(with(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(input.value(), "be");
    }

    #[test]
    fn ctrl_e_is_left_for_the_app_wide_editor_binding() {
        let mut input = typed("x");
        assert_eq!(
            input.handle_key(with(KeyCode::Char('e'), KeyModifiers::CONTROL)),
            InputAction::Ignored
        );
    }

    #[test]
    fn vertical_arrows_ask_the_caller_to_move_focus() {
        let mut input = typed("x");
        assert_eq!(input.handle_key(key(KeyCode::Up)), InputAction::LeaveUp);
        assert_eq!(input.handle_key(key(KeyCode::Down)), InputAction::LeaveDown);
        assert_eq!(
            input.handle_key(key(KeyCode::Enter)),
            InputAction::Submitted
        );
    }

    #[test]
    fn read_only_rejects_every_mutation() {
        let mut input = typed("abc");
        input.read_only = true;
        input.set_cursor(1);
        for event in [
            key(KeyCode::Char('X')),
            key(KeyCode::Backspace),
            key(KeyCode::Delete),
            with(KeyCode::Char('u'), KeyModifiers::CONTROL),
            with(KeyCode::Char('k'), KeyModifiers::CONTROL),
        ] {
            assert_eq!(input.handle_key(event), InputAction::Consumed, "{event:?}");
        }
        assert_eq!(input.value(), "abc");
    }

    #[test]
    fn splice_replaces_a_range_and_lands_the_caret_after_it() {
        let mut input = typed("url/$PO/x");
        input.splice(4..7, "$POST_ID");
        assert_eq!(input.value(), "url/$POST_ID/x");
        assert_eq!(input.cursor(), 12);
    }

    fn render_to_string(input: &mut Input, width: u16, focused: bool) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buffer = Buffer::empty(area);
        input.render(area, &mut buffer, focused, &[]);
        (0..width)
            .map(|x| buffer[(x, 0)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn renders_the_placeholder_when_empty() {
        let mut input = Input::with_placeholder("Enter a URL");
        assert_eq!(render_to_string(&mut input, 11, false), "Enter a URL");
    }

    #[test]
    fn scrolls_to_keep_the_caret_visible() {
        let mut input = typed("abcdefghij");
        let rendered = render_to_string(&mut input, 4, true);
        // The caret is at the end, so the tail is what shows.
        assert!(rendered.ends_with(' '), "caret cell: {rendered:?}");
        assert!(rendered.starts_with("hij"), "{rendered:?}");
    }

    #[test]
    fn backward_edits_reveal_the_hidden_unicode_prefix() {
        let mut input = typed("日本語入力");
        render_to_string(&mut input, 7, true);
        assert_eq!(&input.value()[input.scroll..], "語入力");

        input.handle_key(key(KeyCode::Backspace));
        render_to_string(&mut input, 7, true);
        assert_eq!(&input.value()[input.scroll..], "本語入");
        input.handle_key(key(KeyCode::Left));
        render_to_string(&mut input, 7, true);
        assert_eq!(input.scroll, 0);
    }

    #[test]
    fn password_masks_every_grapheme() {
        let mut input = typed("secret");
        input.password = true;
        assert_eq!(render_to_string(&mut input, 6, false), "••••••");
    }

    #[test]
    fn the_caret_cell_is_painted_when_focused() {
        let mut input = typed("ab");
        input.set_cursor(0);
        let area = Rect::new(0, 0, 4, 1);
        let mut buffer = Buffer::empty(area);
        input.render(area, &mut buffer, true, &[]);
        assert_eq!(buffer[(0, 0)].style().bg, theme::cursor().bg);
        assert_ne!(buffer[(1, 0)].style().bg, theme::cursor().bg);
    }

    #[test]
    fn highlights_reach_the_rendered_spans() {
        let mut input = typed("ab");
        let area = Rect::new(0, 0, 4, 1);
        let mut buffer = Buffer::empty(area);
        input.render(
            area,
            &mut buffer,
            false,
            &[Highlight {
                range: 0..1,
                style: Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
            }],
        );
        assert_eq!(buffer[(0, 0)].style().fg, Some(theme::ACCENT));
        assert_ne!(buffer[(1, 0)].style().fg, Some(theme::ACCENT));
    }

    #[test]
    fn a_zero_sized_area_is_not_a_panic() {
        let mut input = typed("abc");
        let area = Rect::new(0, 0, 0, 0);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        input.render(area, &mut buffer, true, &[]);
    }
}
