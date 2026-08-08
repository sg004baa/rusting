//! A compact select control with an explicitly rendered overlay.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget as _};

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectAction {
    Ignored,
    Consumed,
    Changed,
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone)]
pub struct Select<T> {
    options: Vec<(String, T)>,
    mnemonics: Vec<Option<(char, usize)>>,
    selected: usize,
    highlighted: usize,
    open: bool,
    overlay_scroll: usize,
    pub placeholder: String,
}

impl<T: Clone + PartialEq> Select<T> {
    /// Creates a select from `(display label, value)` pairs.
    ///
    /// A select necessarily has a value, so constructing one without options is
    /// a programming error.
    pub fn new(options: Vec<(String, T)>) -> Self {
        assert!(!options.is_empty(), "Select requires at least one option");
        let option_count = options.len();
        Self {
            options,
            mnemonics: vec![None; option_count],
            selected: 0,
            highlighted: 0,
            open: false,
            overlay_scroll: 0,
            placeholder: String::new(),
        }
    }

    /// Sets each option's mnemonic and its UTF-8 byte position in the label.
    /// Missing entries simply have no mnemonic; excess entries are ignored.
    pub fn with_mnemonics(mut self, mnemonics: Vec<Option<(char, usize)>>) -> Self {
        for (target, mnemonic) in self.mnemonics.iter_mut().zip(mnemonics) {
            *target = mnemonic;
        }
        self
    }

    pub fn value(&self) -> &T {
        &self.options[self.selected].1
    }

    pub fn set_value(&mut self, value: &T) -> bool {
        let Some(index) = self
            .options
            .iter()
            .position(|(_, candidate)| candidate == value)
        else {
            return false;
        };
        self.selected = index;
        self.highlighted = index;
        true
    }

    pub fn label(&self) -> &str {
        &self.options[self.selected].0
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self) {
        self.open = false;
        self.highlighted = self.selected;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SelectAction {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return SelectAction::Ignored;
        }

        if let KeyCode::Char(pressed) = key.code
            && let Some(index) = self.mnemonic_index(pressed)
        {
            let changed = index != self.selected;
            self.selected = index;
            self.highlighted = index;
            self.open = false;
            return if changed {
                SelectAction::Changed
            } else {
                SelectAction::Consumed
            };
        }

        match key.code {
            KeyCode::Enter | KeyCode::Char(' ' | 'l') => {
                if self.open {
                    let changed = self.highlighted != self.selected;
                    self.selected = self.highlighted;
                    self.close();
                    if changed {
                        SelectAction::Changed
                    } else {
                        SelectAction::Consumed
                    }
                } else {
                    self.highlighted = self.selected;
                    self.open = true;
                    SelectAction::Consumed
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !self.open {
                    return SelectAction::LeaveUp;
                }
                self.highlighted = self
                    .highlighted
                    .checked_sub(1)
                    .unwrap_or(self.options.len() - 1);
                SelectAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.open {
                    return SelectAction::LeaveDown;
                }
                self.highlighted = (self.highlighted + 1) % self.options.len();
                SelectAction::Consumed
            }
            KeyCode::Esc if self.open => {
                self.close();
                SelectAction::Consumed
            }
            _ => SelectAction::Ignored,
        }
    }

    /// Draws the closed control. Its border belongs to the caller.
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        if area.is_empty() {
            return;
        }
        let style = if focused {
            Style::new().add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        let label = self.label();
        let mnemonic = self.mnemonics[self.selected].map(|(_, byte)| byte);
        Line::from(styled_label(label, mnemonic, style)).render(area, buffer);
    }

    /// Draws the open option list after the rest of the screen. The overlay is
    /// cleared first and flips above the anchor when that side has more room.
    pub fn render_overlay(&mut self, anchor: Rect, screen: Rect, buffer: &mut Buffer) {
        if !self.open || screen.is_empty() {
            return;
        }
        let wanted = self.options.len().min(usize::from(u16::MAX)) as u16;
        let below = screen.bottom().saturating_sub(anchor.bottom());
        let above = anchor.y.saturating_sub(screen.y);
        let use_below = wanted <= below || below >= above;
        let available = if use_below { below } else { above };
        let height = wanted.min(available);
        if height == 0 || anchor.width == 0 {
            return;
        }
        let y = if use_below {
            anchor.bottom()
        } else {
            anchor.y.saturating_sub(height)
        };
        let area = Rect::new(anchor.x, y, anchor.width, height);
        Clear.render(area, buffer);

        let visible = usize::from(height);
        if self.highlighted < self.overlay_scroll {
            self.overlay_scroll = self.highlighted;
        } else if self.highlighted >= self.overlay_scroll + visible {
            self.overlay_scroll = self.highlighted + 1 - visible;
        }
        self.overlay_scroll = self
            .overlay_scroll
            .min(self.options.len().saturating_sub(visible));

        for row in 0..visible {
            let index = self.overlay_scroll + row;
            let row_area = Rect::new(area.x, area.y + row as u16, area.width, 1);
            if index == self.highlighted {
                buffer.set_style(row_area, theme::selection());
            }
            let mnemonic = self.mnemonics[index].map(|(_, byte)| byte);
            let base = if index == self.highlighted {
                theme::selection()
            } else {
                Style::new()
            };
            Line::from(styled_label(&self.options[index].0, mnemonic, base))
                .render(row_area, buffer);
        }
    }

    fn mnemonic_index(&self, pressed: char) -> Option<usize> {
        self.mnemonics.iter().position(|mnemonic| {
            mnemonic.is_some_and(|(expected, _)| expected.eq_ignore_ascii_case(&pressed))
        })
    }
}

fn styled_label(label: &str, mnemonic: Option<usize>, base: Style) -> Vec<Span<'_>> {
    let Some(start) =
        mnemonic.filter(|index| *index < label.len() && label.is_char_boundary(*index))
    else {
        return vec![Span::styled(label, base)];
    };
    let Some(character) = label[start..].chars().next() else {
        return vec![Span::styled(label, base)];
    };
    let end = start + character.len_utf8();
    let mut spans = Vec::with_capacity(3);
    if start > 0 {
        spans.push(Span::styled(&label[..start], base));
    }
    spans.push(Span::styled(
        &label[start..end],
        base.add_modifier(Modifier::UNDERLINED),
    ));
    if end < label.len() {
        spans.push(Span::styled(&label[end..], base));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn select() -> Select<u8> {
        Select::new(vec![
            ("GET".into(), 1),
            ("POST".into(), 2),
            ("PUT".into(), 3),
        ])
        .with_mnemonics(vec![Some(('g', 0)), Some(('p', 0)), Some(('u', 1))])
    }

    #[test]
    fn set_value_changes_only_to_a_known_option() {
        let mut select = select();
        assert!(select.set_value(&2));
        assert_eq!(select.value(), &2);
        assert_eq!(select.label(), "POST");
        assert!(!select.set_value(&9));
        assert_eq!(select.value(), &2);
    }

    #[test]
    fn closed_vertical_motion_leaves_the_control() {
        let mut select = select();
        assert_eq!(select.handle_key(key(KeyCode::Up)), SelectAction::LeaveUp);
        assert_eq!(
            select.handle_key(key(KeyCode::Char('j'))),
            SelectAction::LeaveDown
        );
    }

    #[test]
    fn overlay_navigation_wraps_and_commits_or_cancels() {
        let mut select = select();
        assert_eq!(
            select.handle_key(key(KeyCode::Enter)),
            SelectAction::Consumed
        );
        assert!(select.is_open());
        select.handle_key(key(KeyCode::Up));
        assert_eq!(
            select.handle_key(key(KeyCode::Char('l'))),
            SelectAction::Changed
        );
        assert_eq!(select.value(), &3);
        assert!(!select.is_open());

        select.handle_key(key(KeyCode::Char(' ')));
        select.handle_key(key(KeyCode::Down));
        assert_eq!(select.handle_key(key(KeyCode::Esc)), SelectAction::Consumed);
        assert_eq!(select.value(), &3);
    }

    #[test]
    fn mnemonics_select_directly_whether_open_or_closed() {
        let mut select = select();
        assert_eq!(
            select.handle_key(key(KeyCode::Char('P'))),
            SelectAction::Changed
        );
        assert_eq!(select.value(), &2);
        select.handle_key(key(KeyCode::Enter));
        assert!(select.is_open());
        assert_eq!(
            select.handle_key(key(KeyCode::Char('u'))),
            SelectAction::Changed
        );
        assert_eq!(select.value(), &3);
        assert!(!select.is_open());
    }

    #[test]
    fn mnemonic_byte_is_underlined_without_painting_a_background() {
        let area = Rect::new(0, 0, 5, 1);
        let mut buffer = Buffer::empty(area);
        let mut select = select();
        select.set_value(&3);
        select.render(area, &mut buffer, true);
        assert!(
            buffer[(1, 0)]
                .style()
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
        for x in 0..area.width {
            assert_ne!(buffer[(x, 0)].style().bg, theme::selection().bg);
            assert_ne!(buffer[(x, 0)].style().bg, theme::cursor().bg);
        }
    }

    #[test]
    fn overlay_flips_above_and_highlights_the_cursor_row() {
        let screen = Rect::new(0, 0, 10, 5);
        let anchor = Rect::new(0, 4, 5, 1);
        let mut buffer = Buffer::empty(screen);
        let mut select = select();
        select.handle_key(key(KeyCode::Enter));
        select.render_overlay(anchor, screen, &mut buffer);
        assert_eq!(buffer[(0, 1)].symbol(), "G");
        assert_eq!(buffer[(0, 1)].style().bg, theme::selection().bg);
        assert_eq!(buffer[(0, 4)].symbol(), " ");
    }
}
