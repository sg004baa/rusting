use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};

use super::{Modal, ModalResult};
use crate::theme;

/// Transparent labels painted directly over the application's jump targets.
#[derive(Debug, Clone)]
pub struct JumpOverlay {
    targets: Vec<(char, Position)>,
    taken: Option<char>,
}

impl JumpOverlay {
    pub fn new(targets: Vec<(char, Position)>) -> Self {
        Self {
            targets,
            taken: None,
        }
    }

    pub fn taken(&self) -> Option<char> {
        self.taken
    }
}

impl Modal for JumpOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        if key.code == KeyCode::Esc {
            return ModalResult::Cancelled;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return ModalResult::Open;
        }
        let pressed = match key.code {
            KeyCode::Tab | KeyCode::BackTab => Some('\t'),
            KeyCode::Char(character) => Some(character),
            _ => None,
        };
        let Some(pressed) = pressed else {
            return ModalResult::Open;
        };
        if self.targets.iter().any(|(label, _)| *label == pressed) {
            self.taken = Some(pressed);
            ModalResult::Accepted
        } else {
            ModalResult::Open
        }
    }

    fn render(&mut self, screen: Rect, buffer: &mut Buffer) {
        for (label, position) in &self.targets {
            if position.x < screen.x
                || position.x >= screen.right()
                || position.y < screen.y
                || position.y >= screen.bottom()
            {
                continue;
            }
            let display = if *label == '\t' { '⇥' } else { *label };
            buffer[(position.x, position.y)]
                .set_char(display)
                .set_style(theme::selection());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn records_character_and_tab_targets() {
        let targets = vec![('q', Position::new(2, 1)), ('\t', Position::new(5, 1))];
        let mut overlay = JumpOverlay::new(targets.clone());
        assert_eq!(
            overlay.handle_key(key(KeyCode::Char('q'))),
            ModalResult::Accepted
        );
        assert_eq!(overlay.taken(), Some('q'));

        let mut overlay = JumpOverlay::new(targets);
        assert_eq!(overlay.handle_key(key(KeyCode::Tab)), ModalResult::Accepted);
        assert_eq!(overlay.taken(), Some('\t'));
    }

    #[test]
    fn rendering_changes_only_target_cells_and_does_not_dim_the_screen() {
        let area = Rect::new(0, 0, 8, 3);
        let mut buffer = Buffer::empty(area);
        buffer.set_style(area, Style::new().bg(Color::Blue));
        let mut overlay = JumpOverlay::new(vec![('1', Position::new(2, 1))]);
        overlay.render(area, &mut buffer);
        assert_eq!(buffer[(2, 1)].symbol(), "1");
        assert_eq!(buffer[(2, 1)].style().bg, theme::selection().bg);
        assert_eq!(buffer[(3, 1)].style().bg, Some(Color::Blue));
    }

    #[test]
    fn unknown_keys_leave_the_overlay_open() {
        let mut overlay = JumpOverlay::new(vec![('q', Position::new(0, 0))]);
        assert_eq!(
            overlay.handle_key(key(KeyCode::Char('x'))),
            ModalResult::Open
        );
        assert_eq!(overlay.taken(), None);
        assert_eq!(
            overlay.handle_key(key(KeyCode::Esc)),
            ModalResult::Cancelled
        );
    }
}
