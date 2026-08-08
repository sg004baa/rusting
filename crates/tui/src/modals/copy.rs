use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};
use rusting_core::KeyValue;

use super::{Modal, ModalResult, centered, frame};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyChoice {
    Name,
    Value,
    Both,
}

/// Selects which part of a key/value row the application should copy.
#[derive(Debug, Clone)]
pub struct CopyModal {
    row: KeyValue,
    cursor: usize,
    choice: Option<CopyChoice>,
}

impl CopyModal {
    pub fn new(row: KeyValue) -> Self {
        Self {
            row,
            cursor: 0,
            choice: None,
        }
    }

    pub fn row(&self) -> &KeyValue {
        &self.row
    }

    pub fn choice(&self) -> Option<CopyChoice> {
        self.choice
    }

    /// Returns the clipboard text after a choice has been accepted.
    pub fn text(&self) -> Option<String> {
        match self.choice? {
            CopyChoice::Name => Some(self.row.name.clone()),
            CopyChoice::Value => Some(self.row.value.clone()),
            CopyChoice::Both => Some(format!("{}: {}", self.row.name, self.row.value)),
        }
    }

    fn select(&mut self, choice: CopyChoice) -> ModalResult {
        self.choice = Some(choice);
        ModalResult::Accepted
    }

    fn current_choice(&self) -> CopyChoice {
        match self.cursor {
            0 => CopyChoice::Name,
            1 => CopyChoice::Value,
            _ => CopyChoice::Both,
        }
    }
}

impl Modal for CopyModal {
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        match key.code {
            KeyCode::Esc => ModalResult::Cancelled,
            KeyCode::Char('n' | 'N') if plain => self.select(CopyChoice::Name),
            KeyCode::Char('v' | 'V') if plain => self.select(CopyChoice::Value),
            KeyCode::Char('b' | 'B') if plain => self.select(CopyChoice::Both),
            KeyCode::Down | KeyCode::Char('j' | 'J') if plain => {
                self.cursor = (self.cursor + 1) % 3;
                ModalResult::Open
            }
            KeyCode::Up | KeyCode::Char('k' | 'K') if plain => {
                self.cursor = (self.cursor + 2) % 3;
                ModalResult::Open
            }
            KeyCode::Enter | KeyCode::Char(' ' | 'l' | 'L') if plain => {
                self.select(self.current_choice())
            }
            _ => ModalResult::Open,
        }
    }

    fn render(&mut self, screen: Rect, buffer: &mut Buffer) {
        let area = centered(screen, 30, 5);
        let inner = frame("Copy", area, buffer);
        let options = [("Copy name", "n"), ("Copy value", "v"), ("Copy both", "b")];
        for (index, (label, binding)) in options.into_iter().enumerate() {
            let y = inner.y.saturating_add(index as u16);
            if y >= inner.bottom() {
                break;
            }
            let row = Rect::new(inner.x, y, inner.width, 1);
            let row_style = if self.cursor == index {
                theme::selection()
            } else {
                Style::new()
            };
            let line = Line::from(vec![
                Span::styled(format!(" {label}"), row_style),
                Span::styled(
                    format!(" [{binding}] "),
                    row_style.patch(Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM)),
                ),
            ]);
            Paragraph::new(line).style(row_style).render(row, buffer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn exposes_the_source_row_without_copying_to_the_clipboard() {
        let row = KeyValue::new("Authorization", "Bearer token");
        let modal = CopyModal::new(row);
        assert_eq!(modal.row().name, "Authorization");
        assert_eq!(modal.row().value, "Bearer token");
        assert_eq!(modal.choice(), None);
        assert_eq!(modal.text(), None);
    }

    #[test]
    fn direct_choices_produce_the_expected_text() {
        let mut modal = CopyModal::new(KeyValue::new("Accept", "application/json"));
        assert_eq!(
            modal.handle_key(key(KeyCode::Char('b'))),
            ModalResult::Accepted
        );
        assert_eq!(modal.choice(), Some(CopyChoice::Both));
        assert_eq!(modal.text().as_deref(), Some("Accept: application/json"));
    }

    #[test]
    fn navigation_wraps_and_enter_uses_the_highlighted_choice() {
        let mut modal = CopyModal::new(KeyValue::new("A", "B"));
        modal.handle_key(key(KeyCode::Up));
        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalResult::Accepted);
        assert_eq!(modal.choice(), Some(CopyChoice::Both));
    }
}
