//! Floating completion candidates.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Widget as _};

use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupItem {
    pub text: String,
    /// Character positions returned by `widgets::fuzzy::rank`.
    pub match_positions: Vec<u32>,
    pub style: Style,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupAction {
    Ignored,
    Consumed,
    Accepted(usize),
    Dismissed,
}

#[derive(Debug, Clone)]
pub struct Popup {
    items: Vec<PopupItem>,
    open: bool,
    selected: Option<usize>,
    scroll: usize,
    pub max_height: u16,
}

impl Default for Popup {
    fn default() -> Self {
        Self::new()
    }
}

impl Popup {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            open: false,
            selected: None,
            scroll: 0,
            max_height: 12,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Replaces the candidates and opens at the first one. Empty candidate
    /// lists close the popup.
    pub fn open(&mut self, items: Vec<PopupItem>) {
        self.items = items;
        self.scroll = 0;
        self.open = !self.items.is_empty();
        self.selected = self.open.then_some(0);
    }

    /// Dismisses the visible list but retains its candidates so `Down` can
    /// reopen it without making the caller recompute completions.
    pub fn close(&mut self) {
        self.open = false;
        self.selected = None;
        self.scroll = 0;
    }

    pub fn items(&self) -> &[PopupItem] {
        &self.items
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PopupAction {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return PopupAction::Ignored;
        }
        match key.code {
            KeyCode::Down => {
                if self.items.is_empty() {
                    return PopupAction::Ignored;
                }
                if !self.open {
                    self.open = true;
                    self.selected = Some(0);
                } else if let Some(selected) = self.selected {
                    self.selected = Some((selected + 1) % self.items.len());
                }
                PopupAction::Consumed
            }
            KeyCode::Up if self.open => {
                if let Some(selected) = self.selected {
                    self.selected = Some(
                        selected
                            .checked_sub(1)
                            .unwrap_or(self.items.len().saturating_sub(1)),
                    );
                }
                PopupAction::Consumed
            }
            KeyCode::Enter | KeyCode::Tab if self.open => {
                let Some(selected) = self.selected else {
                    return PopupAction::Ignored;
                };
                self.close();
                PopupAction::Accepted(selected)
            }
            KeyCode::Esc if self.open => {
                self.close();
                PopupAction::Dismissed
            }
            _ => PopupAction::Ignored,
        }
    }

    /// Draws the popup below the caret anchor, or above it when the requested
    /// height does not fit below. `Clear` restores transparent reset cells
    /// before the list is painted.
    pub fn render(&mut self, anchor: Rect, screen: Rect, buffer: &mut Buffer) {
        if !self.open || self.items.is_empty() || self.max_height == 0 || screen.is_empty() {
            return;
        }
        let wanted = self
            .max_height
            .min(self.items.len().min(usize::from(u16::MAX)) as u16);
        let below = screen.bottom().saturating_sub(anchor.bottom());
        let above = anchor.y.saturating_sub(screen.y);
        let use_below = wanted <= below;
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

        let selected = self.selected.unwrap_or(0);
        let visible = usize::from(height);
        if selected < self.scroll {
            self.scroll = selected;
        } else if selected >= self.scroll + visible {
            self.scroll = selected + 1 - visible;
        }
        self.scroll = self.scroll.min(self.items.len().saturating_sub(visible));

        for row in 0..visible {
            let index = self.scroll + row;
            let row_area = Rect::new(area.x, area.y + row as u16, area.width, 1);
            let selected_style = (index == selected).then(theme::selection);
            if let Some(style) = selected_style {
                buffer.set_style(row_area, style);
            }
            let item = &self.items[index];
            let base = selected_style.map_or(item.style, |style| item.style.patch(style));
            Line::from(item_spans(item, base)).render(row_area, buffer);
        }
    }
}

fn item_spans(item: &PopupItem, base: Style) -> Vec<Span<'_>> {
    if item.text.is_empty() {
        return vec![Span::styled("", base)];
    }
    item.text
        .char_indices()
        .enumerate()
        .map(|(character_index, (byte, character))| {
            let end = byte + character.len_utf8();
            let style = if item.match_positions.contains(&(character_index as u32)) {
                base.patch(theme::selection())
            } else {
                base
            };
            Span::styled(&item.text[byte..end], style)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn item(text: &str) -> PopupItem {
        PopupItem {
            text: text.into(),
            match_positions: Vec::new(),
            style: Style::new(),
        }
    }

    #[test]
    fn an_empty_candidate_list_stays_closed() {
        let mut popup = Popup::new();
        popup.open(Vec::new());
        assert!(!popup.is_open());
        assert_eq!(popup.selected(), None);
        assert_eq!(popup.handle_key(key(KeyCode::Down)), PopupAction::Ignored);
    }

    #[test]
    fn navigation_wraps_and_acceptance_reports_the_candidate_index() {
        let mut popup = Popup::new();
        popup.open(vec![item("one"), item("two")]);
        assert_eq!(popup.selected(), Some(0));
        popup.handle_key(key(KeyCode::Up));
        assert_eq!(popup.selected(), Some(1));
        popup.handle_key(key(KeyCode::Down));
        assert_eq!(popup.selected(), Some(0));
        popup.handle_key(key(KeyCode::Down));
        assert_eq!(
            popup.handle_key(key(KeyCode::Tab)),
            PopupAction::Accepted(1)
        );
        assert!(!popup.is_open());
    }

    #[test]
    fn dismissed_candidates_can_be_reopened_with_down() {
        let mut popup = Popup::new();
        popup.open(vec![item("one")]);
        assert_eq!(popup.handle_key(key(KeyCode::Esc)), PopupAction::Dismissed);
        assert_eq!(popup.items().len(), 1);
        assert_eq!(popup.handle_key(key(KeyCode::Down)), PopupAction::Consumed);
        assert!(popup.is_open());
        assert_eq!(popup.selected(), Some(0));
    }

    #[test]
    fn popup_flips_above_when_it_does_not_fit_below() {
        let screen = Rect::new(0, 0, 8, 5);
        let anchor = Rect::new(0, 4, 8, 1);
        let mut buffer = Buffer::empty(screen);
        let mut popup = Popup::new();
        popup.open(vec![item("one"), item("two")]);
        popup.render(anchor, screen, &mut buffer);
        assert_eq!(buffer[(0, 2)].symbol(), "o");
        assert_eq!(buffer[(0, 3)].symbol(), "t");
        assert_eq!(buffer[(0, 4)].symbol(), " ");
    }

    #[test]
    fn match_positions_are_interpreted_as_character_indices() {
        let screen = Rect::new(0, 0, 8, 3);
        let anchor = Rect::new(0, 0, 8, 1);
        let mut buffer = Buffer::empty(screen);
        let mut popup = Popup::new();
        popup.open(vec![
            item("plain"),
            PopupItem {
                text: "aéb".into(),
                match_positions: vec![1],
                style: Style::new(),
            },
        ]);
        popup.render(anchor, screen, &mut buffer);
        assert_ne!(buffer[(0, 2)].style().bg, theme::selection().bg);
        assert_eq!(buffer[(1, 2)].symbol(), "é");
        assert_eq!(buffer[(1, 2)].style().bg, theme::selection().bg);
    }

    #[test]
    fn scrolling_keeps_the_selected_candidate_visible() {
        let screen = Rect::new(0, 0, 8, 4);
        let anchor = Rect::new(0, 0, 8, 1);
        let mut buffer = Buffer::empty(screen);
        let mut popup = Popup::new();
        popup.max_height = 2;
        popup.open(vec![item("zero"), item("one"), item("two")]);
        popup.handle_key(key(KeyCode::Down));
        popup.handle_key(key(KeyCode::Down));
        popup.render(anchor, screen, &mut buffer);
        assert_eq!(buffer[(0, 1)].symbol(), "o");
        assert_eq!(buffer[(0, 2)].symbol(), "t");
        assert_eq!(buffer[(0, 2)].style().bg, theme::selection().bg);
    }
}
