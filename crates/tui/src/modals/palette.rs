use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use super::{Modal, ModalResult, centered, control, frame, percent};
use crate::theme;
use crate::widgets::fuzzy;
use crate::widgets::{Input, InputAction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub label: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    /// Text searched in addition to the visible label, such as a request URL.
    pub search_extra: Option<String>,
    pub id: usize,
}

#[derive(Debug, Clone)]
struct RankedItem {
    item_index: usize,
    score: u32,
    label_positions: Vec<u32>,
}

/// Reusable fuzzy command/request chooser.
pub struct Palette {
    input: Input,
    items: Vec<PaletteItem>,
    ranked: Vec<RankedItem>,
    selected: usize,
    chosen: Option<usize>,
}

impl Palette {
    pub fn new(placeholder: &str, items: Vec<PaletteItem>) -> Self {
        let mut palette = Self {
            input: Input::with_placeholder(placeholder),
            items,
            ranked: Vec::new(),
            selected: 0,
            chosen: None,
        };
        palette.rank();
        palette
    }

    pub fn chosen(&self) -> Option<usize> {
        self.chosen
    }

    fn rank(&mut self) {
        let needle = self.input.value();
        if needle.is_empty() {
            self.ranked = self
                .items
                .iter()
                .enumerate()
                .map(|(item_index, _)| RankedItem {
                    item_index,
                    score: 0,
                    label_positions: Vec::new(),
                })
                .collect();
            self.selected = self.selected.min(self.ranked.len().saturating_sub(1));
            return;
        }

        let mut ranked = Vec::new();
        for (item_index, item) in self.items.iter().enumerate() {
            let label_match = fuzzy::rank(needle, &[item.label.as_str()])
                .into_iter()
                .next();
            let extra_match = item
                .search_extra
                .as_deref()
                .and_then(|extra| fuzzy::rank(needle, &[extra]).into_iter().next());
            let (score, label_positions) = match (label_match, extra_match) {
                (None, None) => continue,
                (Some(label), None) => (label.score, label.positions),
                (None, Some(extra)) => (extra.score, Vec::new()),
                (Some(label), Some(extra)) => (label.score.max(extra.score), label.positions),
            };
            ranked.push(RankedItem {
                item_index,
                score,
                label_positions,
            });
        }
        ranked.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then(left.item_index.cmp(&right.item_index))
        });
        self.ranked = ranked;
        self.selected = 0;
    }

    fn choose_selected(&mut self) -> ModalResult {
        let Some(ranked) = self.ranked.get(self.selected) else {
            return ModalResult::Open;
        };
        self.chosen = Some(self.items[ranked.item_index].id);
        ModalResult::Accepted
    }

    fn move_down(&mut self) {
        if !self.ranked.is_empty() {
            self.selected = (self.selected + 1) % self.ranked.len();
        }
    }

    fn move_up(&mut self) {
        if !self.ranked.is_empty() {
            self.selected = (self.selected + self.ranked.len() - 1) % self.ranked.len();
        }
    }
}

impl Modal for Palette {
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match key.code {
            KeyCode::Esc => return ModalResult::Cancelled,
            KeyCode::Enter => return self.choose_selected(),
            KeyCode::Down => {
                self.move_down();
                return ModalResult::Open;
            }
            KeyCode::Up => {
                self.move_up();
                return ModalResult::Open;
            }
            _ => {}
        }

        match self.input.handle_key(key) {
            InputAction::Changed => {
                self.rank();
                ModalResult::Open
            }
            InputAction::Submitted => self.choose_selected(),
            InputAction::Ignored
            | InputAction::Consumed
            | InputAction::LeaveUp
            | InputAction::LeaveDown => ModalResult::Open,
        }
    }

    fn render(&mut self, screen: Rect, buffer: &mut Buffer) {
        let width = percent(screen.width, 60).max(1);
        let item_count = self.ranked.len().min(usize::from(u16::MAX)) as u16;
        let desired_height = item_count.saturating_add(6).clamp(7, 16);
        let area = centered(screen, width, desired_height);
        let inner = frame("Palette", area, buffer);
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);
        let input_area = control(rows[0], buffer, true);
        self.input.render(input_area, buffer, true, &[]);

        if self.ranked.is_empty() {
            Paragraph::new("No matches")
                .style(theme::placeholder())
                .centered()
                .render(rows[2], buffer);
            return;
        }

        let visible = usize::from(rows[2].height);
        let offset = if self.selected >= visible {
            self.selected + 1 - visible
        } else {
            0
        };
        for (row_index, ranked) in self.ranked.iter().skip(offset).take(visible).enumerate() {
            let y = rows[2].y + row_index as u16;
            let row = Rect::new(rows[2].x, y, rows[2].width, 1);
            let selected = offset + row_index == self.selected;
            let row_style = if selected {
                theme::selection()
            } else {
                Style::new()
            };
            buffer.set_style(row, row_style);
            let item = &self.items[ranked.item_index];
            let hint_width = item.hint.as_deref().map(|hint| hint.width()).unwrap_or(0);
            let content_limit = usize::from(row.width)
                .saturating_sub(hint_width)
                .saturating_sub(2);
            let mut line = highlighted_label(
                &item.label,
                &ranked.label_positions,
                content_limit,
                row_style,
            );
            let label_width = line.width();
            if label_width > 0
                && let Some(detail) = &item.detail
            {
                let detail_style =
                    row_style.patch(Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM));
                let detail_line = highlighted_label(
                    detail,
                    &[],
                    content_limit.saturating_sub(label_width).saturating_sub(1),
                    detail_style,
                );
                if !detail_line.spans.is_empty() {
                    line.spans.push(Span::styled(" ", detail_style));
                    line.spans.extend(detail_line.spans);
                }
            }
            Paragraph::new(line).render(row, buffer);
            if let Some(hint) = &item.hint {
                let hint_x = row
                    .right()
                    .saturating_sub(hint_width.min(usize::from(row.width)) as u16);
                buffer.set_stringn(
                    hint_x,
                    row.y,
                    hint,
                    usize::from(row.right().saturating_sub(hint_x)),
                    row_style.patch(Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM)),
                );
            }
        }
    }
}

fn highlighted_label(label: &str, positions: &[u32], width: usize, base: Style) -> Line<'static> {
    let mut spans = Vec::new();
    let mut segment = String::new();
    let mut segment_matched = false;
    let mut used_width = 0;
    for (char_index, character) in label.chars().enumerate() {
        let character_width = character.width().unwrap_or(0);
        if used_width + character_width > width {
            break;
        }
        let matched = positions.binary_search(&(char_index as u32)).is_ok();
        if !segment.is_empty() && matched != segment_matched {
            let style = if segment_matched {
                base.patch(Style::new().add_modifier(Modifier::BOLD))
            } else {
                base
            };
            spans.push(Span::styled(std::mem::take(&mut segment), style));
        }
        segment_matched = matched;
        segment.push(character);
        used_width += character_width;
    }
    if !segment.is_empty() {
        let style = if segment_matched {
            base.patch(Style::new().add_modifier(Modifier::BOLD))
        } else {
            base
        };
        spans.push(Span::styled(segment, style));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn item(label: &str, extra: Option<&str>, id: usize) -> PaletteItem {
        PaletteItem {
            label: label.into(),
            detail: None,
            hint: None,
            search_extra: extra.map(str::to_owned),
            id,
        }
    }

    fn type_text(palette: &mut Palette, text: &str) {
        for character in text.chars() {
            palette.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
    }

    fn find_text(buffer: &Buffer, y: u16, text: &str) -> Option<u16> {
        let symbols = text
            .chars()
            .map(|character| character.to_string())
            .collect::<Vec<_>>();
        let width = u16::try_from(symbols.len()).ok()?;
        if width > buffer.area.width {
            return None;
        }
        (buffer.area.left()..=buffer.area.right().saturating_sub(width)).find(|start| {
            symbols
                .iter()
                .enumerate()
                .all(|(offset, symbol)| buffer[(*start + offset as u16, y)].symbol() == symbol)
        })
    }

    #[test]
    fn searches_labels_and_extra_text_using_the_better_score() {
        let mut palette = Palette::new(
            "Search requests",
            vec![
                item("List users", Some("https://api.test/users"), 10),
                item("Health", Some("https://api.test/status"), 20),
            ],
        );
        type_text(&mut palette, "status");
        assert_eq!(palette.ranked.len(), 1);
        assert_eq!(palette.items[palette.ranked[0].item_index].id, 20);
        assert!(palette.ranked[0].label_positions.is_empty());
    }

    #[test]
    fn enter_returns_the_item_id_and_empty_query_preserves_input_order() {
        let mut palette = Palette::new(
            "Commands",
            vec![item("First", None, 42), item("Second", None, 7)],
        );
        assert_eq!(
            palette.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            ModalResult::Open
        );
        assert_eq!(
            palette.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            ModalResult::Accepted
        );
        assert_eq!(palette.chosen(), Some(7));
    }

    #[test]
    fn matched_label_positions_are_rendered_bold() {
        let mut palette = Palette::new("Commands", vec![item("Copy YAML", None, 1)]);
        type_text(&mut palette, "cpy");
        let area = Rect::new(0, 0, 60, 12);
        let mut buffer = Buffer::empty(area);
        palette.render(area, &mut buffer);
        let has_bold_match = buffer
            .content
            .iter()
            .any(|cell| cell.symbol() == "C" && cell.style().add_modifier.contains(Modifier::BOLD));
        assert!(has_bold_match);
    }

    #[test]
    fn detail_renders_beside_label_in_ghost_style_and_keeps_hint_right_aligned() {
        let mut palette = Palette::new(
            "Search requests",
            vec![PaletteItem {
                label: "Health".to_owned(),
                detail: Some("users/admin".to_owned()),
                hint: Some("GET".to_owned()),
                search_extra: None,
                id: 1,
            }],
        );
        let area = Rect::new(0, 0, 80, 12);
        let mut buffer = Buffer::empty(area);

        palette.render(area, &mut buffer);

        let (y, label_x) = (area.top()..area.bottom())
            .find_map(|y| find_text(&buffer, y, "Health").map(|x| (y, x)))
            .expect("rendered label");
        let detail_x = find_text(&buffer, y, "users/admin").expect("rendered detail");
        let hint_x = find_text(&buffer, y, "GET").expect("rendered hint");
        assert_eq!(detail_x, label_x + "Health ".len() as u16);
        for x in detail_x..detail_x + "users/admin".len() as u16 {
            let style = buffer[(x, y)].style();
            assert_eq!(style.fg, Some(theme::MUTED));
            assert!(style.add_modifier.contains(Modifier::DIM));
            assert_eq!(style.bg, theme::selection().bg);
        }
        let right_border = (area.left()..area.right())
            .rev()
            .find(|x| buffer[(*x, y)].symbol() == "│")
            .expect("right frame border");
        assert_eq!(hint_x + "GET".len() as u16, right_border);
    }

    #[test]
    fn long_detail_and_hint_render_safely_at_narrow_width() {
        let mut palette = Palette::new(
            "Search requests",
            vec![PaletteItem {
                label: "Very long request name".to_owned(),
                detail: Some("users/administrators/archive".to_owned()),
                hint: Some("EXTREMELY-LONG-METHOD".to_owned()),
                search_extra: None,
                id: 1,
            }],
        );
        let area = Rect::new(0, 0, 10, 8);
        let mut buffer = Buffer::empty(area);

        palette.render(area, &mut buffer);

        assert_eq!(buffer.area, area);
        assert_eq!(buffer.content.len(), usize::from(area.width * area.height));
    }
}
