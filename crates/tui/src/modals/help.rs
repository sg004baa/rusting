use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _, Wrap};
use unicode_width::UnicodeWidthStr as _;

use super::{Modal, ModalResult, frame, percent_size};
use crate::theme;

/// Contextual help text followed by a scrollable keybinding table.
#[derive(Debug, Clone)]
pub struct HelpModal {
    title: String,
    description: String,
    bindings: Vec<(String, String)>,
    scroll: usize,
    viewport_height: usize,
    content_height: usize,
}

impl HelpModal {
    pub fn new(title: String, description: &str, bindings: Vec<(String, String)>) -> Self {
        let content_height = description.lines().count().max(1) + bindings.len() + 4;
        Self {
            title,
            description: description.trim().to_owned(),
            bindings,
            scroll: 0,
            viewport_height: 1,
            content_height,
        }
    }

    fn max_scroll(&self) -> usize {
        self.content_height.saturating_sub(self.viewport_height)
    }

    fn move_by(&mut self, amount: isize) {
        self.scroll = if amount.is_negative() {
            self.scroll.saturating_sub(amount.unsigned_abs())
        } else {
            self.scroll
                .saturating_add(amount as usize)
                .min(self.max_scroll())
        };
    }

    fn content(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if self.description.is_empty() {
            lines.push(Line::styled(
                format!("No help available for {}", self.title),
                theme::placeholder(),
            ));
        } else {
            lines.extend(
                self.description
                    .lines()
                    .map(|line| Line::raw(line.to_owned())),
            );
        }
        lines.push(Line::default());
        lines.push(Line::styled(
            "Key                Description",
            Style::new().add_modifier(Modifier::BOLD),
        ));

        let key_width = width.saturating_div(3).clamp(8, 20);
        for (key, description) in &self.bindings {
            let key_display = truncate(key, key_width);
            let padding = key_width.saturating_sub(key_display.width()) + 2;
            lines.push(Line::from(vec![
                Span::styled(key_display, Style::new().add_modifier(Modifier::BOLD)),
                Span::raw(" ".repeat(padding)),
                Span::raw(description.clone()),
            ]));
        }
        lines.push(Line::default());
        lines.push(Line::styled(
            "Press ESC to dismiss.",
            Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM),
        ));
        lines
    }
}

impl Modal for HelpModal {
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        match key.code {
            KeyCode::Esc => ModalResult::Cancelled,
            KeyCode::Down | KeyCode::Char('j' | 'J') if plain => {
                self.move_by(1);
                ModalResult::Open
            }
            KeyCode::Up | KeyCode::Char('k' | 'K') if plain => {
                self.move_by(-1);
                ModalResult::Open
            }
            KeyCode::PageDown if plain => {
                self.move_by(self.viewport_height.max(1) as isize);
                ModalResult::Open
            }
            KeyCode::PageUp if plain => {
                self.move_by(-(self.viewport_height.max(1) as isize));
                ModalResult::Open
            }
            KeyCode::Home if plain => {
                self.scroll = 0;
                ModalResult::Open
            }
            KeyCode::End if plain => {
                self.scroll = self.max_scroll();
                ModalResult::Open
            }
            _ => ModalResult::Open,
        }
    }

    fn render(&mut self, screen: Rect, buffer: &mut Buffer) {
        let area = percent_size(screen, 65, 80);
        let inner = frame(&self.title, area, buffer);
        self.viewport_height = usize::from(inner.height);
        let lines = self.content(usize::from(inner.width).max(1));
        self.content_height = visual_height(&lines, usize::from(inner.width).max(1));
        self.scroll = self.scroll.min(self.max_scroll());
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll.min(u16::MAX as usize) as u16, 0))
            .render(inner, buffer);
    }
}

fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let mut out = String::new();
    let target = width - 1;
    for ch in text.chars() {
        if out.width() + ch.to_string().width() > target {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn visual_height(lines: &[Line<'_>], width: usize) -> usize {
    lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width.max(1)))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn escape_closes_and_navigation_scrolls() {
        let bindings = (0..30)
            .map(|index| (format!("key-{index}"), format!("binding {index}")))
            .collect();
        let mut modal = HelpModal::new("Editor".into(), "Editing help", bindings);
        let area = Rect::new(0, 0, 80, 20);
        let mut buffer = Buffer::empty(area);
        modal.render(area, &mut buffer);
        assert_eq!(modal.scroll, 0);
        modal.handle_key(key(KeyCode::PageDown));
        assert!(modal.scroll > 0);
        assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalResult::Cancelled);
    }

    #[test]
    fn content_is_plain_text_and_contains_the_binding_table() {
        let mut modal = HelpModal::new(
            "URL".into(),
            "Use variables like **TOKEN**.",
            vec![("ctrl+j".into(), "send request".into())],
        );
        let area = Rect::new(0, 0, 80, 24);
        let mut buffer = Buffer::empty(area);
        modal.render(area, &mut buffer);
        let text = super::super::buffer_text(&buffer, area);
        assert!(text.contains("**TOKEN**"));
        assert!(text.contains("ctrl+j"));
        assert!(text.contains("send request"));
    }
}
