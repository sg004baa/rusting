//! A transparent, keyboard-only checkbox.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget as _;

use crate::theme;

// Keep this to one ASCII cell: the terminal backend coalesces adjacent cell writes.
const CHECKED: &str = "x";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckboxAction {
    Ignored,
    Consumed,
    Toggled,
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkbox {
    pub label: String,
    pub checked: bool,
}

impl Checkbox {
    pub fn new(label: impl Into<String>, checked: bool) -> Self {
        Self {
            label: label.into(),
            checked,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CheckboxAction {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return CheckboxAction::Ignored;
        }
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter => {
                self.checked = !self.checked;
                CheckboxAction::Toggled
            }
            KeyCode::Up | KeyCode::Char('k') => CheckboxAction::LeaveUp,
            KeyCode::Down | KeyCode::Char('j') => CheckboxAction::LeaveDown,
            _ => CheckboxAction::Ignored,
        }
    }

    /// Draws the checkmark and label without painting a pill or any other
    /// background. Focus is represented by bold foreground text only.
    pub fn render(&self, area: Rect, buffer: &mut Buffer, focused: bool) {
        if area.is_empty() {
            return;
        }
        let style = if focused {
            Style::new().add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        let marker = if self.checked { CHECKED } else { " " };
        let marker_style = if self.checked {
            style.fg(theme::ACCENT)
        } else {
            style
        };
        Line::from(vec![
            Span::styled(marker, marker_style),
            Span::raw(" "),
            Span::styled(self.label.as_str(), style),
        ])
        .render(area, buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;
    use unicode_width::UnicodeWidthStr as _;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn space_and_enter_toggle_the_value() {
        let mut checkbox = Checkbox::new("Wrap", false);
        assert_eq!(
            checkbox.handle_key(key(KeyCode::Char(' '))),
            CheckboxAction::Toggled
        );
        assert!(checkbox.checked);
        assert_eq!(
            checkbox.handle_key(key(KeyCode::Enter)),
            CheckboxAction::Toggled
        );
        assert!(!checkbox.checked);
    }

    #[test]
    fn vertical_keys_leave_the_control() {
        let mut checkbox = Checkbox::new("Wrap", false);
        assert_eq!(
            checkbox.handle_key(key(KeyCode::Char('k'))),
            CheckboxAction::LeaveUp
        );
        assert_eq!(
            checkbox.handle_key(key(KeyCode::Down)),
            CheckboxAction::LeaveDown
        );
    }

    #[test]
    fn render_uses_the_exact_markers_and_no_background() {
        let area = Rect::new(0, 0, 12, 1);
        let mut buffer = Buffer::empty(area);
        Checkbox::new("Wrap", true).render(area, &mut buffer, true);
        let rendered = (0..6).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
        assert_eq!(rendered, "x Wrap");
        for x in 0..area.width {
            assert_eq!(buffer[(x, 0)].style().bg, Some(Color::Reset));
        }

        let mut buffer = Buffer::empty(area);
        Checkbox::new("Wrap", false).render(area, &mut buffer, false);
        let rendered = (0..6).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
        assert_eq!(rendered, "  Wrap");
    }

    #[test]
    fn checked_marker_is_an_unambiguous_single_terminal_cell() {
        assert!(CHECKED.is_ascii());
        assert_eq!(CHECKED.len(), 1);
        assert_eq!(CHECKED.width(), 1);
    }
}
