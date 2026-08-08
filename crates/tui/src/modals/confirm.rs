use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _, Wrap};
use unicode_width::UnicodeWidthStr as _;

use super::{Modal, ModalResult, centered, control, frame};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Confirm,
    Cancel,
}

/// A two-button confirmation dialog.
#[derive(Debug, Clone)]
pub struct ConfirmModal {
    title: String,
    message: String,
    confirm_label: String,
    cancel_label: String,
    focused: Button,
}

impl ConfirmModal {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        confirm_label: impl Into<String>,
        cancel_label: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            confirm_label: confirm_label.into(),
            cancel_label: cancel_label.into(),
            focused: Button::Confirm,
        }
    }

    fn activate_focused(&self) -> ModalResult {
        match self.focused {
            Button::Confirm => ModalResult::Accepted,
            Button::Cancel => ModalResult::Cancelled,
        }
    }

    fn move_focus(&mut self) {
        self.focused = match self.focused {
            Button::Confirm => Button::Cancel,
            Button::Cancel => Button::Confirm,
        };
    }
}

impl Modal for ConfirmModal {
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        match key.code {
            KeyCode::Esc => ModalResult::Cancelled,
            KeyCode::Char('y' | 'Y') if plain => ModalResult::Accepted,
            KeyCode::Char('n' | 'N') if plain => ModalResult::Cancelled,
            KeyCode::Enter | KeyCode::Char(' ') if plain => self.activate_focused(),
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('h' | 'H' | 'j' | 'J' | 'k' | 'K' | 'l' | 'L')
                if plain =>
            {
                self.move_focus();
                ModalResult::Open
            }
            _ => ModalResult::Open,
        }
    }

    fn render(&mut self, screen: Rect, buffer: &mut Buffer) {
        let widest_message = self
            .message
            .lines()
            .map(|line| line.width())
            .max()
            .unwrap_or(0);
        let buttons_width = self.confirm_label.width() + self.cancel_label.width() + 8;
        let width =
            (widest_message.max(buttons_width).max(self.title.width()) + 4).clamp(24, 72) as u16;
        let text_width = usize::from(width.saturating_sub(4)).max(1);
        let message_height: usize = self
            .message
            .lines()
            .map(|line| line.width().max(1).div_ceil(text_width))
            .sum();
        let height = (message_height + 8).min(usize::from(screen.height)) as u16;
        let area = centered(screen, width, height.max(7));
        let inner = frame(&self.title, area, buffer);
        let rows = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .spacing(1)
        .split(inner);

        Paragraph::new(self.message.as_str())
            .wrap(Wrap { trim: false })
            .render(rows[0], buffer);

        let button_widths =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(rows[2]);
        render_button(
            &self.confirm_label,
            self.focused == Button::Confirm,
            button_widths[0],
            buffer,
        );
        render_button(
            &self.cancel_label,
            self.focused == Button::Cancel,
            button_widths[1],
            buffer,
        );
    }
}

fn render_button(label: &str, focused: bool, area: Rect, buffer: &mut Buffer) {
    let style = if focused {
        theme::selection()
    } else {
        Style::new().fg(theme::MUTED)
    };
    let inner = control(area, buffer, focused);
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(label.to_owned(), style),
        Span::raw(" "),
    ]);
    Paragraph::new(line)
        .style(style)
        .centered()
        .render(inner, buffer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn direct_keys_and_escape_return_the_settled_results() {
        let mut modal = ConfirmModal::new("Confirm", "Delete it?", "Delete", "Cancel");
        assert_eq!(
            modal.handle_key(key(KeyCode::Char('y'))),
            ModalResult::Accepted
        );
        assert_eq!(
            modal.handle_key(key(KeyCode::Char('n'))),
            ModalResult::Cancelled
        );
        assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalResult::Cancelled);
    }

    #[test]
    fn enter_and_space_activate_the_focused_button() {
        let mut modal = ConfirmModal::new("Confirm", "Delete it?", "Delete", "Cancel");
        assert_eq!(modal.handle_key(key(KeyCode::Enter)), ModalResult::Accepted);
        assert_eq!(modal.handle_key(key(KeyCode::Right)), ModalResult::Open);
        assert_eq!(
            modal.handle_key(key(KeyCode::Char(' '))),
            ModalResult::Cancelled
        );
    }

    #[test]
    fn rendering_does_not_paint_the_modal_surface() {
        let area = Rect::new(0, 0, 50, 12);
        let mut buffer = Buffer::empty(area);
        let mut modal = ConfirmModal::new("Confirm", "Delete it?", "Delete", "Cancel");
        modal.render(area, &mut buffer);
        assert_eq!(
            buffer[(25, 3)].style().bg,
            Some(ratatui::style::Color::Reset)
        );
        assert_eq!(
            buffer[(0, 0)].style().bg,
            Some(ratatui::style::Color::Reset)
        );
    }
}
