//! Modal dialogs and transparent overlays.

pub mod confirm;
pub mod copy;
pub mod help;
pub mod jump;
pub mod load_env;
pub mod new_request;
pub mod palette;

pub use confirm::ConfirmModal;
pub use copy::{CopyChoice, CopyModal};
pub use help::HelpModal;
pub use jump::JumpOverlay;
pub use load_env::LoadEnvModal;
pub use new_request::{NewRequestData, NewRequestModal};
pub use palette::{Palette, PaletteItem};

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget as _};

use crate::theme;

/// Common interface used by the application to host exactly one modal at a time.
pub trait Modal {
    /// `Open` keeps the modal alive; either other value closes it.
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult;

    /// Draws the modal against the complete terminal area.
    fn render(&mut self, screen: Rect, buffer: &mut Buffer);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalResult {
    Open,
    Cancelled,
    Accepted,
}

pub(crate) fn centered(screen: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(screen.width);
    let height = height.min(screen.height);
    Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

pub(crate) fn percent(value: u16, percentage: u16) -> u16 {
    (u32::from(value) * u32::from(percentage))
        .div_ceil(100)
        .min(u32::from(u16::MAX)) as u16
}

pub(crate) fn percent_size(screen: Rect, width_percent: u16, height_percent: u16) -> Rect {
    centered(
        screen,
        percent(screen.width, width_percent).max(1),
        percent(screen.height, height_percent).max(1),
    )
}

/// Clears only the modal rectangle and draws a border with no background style.
pub(crate) fn frame(title: &str, area: Rect, buffer: &mut Buffer) -> Rect {
    Clear.render(area, buffer);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border(true))
        .title(title.to_owned())
        .title_style(theme::border_title(true));
    let inner = block.inner(area);
    block.render(area, buffer);
    inner
}

pub(crate) fn control(area: Rect, buffer: &mut Buffer, focused: bool) -> Rect {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border(focused));
    let inner = block.inner(area);
    block.render(area, buffer);
    inner
}

#[cfg(test)]
pub(crate) fn buffer_text(buffer: &Buffer, area: Rect) -> String {
    let mut text = String::new();
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}
