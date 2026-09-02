//! Collection browser: a flattened directory/request tree with request actions.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Widget as _};
use rusting_core::{Collection, HttpMethod, files, urls};

use crate::theme;
use crate::widgets::tree::{NodeId, Tree, TreeAction, TreeNode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionAction {
    Ignored,
    Consumed,
    Open(PathBuf),
    NewRequest { parent: PathBuf },
    Duplicate { path: PathBuf, quick: bool },
    Delete { path: PathBuf, confirm: bool },
    SearchRequested,
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum NodeKey {
    Directory(PathBuf),
    Request(PathBuf),
    PathlessRequest {
        parent: PathBuf,
        index: usize,
        name: String,
    },
}

#[derive(Debug, Clone)]
enum NodeMeta {
    Directory {
        path: PathBuf,
    },
    Request {
        path: Option<PathBuf>,
        parent: PathBuf,
        description: String,
    },
}

pub struct CollectionPane {
    collection: Collection,
    tree: Tree,
    expanded: HashSet<PathBuf>,
    known_directories: HashSet<PathBuf>,
    ids: HashMap<NodeKey, NodeId>,
    next_id: NodeId,
    meta: HashMap<NodeId, NodeMeta>,
    open: Option<PathBuf>,
    base_urls: Vec<String>,
    area: Rect,
}

impl CollectionPane {
    pub fn new(collection: &Collection) -> Self {
        let known_directories = directory_paths(collection);
        let expanded = known_directories.clone();
        let mut pane = Self {
            collection: collection.clone(),
            tree: Tree::new(),
            expanded,
            known_directories,
            ids: HashMap::new(),
            next_id: 0,
            meta: HashMap::new(),
            open: None,
            base_urls: collect_base_urls(collection),
            area: Rect::ZERO,
        };
        pane.rebuild(None);
        pane
    }

    pub fn reload(&mut self, collection: &Collection) {
        let selected = self.selected_key();
        let directories = directory_paths(collection);
        for path in directories.difference(&self.known_directories) {
            self.expanded.insert(path.clone());
        }
        self.expanded.retain(|path| directories.contains(path));
        self.known_directories = directories;
        self.collection = collection.clone();
        self.base_urls = collect_base_urls(collection);
        self.rebuild(selected);
    }

    pub fn set_open(&mut self, path: Option<PathBuf>) {
        let selected = self.selected_key();
        self.open = path;
        self.rebuild(selected);
    }

    pub fn selected_request(&self) -> Option<&Path> {
        let id = self.tree.cursor()?;
        match self.meta.get(&id)? {
            NodeMeta::Request {
                path: Some(path), ..
            } => Some(path.as_path()),
            NodeMeta::Directory { .. } | NodeMeta::Request { path: None, .. } => None,
        }
    }

    pub fn select_request(&mut self, path: &Path) {
        if !expand_to_request(&self.collection, path, &mut self.expanded) {
            return;
        }
        self.rebuild(Some(NodeKey::Request(path.to_path_buf())));
    }

    pub fn target_directory(&self) -> PathBuf {
        let Some(id) = self.tree.cursor() else {
            return self.collection.path.clone();
        };
        match self.meta.get(&id) {
            Some(NodeMeta::Directory { path, .. }) => path.clone(),
            Some(NodeMeta::Request { parent, .. }) => parent.clone(),
            None => self.collection.path.clone(),
        }
    }

    pub fn base_urls(&self) -> Vec<String> {
        self.base_urls.clone()
    }

    pub fn restore_collapsed_directories(&mut self, collapsed: &HashSet<PathBuf>) {
        let selected = self.selected_key();
        self.expanded.retain(|path| !collapsed.contains(path));
        self.rebuild(selected);
    }

    pub fn collapsed_directories(&self) -> impl Iterator<Item = &Path> {
        self.known_directories
            .difference(&self.expanded)
            .map(PathBuf::as_path)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CollectionAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('n') {
            return CollectionAction::NewRequest {
                parent: self.target_directory(),
            };
        }
        if key.code == KeyCode::Char('/') && key.modifiers.is_empty() {
            return CollectionAction::SearchRequested;
        }
        if matches!(
            key.code,
            KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Backspace
        ) {
            let confirm =
                key.code != KeyCode::Char('D') && !key.modifiers.contains(KeyModifiers::SHIFT);
            return self
                .selected_request()
                .map(|path| CollectionAction::Delete {
                    path: path.to_path_buf(),
                    confirm,
                })
                .unwrap_or(CollectionAction::Consumed);
        }
        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            let quick =
                key.code == KeyCode::Char('Y') || key.modifiers.contains(KeyModifiers::SHIFT);
            return self
                .selected_request()
                .map(|path| CollectionAction::Duplicate {
                    path: path.to_path_buf(),
                    quick,
                })
                .unwrap_or(CollectionAction::Consumed);
        }

        match self.tree.handle_key(key) {
            TreeAction::Selected(id) => match self.meta.get(&id) {
                Some(NodeMeta::Request {
                    path: Some(path), ..
                }) => CollectionAction::Open(path.clone()),
                _ => CollectionAction::Consumed,
            },
            TreeAction::Toggle(id) => {
                if let Some(NodeMeta::Directory { path, .. }) = self.meta.get(&id) {
                    let path = path.clone();
                    if !self.expanded.remove(&path) {
                        self.expanded.insert(path.clone());
                    }
                    self.rebuild(Some(NodeKey::Directory(path)));
                }
                CollectionAction::Consumed
            }
            TreeAction::CollapseParent(id) => {
                let path = match self.meta.get(&id) {
                    Some(NodeMeta::Directory { path, .. }) => Some(path.clone()),
                    Some(NodeMeta::Request { parent, .. }) => Some(parent.clone()),
                    None => None,
                };
                if let Some(path) = path {
                    self.expanded.remove(&path);
                    self.rebuild(Some(NodeKey::Directory(path)));
                }
                CollectionAction::Consumed
            }
            TreeAction::LeaveUp => CollectionAction::LeaveUp,
            TreeAction::LeaveDown => CollectionAction::LeaveDown,
            TreeAction::Ignored => CollectionAction::Ignored,
            TreeAction::Consumed => CollectionAction::Consumed,
        }
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.area = area;
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::border(focused))
            .title(
                Line::from(Span::styled("Collection", theme::border_title(focused)))
                    .right_aligned(),
            )
            .title_bottom(
                Line::from(Span::styled(
                    self.collection.name.clone(),
                    Style::new().fg(theme::MUTED),
                ))
                .right_aligned(),
            );
        let inner = block.inner(area);
        block.render(area, buffer);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        if self.collection.requests.is_empty() && self.collection.children.is_empty() {
            render_empty(inner, buffer);
            return;
        }

        let preview = self.selected_description().to_owned();
        let preview_height = if preview.is_empty() {
            0
        } else {
            3.min(inner.height)
        };
        let tree_area = Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(preview_height),
        );
        self.tree.render(tree_area, buffer, focused);
        if preview_height > 0 {
            let preview_area = Rect::new(
                inner.x,
                inner.y + inner.height - preview_height,
                inner.width,
                preview_height,
            );
            let lines = preview
                .lines()
                .take(3)
                .map(|line| Line::from(Span::styled(line, Style::new().fg(theme::MUTED))))
                .collect::<Vec<_>>();
            Paragraph::new(lines).render(preview_area, buffer);
        }
    }

    pub fn jump_targets(&self) -> Vec<(char, Position)> {
        vec![('\t', Position::new(self.area.x, self.area.y))]
    }

    fn selected_description(&self) -> &str {
        let Some(id) = self.tree.cursor() else {
            return "";
        };
        match self.meta.get(&id) {
            Some(NodeMeta::Request { description, .. }) => description,
            _ => "",
        }
    }

    fn selected_key(&self) -> Option<NodeKey> {
        let id = self.tree.cursor()?;
        match self.meta.get(&id)? {
            NodeMeta::Directory { path, .. } => Some(NodeKey::Directory(path.clone())),
            NodeMeta::Request {
                path: Some(path), ..
            } => Some(NodeKey::Request(path.clone())),
            NodeMeta::Request { path: None, .. } => self
                .ids
                .iter()
                .find_map(|(key, candidate)| (*candidate == id).then(|| key.clone())),
        }
    }

    fn rebuild(&mut self, preferred: Option<NodeKey>) {
        let mut nodes = Vec::new();
        let mut meta = HashMap::new();
        let mut ids = std::mem::take(&mut self.ids);
        let mut next_id = self.next_id;
        flatten_collection(
            &self.collection,
            0,
            &self.expanded,
            self.open.as_deref(),
            &mut ids,
            &mut next_id,
            &mut nodes,
            &mut meta,
        );
        self.next_id = next_id;
        self.ids = ids;
        self.meta = meta;
        self.tree.set_nodes(nodes);
        if let Some(key) = preferred
            && let Some(id) = self.ids.get(&key)
        {
            self.tree.set_cursor(*id);
        }
    }
}

fn expand_to_request(
    collection: &Collection,
    path: &Path,
    expanded: &mut HashSet<PathBuf>,
) -> bool {
    if collection
        .requests
        .iter()
        .any(|request| request.path.as_deref() == Some(path))
    {
        return true;
    }
    for child in &collection.children {
        if expand_to_request(child, path, expanded) {
            expanded.insert(child.path.clone());
            return true;
        }
    }
    false
}

fn has_request_in_subtree(collection: &Collection) -> bool {
    !collection.requests.is_empty() || collection.children.iter().any(has_request_in_subtree)
}

#[allow(clippy::too_many_arguments)]
fn flatten_collection(
    collection: &Collection,
    depth: usize,
    expanded: &HashSet<PathBuf>,
    open: Option<&Path>,
    ids: &mut HashMap<NodeKey, NodeId>,
    next_id: &mut NodeId,
    nodes: &mut Vec<TreeNode>,
    meta: &mut HashMap<NodeId, NodeMeta>,
) {
    for (index, request) in collection.requests.iter().enumerate() {
        let key = match &request.path {
            Some(path) => NodeKey::Request(path.clone()),
            None => NodeKey::PathlessRequest {
                parent: collection.path.clone(),
                index,
                name: request.name.clone(),
            },
        };
        let id = id_for(key, ids, next_id);
        let is_open = request
            .path
            .as_deref()
            .is_some_and(|path| open == Some(path));
        nodes.push(TreeNode {
            id,
            depth,
            label: request_label(request.method, &request.name, is_open),
            expandable: false,
            expanded: false,
        });
        meta.insert(
            id,
            NodeMeta::Request {
                path: request.path.clone(),
                parent: collection.path.clone(),
                description: request.description.clone(),
            },
        );
    }

    for child in &collection.children {
        if !has_request_in_subtree(child) {
            continue;
        }
        let key = NodeKey::Directory(child.path.clone());
        let id = id_for(key, ids, next_id);
        let is_expanded = expanded.contains(&child.path);
        nodes.push(TreeNode {
            id,
            depth,
            label: Line::from(Span::styled(
                format!("{}/", child.name),
                Style::new()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::DIM | Modifier::BOLD),
            )),
            expandable: true,
            expanded: is_expanded,
        });
        meta.insert(
            id,
            NodeMeta::Directory {
                path: child.path.clone(),
            },
        );
        if is_expanded {
            flatten_collection(child, depth + 1, expanded, open, ids, next_id, nodes, meta);
        }
    }
}

fn id_for(key: NodeKey, ids: &mut HashMap<NodeKey, NodeId>, next_id: &mut NodeId) -> NodeId {
    if let Some(id) = ids.get(&key) {
        return *id;
    }
    let id = *next_id;
    *next_id = (*next_id).saturating_add(1);
    ids.insert(key, id);
    id
}

fn request_label(method: HttpMethod, name: &str, open: bool) -> Line<'static> {
    let color = theme::method_color(method);
    let short = &method.as_str()[..3];
    Line::from(vec![
        Span::raw(if open { ">" } else { " " }),
        Span::styled(short.to_owned(), Style::new().fg(color)),
        Span::raw(" "),
        Span::raw(files::display_name(name).to_owned()),
    ])
}

fn directory_paths(collection: &Collection) -> HashSet<PathBuf> {
    let mut paths = HashSet::new();
    collect_directory_paths(collection, &mut paths);
    paths
}

fn collect_directory_paths(collection: &Collection, paths: &mut HashSet<PathBuf>) {
    for child in collection
        .children
        .iter()
        .filter(|child| has_request_in_subtree(child))
    {
        paths.insert(child.path.clone());
        collect_directory_paths(child, paths);
    }
}

fn collect_base_urls(collection: &Collection) -> Vec<String> {
    let mut values = BTreeSet::new();
    collect_collection_base_urls(collection, &mut values);
    values.into_iter().collect()
}

fn collect_collection_base_urls(collection: &Collection, values: &mut BTreeSet<String>) {
    for request in &collection.requests {
        if let Some(base) = urls::base_url(&request.url) {
            values.insert(base);
        }
    }
    for child in &collection.children {
        collect_collection_base_urls(child, values);
    }
}

fn render_empty(area: Rect, buffer: &mut Buffer) {
    let lines = ["Collection is empty.", "Press ctrl+n to create a request."];
    let height = (lines.len() as u16).min(area.height);
    let y = area.y + area.height.saturating_sub(height) / 2;
    Paragraph::new(
        lines
            .into_iter()
            .map(|line| Line::from(Span::styled(line, theme::placeholder())).centered())
            .collect::<Vec<_>>(),
    )
    .render(Rect::new(area.x, y, area.width, height), buffer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusting_core::RequestModel;

    fn request(
        parent: &Path,
        file: &str,
        method: HttpMethod,
        url: &str,
        description: &str,
    ) -> RequestModel {
        RequestModel {
            name: file.to_owned(),
            description: description.to_owned(),
            method,
            url: url.to_owned(),
            path: Some(parent.join(file)),
            ..RequestModel::default()
        }
    }

    fn collection() -> Collection {
        let root = PathBuf::from("/tmp/apis");
        let child_path = root.join("users");
        Collection {
            path: root.clone(),
            name: "apis".to_owned(),
            requests: vec![request(
                &root,
                "health.posting.yaml",
                HttpMethod::Get,
                "https://api.example.com/health",
                "Checks service health\nand readiness\nwithout mutation\nfourth line",
            )],
            children: vec![Collection {
                path: child_path.clone(),
                name: "users".to_owned(),
                requests: vec![request(
                    &child_path,
                    "create.posting.yaml",
                    HttpMethod::Post,
                    "https://api.example.com/users",
                    "Creates a user",
                )],
                children: Vec::new(),
            }],
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn rendered_text(buffer: &Buffer) -> String {
        (buffer.area.top()..buffer.area.bottom())
            .map(|y| {
                (buffer.area.left()..buffer.area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn tree_labels(pane: &CollectionPane) -> Vec<String> {
        pane.tree
            .nodes()
            .iter()
            .map(|node| {
                node.label
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn flattening_hides_the_root_and_expands_directories_initially() {
        let pane = CollectionPane::new(&collection());
        assert_eq!(pane.tree.nodes().len(), 3);
        assert_eq!(pane.tree.nodes()[0].depth, 0);
        assert!(!pane.tree.nodes()[0].expandable);
        assert_eq!(pane.tree.nodes()[1].depth, 0);
        assert!(pane.tree.nodes()[1].expandable);
        assert!(pane.tree.nodes()[1].expanded);
        assert_eq!(pane.tree.nodes()[2].depth, 1);
    }

    #[test]
    fn flattening_hides_an_asset_only_scripts_sibling() {
        let mut value = collection();
        let scripts_path = value.path.join("scripts");
        value.children.push(Collection {
            path: scripts_path.clone(),
            name: "scripts".to_owned(),
            requests: Vec::new(),
            children: Vec::new(),
        });

        let pane = CollectionPane::new(&value);

        assert_eq!(
            tree_labels(&pane),
            vec![" GET health", "users/", " POS create"]
        );
        assert!(!pane.known_directories.contains(&scripts_path));
        assert!(!pane.expanded.contains(&scripts_path));
    }

    #[test]
    fn flattening_keeps_ancestors_of_a_nested_request_visible() {
        let root = PathBuf::from("/tmp/apis");
        let parent_path = root.join("parent");
        let nested_path = parent_path.join("nested");
        let value = Collection {
            path: root,
            name: "apis".to_owned(),
            requests: Vec::new(),
            children: vec![Collection {
                path: parent_path,
                name: "parent".to_owned(),
                requests: Vec::new(),
                children: vec![Collection {
                    path: nested_path.clone(),
                    name: "nested".to_owned(),
                    requests: vec![request(
                        &nested_path,
                        "details.posting.yaml",
                        HttpMethod::Get,
                        "https://api.example.com/details",
                        "",
                    )],
                    children: Vec::new(),
                }],
            }],
        };

        let pane = CollectionPane::new(&value);

        assert_eq!(
            tree_labels(&pane),
            vec!["parent/", "nested/", " GET details"]
        );
        assert_eq!(
            pane.tree
                .nodes()
                .iter()
                .map(|node| node.depth)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert!(pane.tree.nodes()[0].expanded);
        assert!(pane.tree.nodes()[1].expanded);
    }

    #[test]
    fn request_labels_include_open_marker_method_and_display_name() {
        let value = collection();
        let open = value.requests[0].path.clone();
        let mut pane = CollectionPane::new(&value);
        pane.set_open(open);
        let text = pane.tree.nodes()[0]
            .label
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert_eq!(text, ">GET health");
    }

    #[test]
    fn base_urls_are_unique_sorted_and_ignore_relative_urls() {
        let mut value = collection();
        value.requests.push(request(
            &value.path,
            "other.posting.yaml",
            HttpMethod::Get,
            "http://z.example.test/x",
            "",
        ));
        value.requests.push(request(
            &value.path,
            "relative.posting.yaml",
            HttpMethod::Get,
            "/local",
            "",
        ));
        assert_eq!(
            CollectionPane::new(&value).base_urls(),
            vec!["http://z.example.test", "https://api.example.com"]
        );
    }

    #[test]
    fn enter_opens_a_request_and_new_targets_its_parent() {
        let value = collection();
        let expected = value.requests[0].path.clone().expect("path");
        let mut pane = CollectionPane::new(&value);
        assert_eq!(pane.selected_request(), Some(expected.as_path()));
        assert_eq!(pane.target_directory(), value.path);
        assert_eq!(
            pane.handle_key(key(KeyCode::Enter)),
            CollectionAction::Open(expected)
        );
        assert_eq!(
            pane.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL)),
            CollectionAction::NewRequest {
                parent: PathBuf::from("/tmp/apis")
            }
        );
    }

    #[test]
    fn selecting_a_visible_request_moves_the_tree_cursor() {
        let value = collection();
        let expected = value.children[0].requests[0]
            .path
            .clone()
            .expect("request path");
        let mut pane = CollectionPane::new(&value);

        pane.select_request(&expected);

        assert_eq!(pane.selected_request(), Some(expected.as_path()));
    }

    #[test]
    fn selecting_a_hidden_request_expands_all_ancestors() {
        let root = PathBuf::from("/tmp/apis");
        let parent_path = root.join("parent");
        let nested_path = parent_path.join("nested");
        let expected = nested_path.join("details.posting.yaml");
        let value = Collection {
            path: root,
            name: "apis".to_owned(),
            requests: Vec::new(),
            children: vec![Collection {
                path: parent_path.clone(),
                name: "parent".to_owned(),
                requests: Vec::new(),
                children: vec![Collection {
                    path: nested_path.clone(),
                    name: "nested".to_owned(),
                    requests: vec![request(
                        &nested_path,
                        "details.posting.yaml",
                        HttpMethod::Get,
                        "https://api.example.com/details",
                        "",
                    )],
                    children: Vec::new(),
                }],
            }],
        };
        let mut pane = CollectionPane::new(&value);
        pane.restore_collapsed_directories(&HashSet::from([
            parent_path.clone(),
            nested_path.clone(),
        ]));
        assert_eq!(tree_labels(&pane), vec!["parent/"]);

        pane.select_request(&expected);

        assert_eq!(
            tree_labels(&pane),
            vec!["parent/", "nested/", " GET details"]
        );
        assert!(pane.expanded.contains(&parent_path));
        assert!(pane.expanded.contains(&nested_path));
        assert_eq!(pane.selected_request(), Some(expected.as_path()));
    }

    #[test]
    fn duplicate_delete_and_search_actions_preserve_quick_and_confirm_flags() {
        let value = collection();
        let path = value.requests[0].path.clone().expect("path");
        let mut pane = CollectionPane::new(&value);
        assert_eq!(
            pane.handle_key(key(KeyCode::Char('d'))),
            CollectionAction::Delete {
                path: path.clone(),
                confirm: true
            }
        );
        assert_eq!(
            pane.handle_key(key(KeyCode::Char('D'))),
            CollectionAction::Delete {
                path: path.clone(),
                confirm: false
            }
        );
        assert_eq!(
            pane.handle_key(key(KeyCode::Backspace)),
            CollectionAction::Delete {
                path: path.clone(),
                confirm: true
            }
        );
        assert_eq!(
            pane.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::SHIFT)),
            CollectionAction::Delete {
                path: path.clone(),
                confirm: false
            }
        );
        assert_eq!(
            pane.handle_key(key(KeyCode::Char('y'))),
            CollectionAction::Duplicate {
                path: path.clone(),
                quick: false
            }
        );
        assert_eq!(
            pane.handle_key(key(KeyCode::Char('Y'))),
            CollectionAction::Duplicate { path, quick: true }
        );
        assert_eq!(
            pane.handle_key(key(KeyCode::Char('/'))),
            CollectionAction::SearchRequested
        );
    }

    #[test]
    fn enter_toggles_a_directory_and_preserves_its_selection() {
        let mut pane = CollectionPane::new(&collection());
        let directory = pane.tree.nodes()[1].id;
        pane.tree.set_cursor(directory);
        assert_eq!(
            pane.handle_key(key(KeyCode::Enter)),
            CollectionAction::Consumed
        );
        assert_eq!(pane.tree.nodes().len(), 2);
        assert!(!pane.tree.nodes()[1].expanded);
        assert_eq!(
            pane.handle_key(key(KeyCode::Enter)),
            CollectionAction::Consumed
        );
        assert_eq!(pane.tree.nodes().len(), 3);
        assert_eq!(pane.tree.cursor(), Some(directory));
    }

    #[test]
    fn reload_preserves_cursor_and_collapsed_state() {
        let value = collection();
        let mut pane = CollectionPane::new(&value);
        let directory = pane.tree.nodes()[1].id;
        pane.tree.set_cursor(directory);
        pane.handle_key(key(KeyCode::Char(' ')));
        pane.reload(&value);
        assert_eq!(pane.tree.cursor(), Some(directory));
        assert_eq!(pane.tree.nodes().len(), 2);
    }

    #[test]
    fn restored_state_filters_stale_directories_and_new_directories_expand_on_reload() {
        let mut value = collection();
        let users = value.children[0].path.clone();
        let stale = value.path.join("removed");
        let mut pane = CollectionPane::new(&value);
        pane.restore_collapsed_directories(&HashSet::from([users.clone(), stale]));

        assert_eq!(
            pane.collapsed_directories().collect::<HashSet<_>>(),
            HashSet::from([users.as_path()])
        );
        assert_eq!(pane.tree.nodes().len(), 2);

        let teams = value.path.join("teams");
        value.children.push(Collection {
            path: teams.clone(),
            name: "teams".to_owned(),
            requests: vec![request(
                &teams,
                "list.posting.yaml",
                HttpMethod::Get,
                "https://api.example.com/teams",
                "",
            )],
            children: Vec::new(),
        });
        pane.reload(&value);

        assert!(pane.expanded.contains(&teams));
        assert_eq!(
            pane.collapsed_directories().collect::<HashSet<_>>(),
            HashSet::from([users.as_path()])
        );
    }

    #[test]
    fn selected_directory_is_the_new_request_target() {
        let value = collection();
        let expected = value.children[0].path.clone();
        let mut pane = CollectionPane::new(&value);
        pane.tree.set_cursor(pane.tree.nodes()[1].id);
        assert_eq!(pane.selected_request(), None);
        assert_eq!(pane.target_directory(), expected);
    }

    #[test]
    fn rendering_shows_title_subtitle_tree_and_at_most_three_preview_lines() {
        let mut pane = CollectionPane::new(&collection());
        let area = Rect::new(0, 0, 60, 12);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, false);
        let text = rendered_text(&buffer);
        assert!(text.contains("Collection"));
        assert!(text.contains("apis"));
        assert!(text.contains("GET health"));
        assert!(text.contains("Checks service health"));
        assert!(text.contains("without mutation"));
        assert!(!text.contains("fourth line"));
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| cell.style().bg == Some(ratatui::style::Color::Reset))
        );
    }

    #[test]
    fn an_empty_collection_renders_the_required_empty_state() {
        let mut pane = CollectionPane::new(&Collection::new("/tmp/empty"));
        let area = Rect::new(0, 0, 60, 10);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true);
        let text = rendered_text(&buffer);
        assert!(text.contains("Collection is empty."));
        assert!(text.contains("Press ctrl+n to create a request."));
    }

    #[test]
    fn jump_target_uses_tab_at_the_pane_origin() {
        let mut pane = CollectionPane::new(&collection());
        let area = Rect::new(4, 7, 30, 10);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true);
        assert_eq!(pane.jump_targets(), vec![('\t', Position::new(4, 7))]);
    }
}
