use std::time::{Duration, Instant};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Widget as _},
};
use rusting_script::types::Severity;
use unicode_width::UnicodeWidthStr as _;

use crate::theme;

const TOAST_LIFETIME: Duration = Duration::from_secs(5);
const MAX_TOAST_WIDTH: u16 = 52;

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub severity: Severity,
    pub shown_at: Instant,
}

#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    pub fn push(&mut self, message: impl Into<String>, severity: Severity) {
        let now = Instant::now();
        self.discard_expired(now);
        let message = message.into();
        if self
            .items
            .iter()
            .any(|toast| toast.message == message && toast.severity == severity)
        {
            return;
        }
        self.items.push(Toast {
            message,
            severity,
            shown_at: now,
        });
    }

    pub fn tick(&mut self) {
        self.discard_expired(Instant::now());
    }

    fn discard_expired(&mut self, now: Instant) {
        self.items
            .retain(|toast| now.saturating_duration_since(toast.shown_at) < TOAST_LIFETIME);
    }

    pub fn render(&self, screen: Rect, buffer: &mut Buffer) {
        let mut bottom = screen.bottom();
        for toast in self.items.iter().rev() {
            let lines = toast.message.lines().collect::<Vec<_>>();
            let longest = lines.iter().map(|line| line.width()).max().unwrap_or(0);
            let width = u16::try_from(longest)
                .unwrap_or(u16::MAX)
                .saturating_add(4)
                .clamp(18.min(screen.width), MAX_TOAST_WIDTH.min(screen.width));
            let content_width = usize::from(width.saturating_sub(2).max(1));
            let wrapped_lines = lines
                .iter()
                .map(|line| line.width().max(1).div_ceil(content_width))
                .sum::<usize>()
                .max(1);
            let content_height = u16::try_from(wrapped_lines).unwrap_or(u16::MAX);
            let height = content_height
                .saturating_add(2)
                .min(bottom.saturating_sub(screen.y));
            if height < 3 {
                break;
            }
            let y = bottom.saturating_sub(height);
            let area = Rect::new(screen.right().saturating_sub(width), y, width, height);
            Clear.render(area, buffer);
            let (title, color) = match toast.severity {
                Severity::Information => ("Information", theme::PRIMARY),
                Severity::Warning => ("Warning", theme::WARNING),
                Severity::Error => ("Error", theme::ERROR),
            };
            let block = Block::new()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(color))
                .title(Line::styled(title, Style::default().fg(color)));
            Paragraph::new(toast.message.as_str())
                .wrap(ratatui::widgets::Wrap { trim: false })
                .block(block)
                .render(area, buffer);
            bottom = y.saturating_sub(1);
            if bottom <= screen.y {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_discards_only_expired_toasts() {
        let mut toasts = Toasts::default();
        toasts.push("old", Severity::Warning);
        toasts.push("new", Severity::Information);
        toasts.items[0].shown_at = Instant::now() - Duration::from_secs(6);
        toasts.tick();
        assert_eq!(toasts.items.len(), 1);
        assert_eq!(toasts.items[0].message, "new");
    }

    #[test]
    fn live_duplicates_do_not_stack_or_refresh_their_expiry() {
        let mut toasts = Toasts::default();
        toasts.push("Collection reloaded", Severity::Information);
        let original = Instant::now() - Duration::from_secs(4);
        toasts.items[0].shown_at = original;

        toasts.push("Collection reloaded", Severity::Information);

        assert_eq!(toasts.items.len(), 1);
        assert_eq!(toasts.items[0].shown_at, original);
    }

    #[test]
    fn identical_messages_with_different_severities_are_distinct() {
        let mut toasts = Toasts::default();
        toasts.push("request failed", Severity::Warning);
        toasts.push("request failed", Severity::Error);

        assert_eq!(toasts.items.len(), 2);
        assert_eq!(toasts.items[0].severity, Severity::Warning);
        assert_eq!(toasts.items[1].severity, Severity::Error);
    }

    #[test]
    fn expired_duplicate_is_discarded_before_a_fresh_toast_is_added() {
        let mut toasts = Toasts::default();
        toasts.push("Collection reloaded", Severity::Information);
        let expired = Instant::now() - TOAST_LIFETIME;
        toasts.items[0].shown_at = expired;

        toasts.push("Collection reloaded", Severity::Information);

        assert_eq!(toasts.items.len(), 1);
        assert!(toasts.items[0].shown_at > expired);
    }

    #[test]
    fn renders_newest_toast_at_bottom_right() {
        let area = Rect::new(0, 0, 40, 10);
        let mut buffer = Buffer::empty(area);
        let mut toasts = Toasts::default();
        toasts.push("request complete", Severity::Information);
        toasts.render(area, &mut buffer);
        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("request complete"));
        assert!(rendered.contains("Information"));
    }
}
