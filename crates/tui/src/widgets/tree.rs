//! A flattened collection tree with caller-owned expansion state.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget as _;

use crate::theme;

pub type NodeId = usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub id: NodeId,
    pub depth: usize,
    pub label: Line<'static>,
    pub expandable: bool,
    pub expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeAction {
    Ignored,
    Consumed,
    Selected(NodeId),
    Toggle(NodeId),
    CollapseParent(NodeId),
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, Default)]
pub struct Tree {
    nodes: Vec<TreeNode>,
    cursor: Option<usize>,
    scroll: usize,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the caller-flattened visible nodes. The selected id wins over
    /// positional stability; if it disappeared, the old row is clamped into
    /// the new list rather than jumping unconditionally to the top.
    pub fn set_nodes(&mut self, nodes: Vec<TreeNode>) {
        let previous_index = self.cursor.unwrap_or(0);
        let previous_id = self.cursor();
        self.nodes = nodes;
        self.cursor = if self.nodes.is_empty() {
            None
        } else if let Some(id) = previous_id {
            self.nodes
                .iter()
                .position(|node| node.id == id)
                .or_else(|| Some(previous_index.min(self.nodes.len() - 1)))
        } else {
            Some(0)
        };
        self.scroll = self.scroll.min(self.nodes.len().saturating_sub(1));
    }

    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }

    pub fn cursor(&self) -> Option<NodeId> {
        self.cursor.map(|index| self.nodes[index].id)
    }

    pub fn set_cursor(&mut self, id: NodeId) {
        if let Some(index) = self.nodes.iter().position(|node| node.id == id) {
            self.cursor = Some(index);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> TreeAction {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return TreeAction::Ignored;
        }
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Char('J') => self.move_expandable(true),
            KeyCode::Char('K') => self.move_expandable(false),
            KeyCode::Char('g') | KeyCode::Home => {
                if !self.nodes.is_empty() {
                    self.cursor = Some(0);
                }
                TreeAction::Consumed
            }
            KeyCode::Char('G') | KeyCode::End => {
                if !self.nodes.is_empty() {
                    self.cursor = Some(self.nodes.len() - 1);
                }
                TreeAction::Consumed
            }
            KeyCode::Enter | KeyCode::Char('l') => {
                let Some(index) = self.cursor else {
                    return TreeAction::Consumed;
                };
                let node = &self.nodes[index];
                if node.expandable {
                    TreeAction::Toggle(node.id)
                } else {
                    TreeAction::Selected(node.id)
                }
            }
            KeyCode::Char(' ' | 'r') => {
                let Some(index) = self.cursor else {
                    return TreeAction::Consumed;
                };
                let node = &self.nodes[index];
                if node.expandable {
                    TreeAction::Toggle(node.id)
                } else {
                    TreeAction::Consumed
                }
            }
            KeyCode::Char('h') => self.collapse_parent(),
            _ => TreeAction::Ignored,
        }
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        if area.is_empty() || self.nodes.is_empty() {
            return;
        }
        let Some(cursor) = self.cursor else {
            return;
        };
        let visible = usize::from(area.height);
        if cursor < self.scroll {
            self.scroll = cursor;
        } else if cursor >= self.scroll + visible {
            self.scroll = cursor + 1 - visible;
        }
        self.scroll = self.scroll.min(self.nodes.len().saturating_sub(visible));

        for row in 0..visible {
            let index = self.scroll + row;
            let Some(node) = self.nodes.get(index) else {
                break;
            };
            let row_area = Rect::new(area.x, area.y + row as u16, area.width, 1);
            if index == cursor {
                let style = if focused {
                    theme::selection()
                } else {
                    Style::new().add_modifier(Modifier::BOLD)
                };
                buffer.set_style(row_area, style);
            }

            let indent = node.depth.saturating_mul(2).min(usize::from(area.width));
            let mut spans = Vec::with_capacity(node.label.spans.len() + 2);
            spans.push(Span::raw(" ".repeat(indent)));
            if node.expandable {
                spans.push(Span::styled(
                    if node.expanded {
                        "\u{25bc} "
                    } else {
                        "\u{25b6} "
                    },
                    Style::new().add_modifier(Modifier::DIM),
                ));
            }
            spans.extend(node.label.spans.iter().cloned());
            Line::from(spans).render(row_area, buffer);
        }
    }

    fn move_up(&mut self) -> TreeAction {
        let Some(cursor) = self.cursor else {
            return TreeAction::LeaveUp;
        };
        if cursor == 0 {
            TreeAction::LeaveUp
        } else {
            self.cursor = Some(cursor - 1);
            TreeAction::Consumed
        }
    }

    fn move_down(&mut self) -> TreeAction {
        let Some(cursor) = self.cursor else {
            return TreeAction::LeaveDown;
        };
        if cursor + 1 >= self.nodes.len() {
            TreeAction::LeaveDown
        } else {
            self.cursor = Some(cursor + 1);
            TreeAction::Consumed
        }
    }

    fn move_expandable(&mut self, forward: bool) -> TreeAction {
        let Some(cursor) = self.cursor else {
            return TreeAction::Consumed;
        };
        let found = if forward {
            ((cursor + 1)..self.nodes.len()).find(|index| self.nodes[*index].expandable)
        } else {
            (0..cursor)
                .rev()
                .find(|index| self.nodes[*index].expandable)
        };
        if let Some(index) = found {
            self.cursor = Some(index);
        }
        TreeAction::Consumed
    }

    fn collapse_parent(&self) -> TreeAction {
        let Some(cursor) = self.cursor else {
            return TreeAction::Consumed;
        };
        let depth = self.nodes[cursor].depth;
        if depth == 0 {
            return TreeAction::Consumed;
        }
        (0..cursor)
            .rev()
            .find(|index| self.nodes[*index].depth < depth)
            .map_or(TreeAction::Consumed, |index| {
                TreeAction::CollapseParent(self.nodes[index].id)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn node(id: NodeId, depth: usize, expandable: bool, expanded: bool) -> TreeNode {
        TreeNode {
            id,
            depth,
            label: Line::from(format!("node-{id}")),
            expandable,
            expanded,
        }
    }

    fn tree() -> Tree {
        let mut tree = Tree::new();
        tree.set_nodes(vec![
            node(10, 0, true, true),
            node(11, 1, false, false),
            node(12, 1, true, false),
            node(13, 2, false, false),
        ]);
        tree
    }

    #[test]
    fn replacing_nodes_preserves_the_cursor_by_id() {
        let mut tree = tree();
        tree.set_cursor(12);
        tree.set_nodes(vec![
            node(12, 0, true, true),
            node(13, 1, false, false),
            node(10, 0, true, false),
        ]);
        assert_eq!(tree.cursor(), Some(12));
    }

    #[test]
    fn disappearing_cursor_clamps_the_previous_row() {
        let mut tree = tree();
        tree.set_cursor(13);
        tree.set_nodes(vec![node(10, 0, true, true), node(11, 1, false, false)]);
        assert_eq!(tree.cursor(), Some(11));
        tree.set_nodes(Vec::new());
        assert_eq!(tree.cursor(), None);
    }

    #[test]
    fn ordinary_motion_reports_edges_instead_of_wrapping() {
        let mut tree = tree();
        assert_eq!(tree.handle_key(key(KeyCode::Up)), TreeAction::LeaveUp);
        assert_eq!(tree.handle_key(key(KeyCode::Down)), TreeAction::Consumed);
        assert_eq!(tree.cursor(), Some(11));
        tree.handle_key(key(KeyCode::Char('G')));
        assert_eq!(tree.cursor(), Some(13));
        assert_eq!(
            tree.handle_key(key(KeyCode::Char('j'))),
            TreeAction::LeaveDown
        );
        tree.handle_key(key(KeyCode::Char('g')));
        assert_eq!(tree.cursor(), Some(10));
    }

    #[test]
    fn uppercase_motion_visits_only_expandable_nodes() {
        let mut tree = tree();
        assert_eq!(
            tree.handle_key(key(KeyCode::Char('J'))),
            TreeAction::Consumed
        );
        assert_eq!(tree.cursor(), Some(12));
        tree.handle_key(key(KeyCode::Char('K')));
        assert_eq!(tree.cursor(), Some(10));
    }

    #[test]
    fn selection_and_expansion_actions_include_the_node_id() {
        let mut tree = tree();
        assert_eq!(tree.handle_key(key(KeyCode::Enter)), TreeAction::Toggle(10));
        assert_eq!(
            tree.handle_key(key(KeyCode::Char('l'))),
            TreeAction::Toggle(10)
        );
        tree.set_cursor(11);
        assert_eq!(
            tree.handle_key(key(KeyCode::Char('l'))),
            TreeAction::Selected(11)
        );
        assert_eq!(
            tree.handle_key(key(KeyCode::Enter)),
            TreeAction::Selected(11)
        );
        assert_eq!(
            tree.handle_key(key(KeyCode::Char(' '))),
            TreeAction::Consumed
        );
        tree.set_cursor(12);
        assert_eq!(
            tree.handle_key(key(KeyCode::Char('r'))),
            TreeAction::Toggle(12)
        );
    }

    #[test]
    fn h_reports_the_nearest_visible_parent() {
        let mut tree = tree();
        tree.set_cursor(13);
        assert_eq!(
            tree.handle_key(key(KeyCode::Char('h'))),
            TreeAction::CollapseParent(12)
        );
        tree.set_cursor(10);
        assert_eq!(
            tree.handle_key(key(KeyCode::Char('h'))),
            TreeAction::Consumed
        );
    }

    #[test]
    fn render_uses_two_cell_depth_arrow_prefixes_and_selection_only() {
        let area = Rect::new(0, 0, 16, 4);
        let mut buffer = Buffer::empty(area);
        let mut tree = tree();
        tree.set_cursor(12);
        tree.render(area, &mut buffer, true);

        assert_eq!(buffer[(0, 0)].symbol(), "\u{25bc}");
        assert_eq!(buffer[(0, 1)].symbol(), " ");
        assert_eq!(buffer[(2, 1)].symbol(), "n");
        assert_eq!(buffer[(2, 2)].symbol(), "\u{25b6}");
        assert_eq!(buffer[(2, 2)].style().bg, theme::selection().bg);
        assert_ne!(buffer[(0, 1)].style().bg, theme::selection().bg);
    }

    #[test]
    fn render_scrolls_to_keep_the_cursor_visible() {
        let area = Rect::new(0, 0, 12, 2);
        let mut buffer = Buffer::empty(area);
        let mut tree = tree();
        tree.set_cursor(13);
        tree.render(area, &mut buffer, true);
        assert_eq!(buffer[(2, 0)].symbol(), "\u{25b6}");
        assert_eq!(buffer[(4, 1)].symbol(), "n");
        assert_eq!(buffer[(0, 1)].style().bg, theme::selection().bg);
    }
}
