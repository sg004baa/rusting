//! Request pane tab bar and eight request editors.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget as _};
use rusting_core::{KeyValue, RequestModel, ScriptHook, ScriptRef, Variables};

use crate::panes::auth::{AuthAction, AuthTab};
use crate::panes::body::{BodyAction, BodyTab};
use crate::panes::headers::HeadersTab;
use crate::panes::info::{InfoAction, InfoTab};
use crate::panes::key_value::{KeyValueAction, KeyValueField};
use crate::panes::options::{OptionsAction, OptionsTab};
use crate::panes::path::{PathAction, PathTab};
use crate::panes::query::QueryTab;
use crate::panes::scripts::{ScriptsAction, ScriptsTab};
use crate::theme;
use crate::widgets::syntax::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestTab {
    Headers,
    Body,
    Path,
    Query,
    Auth,
    Info,
    Scripts,
    Options,
}

impl RequestTab {
    pub const ALL: [RequestTab; 8] = [
        Self::Headers,
        Self::Body,
        Self::Path,
        Self::Query,
        Self::Auth,
        Self::Info,
        Self::Scripts,
        Self::Options,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Headers => "Headers",
            Self::Body => "Body",
            Self::Path => "Path",
            Self::Query => "Query",
            Self::Auth => "Auth",
            Self::Info => "Info",
            Self::Scripts => "Scripts",
            Self::Options => "Options",
        }
    }

    pub const fn jump_key(self) -> char {
        match self {
            Self::Headers => 'q',
            Self::Body => 'w',
            Self::Path => 'e',
            Self::Query => 'r',
            Self::Auth => 't',
            Self::Info => 'y',
            Self::Scripts => 'u',
            Self::Options => 'i',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyValueEditTarget {
    pub tab: RequestTab,
    pub field: KeyValueField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestPaneAction {
    Ignored,
    Consumed,
    Changed,
    OpenInPager(String, Option<Language>),
    OpenInEditor(String, Option<Language>),
    OpenKeyValueInEditor {
        target: KeyValueEditTarget,
        contents: String,
    },
    OpenPathInEditor(PathBuf),
    OpenPathInPager(PathBuf),
    CopyRequested,
    UrlRewrite(String),
    JumpToUrlParam(String),
    LeaveUp,
    LeaveDown,
}

pub struct RequestPane {
    active: RequestTab,
    tab_bar_focused: bool,
    headers: HeadersTab,
    body: BodyTab,
    path: PathTab,
    query: QueryTab,
    auth: AuthTab,
    info: InfoTab,
    scripts: ScriptsTab,
    options: OptionsTab,
    current_url: String,
    tab_positions: Vec<(char, Position)>,
}

impl RequestPane {
    pub fn new(collection_root: PathBuf) -> Self {
        Self {
            active: RequestTab::Headers,
            tab_bar_focused: true,
            headers: HeadersTab::new(),
            body: BodyTab::new(),
            path: PathTab::new(),
            query: QueryTab::new(),
            auth: AuthTab::new(),
            info: InfoTab::new(),
            scripts: ScriptsTab::new(collection_root),
            options: OptionsTab::new(),
            current_url: String::new(),
            tab_positions: Vec::new(),
        }
    }

    pub fn active_tab(&self) -> RequestTab {
        self.active
    }

    pub fn set_active_tab(&mut self, tab: RequestTab) {
        self.active = tab;
    }

    pub fn tab_bar_focused(&self) -> bool {
        self.tab_bar_focused
    }

    pub fn focus_tab_bar(&mut self) {
        self.tab_bar_focused = true;
    }

    pub fn focus_body(&mut self) {
        if self.active == RequestTab::Path && !self.path.has_content() {
            self.tab_bar_focused = true;
            return;
        }
        self.tab_bar_focused = false;
        match self.active {
            RequestTab::Headers => self.headers.focus_first_control(),
            RequestTab::Body => self.body.focus_first_control(),
            RequestTab::Path => self.path.focus_first_control(),
            RequestTab::Query => self.query.focus_first_control(),
            RequestTab::Auth => self.auth.focus_first_control(),
            RequestTab::Info => self.info.focus_first_control(),
            RequestTab::Scripts => self.scripts.focus_first_control(),
            RequestTab::Options => self.options.focus_first_control(),
        }
    }

    pub fn focus_last_control(&mut self) {
        if self.active == RequestTab::Path && !self.path.has_content() {
            self.tab_bar_focused = true;
            return;
        }
        self.tab_bar_focused = false;
        match self.active {
            RequestTab::Headers => self.headers.focus_last_control(),
            RequestTab::Body => self.body.focus_last_control(),
            RequestTab::Path => self.path.focus_last_control(),
            RequestTab::Query => self.query.focus_last_control(),
            RequestTab::Auth => self.auth.focus_last_control(),
            RequestTab::Info => self.info.focus_last_control(),
            RequestTab::Scripts => self.scripts.focus_last_control(),
            RequestTab::Options => self.options.focus_last_control(),
        }
    }

    pub fn load(&mut self, request: &RequestModel) {
        self.headers.load(request);
        self.body.load(request);
        self.path.load(request);
        self.query.load(request);
        self.auth.load(request);
        self.info.load(request);
        self.scripts.load(request);
        self.options.load(request);
        self.current_url.clone_from(&request.url);
        self.tab_bar_focused = true;
    }

    pub fn to_model(&self, base: &RequestModel) -> Result<RequestModel, String> {
        let mut request = base.clone();
        request.headers = self.headers.to_model();
        request.body = self.body.to_model();
        request.path_params = self.path.to_model();
        request.params = self.query.to_model();
        request.auth = self.auth.to_model();
        let (name, description) = self.info.to_model();
        request.name = name;
        request.description = description;
        request.scripts = self.scripts.to_model();
        request.options = self.options.to_model()?;
        Ok(request)
    }

    pub fn configured_script_hooks(&self) -> Vec<(ScriptHook, String, ScriptRef)> {
        self.scripts.configured_hooks()
    }

    pub fn refresh_script_candidates(&mut self) {
        self.scripts.refresh_candidates();
    }

    pub fn sync_path_params_from_url(&mut self, url: &str) {
        self.current_url.clear();
        self.current_url.push_str(url);
        self.path.sync_from_url(url);
    }

    pub fn is_editing(&self) -> bool {
        match self.active {
            RequestTab::Headers => self.headers.is_editing(),
            RequestTab::Body => self.body.is_editing(),
            RequestTab::Path => self.path.is_editing(),
            RequestTab::Query => self.query.is_editing(),
            RequestTab::Auth | RequestTab::Info | RequestTab::Scripts | RequestTab::Options => {
                false
            }
        }
    }

    /// Applies text returned by `$EDITOR` to the active multiline editor.
    pub fn apply_external_edit(&mut self, text: &str) -> Result<(), String> {
        match self.active {
            RequestTab::Body if self.body.apply_external_edit(text) => Ok(()),
            RequestTab::Info => {
                self.info.apply_external_edit(text);
                Ok(())
            }
            RequestTab::Body => {
                Err("The current Body mode does not contain a text editor.".to_owned())
            }
            tab => Err(format!(
                "The {} tab does not contain an editable text editor.",
                tab.label()
            )),
        }
    }

    /// Applies `$EDITOR` output to the exact key/value draft that requested it.
    pub fn apply_key_value_external_edit(
        &mut self,
        target: KeyValueEditTarget,
        text: &str,
    ) -> Result<(), String> {
        match target.tab {
            RequestTab::Headers => self.headers.apply_external_edit(target.field, text),
            RequestTab::Body => self.body.apply_key_value_external_edit(target.field, text),
            RequestTab::Path => self.path.apply_external_edit(target.field, text),
            RequestTab::Query => self.query.apply_external_edit(target.field, text),
            tab => Err(format!(
                "The {} tab does not contain editable key/value fields.",
                tab.label()
            )),
        }
    }

    /// Selected row for the key/value copy modal, when the active tab has one.
    pub fn copy_target(&self) -> Option<KeyValue> {
        match self.active {
            RequestTab::Headers => self.headers.selected().cloned(),
            RequestTab::Body => self.body.selected_form_row().cloned(),
            RequestTab::Path => self.path.selected().cloned(),
            RequestTab::Query => self.query.selected().cloned(),
            RequestTab::Auth | RequestTab::Info | RequestTab::Scripts | RequestTab::Options => None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, variables: &Variables) -> RequestPaneAction {
        let key = if key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT) {
            KeyEvent {
                code: KeyCode::BackTab,
                ..key
            }
        } else {
            key
        };

        if self.tab_bar_focused {
            return self.handle_tab_key(key);
        }
        match self.active {
            RequestTab::Headers => {
                let action = self.headers.handle_key(key, variables);
                map_key_value(action, self)
            }
            RequestTab::Body => {
                let action = self.body.handle_key(key, variables);
                self.map_body(action)
            }
            RequestTab::Path => {
                let action = self.path.handle_key(key, variables);
                self.map_path(action)
            }
            RequestTab::Query => {
                let action = self.query.handle_key(key, variables);
                map_key_value(action, self)
            }
            RequestTab::Auth => {
                let action = self.auth.handle_key(key, variables);
                map_simple_auth(action, self)
            }
            RequestTab::Info => {
                let action = self.info.handle_key(key);
                self.map_info(action)
            }
            RequestTab::Scripts => {
                let action = self.scripts.handle_key(key);
                self.map_scripts(action)
            }
            RequestTab::Options => {
                let action = self.options.handle_key(key, variables);
                map_simple_options(action, self)
            }
        }
    }

    fn handle_tab_key(&mut self, key: KeyEvent) -> RequestPaneAction {
        let index = RequestTab::ALL
            .iter()
            .position(|tab| *tab == self.active)
            .unwrap_or(0);
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                self.active =
                    RequestTab::ALL[(index + RequestTab::ALL.len() - 1) % RequestTab::ALL.len()];
                RequestPaneAction::Consumed
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.active = RequestTab::ALL[(index + 1) % RequestTab::ALL.len()];
                RequestPaneAction::Consumed
            }
            KeyCode::Tab if self.active == RequestTab::Path && !self.path.has_content() => {
                RequestPaneAction::LeaveDown
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter
                if self.active == RequestTab::Path && !self.path.has_content() =>
            {
                RequestPaneAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter | KeyCode::Tab => {
                self.focus_body();
                RequestPaneAction::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => RequestPaneAction::LeaveUp,
            _ => RequestPaneAction::Ignored,
        }
    }

    fn map_body(&mut self, action: BodyAction) -> RequestPaneAction {
        match action {
            BodyAction::Ignored => RequestPaneAction::Ignored,
            BodyAction::Consumed => RequestPaneAction::Consumed,
            BodyAction::Changed => RequestPaneAction::Changed,
            BodyAction::OpenInPager => {
                RequestPaneAction::OpenInPager(self.body.raw_text(), self.body.raw_language())
            }
            BodyAction::OpenInEditor => {
                RequestPaneAction::OpenInEditor(self.body.raw_text(), self.body.raw_language())
            }
            BodyAction::OpenKeyValueInEditor { field, contents } => {
                RequestPaneAction::OpenKeyValueInEditor {
                    target: KeyValueEditTarget {
                        tab: RequestTab::Body,
                        field,
                    },
                    contents,
                }
            }
            BodyAction::CopyRequested => RequestPaneAction::CopyRequested,
            BodyAction::LeaveUp => {
                self.tab_bar_focused = true;
                RequestPaneAction::Consumed
            }
            BodyAction::LeaveDown => RequestPaneAction::LeaveDown,
        }
    }

    fn map_path(&mut self, action: PathAction) -> RequestPaneAction {
        match action {
            PathAction::Ignored => RequestPaneAction::Ignored,
            PathAction::Consumed => RequestPaneAction::Consumed,
            PathAction::Changed => RequestPaneAction::Changed,
            PathAction::OpenInEditor { field, contents } => {
                RequestPaneAction::OpenKeyValueInEditor {
                    target: KeyValueEditTarget {
                        tab: RequestTab::Path,
                        field,
                    },
                    contents,
                }
            }
            PathAction::Renamed { old, new } => {
                let rewritten = rewrite_path_parameter(&self.current_url, &old, &new);
                self.current_url.clone_from(&rewritten);
                RequestPaneAction::UrlRewrite(rewritten)
            }
            PathAction::JumpToUrl(name) => RequestPaneAction::JumpToUrlParam(name),
            PathAction::LeaveUp => {
                self.tab_bar_focused = true;
                RequestPaneAction::Consumed
            }
            PathAction::LeaveDown => RequestPaneAction::LeaveDown,
        }
    }

    fn map_info(&mut self, action: InfoAction) -> RequestPaneAction {
        match action {
            InfoAction::Ignored => RequestPaneAction::Ignored,
            InfoAction::Consumed => RequestPaneAction::Consumed,
            InfoAction::Changed => RequestPaneAction::Changed,
            InfoAction::OpenInPager => {
                RequestPaneAction::OpenInPager(self.info.editor_text(), None)
            }
            InfoAction::OpenInEditor => {
                RequestPaneAction::OpenInEditor(self.info.editor_text(), None)
            }
            InfoAction::LeaveUp => {
                self.tab_bar_focused = true;
                RequestPaneAction::Consumed
            }
            InfoAction::LeaveDown => RequestPaneAction::LeaveDown,
        }
    }

    fn map_scripts(&mut self, action: ScriptsAction) -> RequestPaneAction {
        match action {
            ScriptsAction::Ignored => RequestPaneAction::Ignored,
            ScriptsAction::Consumed => RequestPaneAction::Consumed,
            ScriptsAction::Changed => RequestPaneAction::Changed,
            ScriptsAction::OpenInEditor(path) => RequestPaneAction::OpenPathInEditor(path),
            ScriptsAction::OpenInPager(path) => RequestPaneAction::OpenPathInPager(path),
            ScriptsAction::LeaveUp => {
                self.tab_bar_focused = true;
                RequestPaneAction::Consumed
            }
            ScriptsAction::LeaveDown => RequestPaneAction::LeaveDown,
        }
    }

    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        focused: bool,
        variables: &Variables,
    ) {
        if area.is_empty() {
            return;
        }
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::border(focused))
            .title(
                Line::from(" Request ")
                    .style(theme::border_title(focused))
                    .right_aligned(),
            );
        let inner = block.inner(area);
        block.render(area, buffer);
        let [tabs_area, content] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        self.render_tabs(tabs_area, buffer, focused && self.tab_bar_focused);
        let content_focused = focused && !self.tab_bar_focused;
        match self.active {
            RequestTab::Headers => self
                .headers
                .render(content, buffer, content_focused, variables),
            RequestTab::Body => self
                .body
                .render(content, buffer, content_focused, variables),
            RequestTab::Path => self
                .path
                .render(content, buffer, content_focused, variables),
            RequestTab::Query => self
                .query
                .render(content, buffer, content_focused, variables),
            RequestTab::Auth => self
                .auth
                .render(content, buffer, content_focused, variables),
            RequestTab::Info => self.info.render(content, buffer, content_focused),
            RequestTab::Scripts => self.scripts.render(content, buffer, content_focused),
            RequestTab::Options => self
                .options
                .render(content, buffer, content_focused, variables),
        }
    }

    fn render_tabs(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        self.tab_positions.clear();
        let mut x = area.x;
        let right = area.x.saturating_add(area.width);
        for tab in RequestTab::ALL {
            if x >= right {
                break;
            }
            self.tab_positions
                .push((tab.jump_key(), Position::new(x, area.y)));
            let active = tab == self.active;
            let mut spans = vec![Span::styled(
                format!(" {}", tab.label()),
                if active {
                    theme::selection()
                } else {
                    Style::new()
                },
            )];
            if self.tab_has_content(tab) {
                spans.push(Span::styled("•", Style::new().fg(theme::ACCENT)));
            }
            spans.push(Span::raw(" "));
            let line = Line::from(spans).style(if active && focused {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            });
            let width = line.width() as u16;
            line.render(Rect::new(x, area.y, width.min(right - x), 1), buffer);
            x = x.saturating_add(width);
        }
    }

    fn tab_has_content(&self, tab: RequestTab) -> bool {
        match tab {
            RequestTab::Headers => self.headers.has_content(),
            RequestTab::Body => self.body.has_content(),
            RequestTab::Path => self.path.has_content(),
            RequestTab::Query => self.query.has_content(),
            RequestTab::Auth | RequestTab::Info | RequestTab::Scripts | RequestTab::Options => {
                false
            }
        }
    }

    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        match self.active {
            RequestTab::Headers => self.headers.render_overlay(screen, buffer),
            RequestTab::Body => self.body.render_overlay(screen, buffer),
            RequestTab::Path => self.path.render_overlay(screen, buffer),
            RequestTab::Query => self.query.render_overlay(screen, buffer),
            RequestTab::Auth => self.auth.render_overlay(screen, buffer),
            RequestTab::Info => self.info.render_overlay(screen, buffer),
            RequestTab::Scripts => self.scripts.render_overlay(screen, buffer),
            RequestTab::Options => self.options.render_overlay(screen, buffer),
        }
    }

    pub fn jump_targets(&self) -> Vec<(char, Position)> {
        self.tab_positions.clone()
    }
}

fn map_key_value(action: KeyValueAction, pane: &mut RequestPane) -> RequestPaneAction {
    match action {
        KeyValueAction::Ignored => RequestPaneAction::Ignored,
        KeyValueAction::Consumed => RequestPaneAction::Consumed,
        KeyValueAction::Changed => RequestPaneAction::Changed,
        KeyValueAction::OpenInEditor { field, contents } => {
            RequestPaneAction::OpenKeyValueInEditor {
                target: KeyValueEditTarget {
                    tab: pane.active,
                    field,
                },
                contents,
            }
        }
        KeyValueAction::CopyRequested => RequestPaneAction::CopyRequested,
        KeyValueAction::LeaveUp => {
            pane.tab_bar_focused = true;
            RequestPaneAction::Consumed
        }
        KeyValueAction::LeaveDown => RequestPaneAction::LeaveDown,
    }
}

fn map_simple_auth(action: AuthAction, pane: &mut RequestPane) -> RequestPaneAction {
    match action {
        AuthAction::Ignored => RequestPaneAction::Ignored,
        AuthAction::Consumed => RequestPaneAction::Consumed,
        AuthAction::Changed => RequestPaneAction::Changed,
        AuthAction::LeaveUp => {
            pane.tab_bar_focused = true;
            RequestPaneAction::Consumed
        }
        AuthAction::LeaveDown => RequestPaneAction::LeaveDown,
    }
}

fn map_simple_options(action: OptionsAction, pane: &mut RequestPane) -> RequestPaneAction {
    match action {
        OptionsAction::Ignored => RequestPaneAction::Ignored,
        OptionsAction::Consumed => RequestPaneAction::Consumed,
        OptionsAction::Changed => RequestPaneAction::Changed,
        OptionsAction::LeaveUp => {
            pane.tab_bar_focused = true;
            RequestPaneAction::Consumed
        }
        OptionsAction::LeaveDown => RequestPaneAction::LeaveDown,
    }
}

fn rewrite_path_parameter(url: &str, old: &str, new: &str) -> String {
    let mut rewritten = url.to_owned();
    for token in rusting_core::urls::find_path_params(url).into_iter().rev() {
        if token.name == old {
            rewritten.replace_range(token.start..token.end, &format!(":{new}"));
        }
    }
    rewritten
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusting_core::{BodyContent, PathParam};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn jump_keys_are_stable_after_render() {
        let mut pane = RequestPane::new(PathBuf::from("."));
        let area = Rect::new(0, 0, 100, 20);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true, &Variables::new());
        assert_eq!(
            pane.jump_targets()
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            vec!['q', 'w', 'e', 'r', 't', 'y', 'u', 'i']
        );
    }

    #[test]
    fn content_tabs_render_bullets_and_model_round_trips() {
        let request = RequestModel {
            headers: vec![KeyValue::new("Accept", "application/json")],
            url: "https://example.test/:id".into(),
            ..RequestModel::default()
        };
        let mut pane = RequestPane::new(PathBuf::from("."));
        pane.load(&request);
        let area = Rect::new(0, 0, 100, 20);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true, &Variables::new());
        let rendered = (0..area.width)
            .map(|x| buffer[(x, 1)].symbol())
            .collect::<String>();
        assert!(rendered.contains('•'));
        assert_eq!(
            pane.to_model(&request).expect("valid model").headers,
            request.headers
        );
    }

    #[test]
    fn headers_tab_enters_key_and_add_tab_leaves_request() {
        let vars = Variables::new();
        let base = RequestModel::default();
        let mut pane = RequestPane::new(PathBuf::from("."));
        assert_eq!(
            pane.handle_key(key(KeyCode::Tab), &vars),
            RequestPaneAction::Consumed
        );
        assert!(!pane.tab_bar_focused());
        for character in "X-Test".chars() {
            pane.handle_key(key(KeyCode::Char(character)), &vars);
        }
        pane.handle_key(key(KeyCode::Tab), &vars);
        for character in "value".chars() {
            pane.handle_key(key(KeyCode::Char(character)), &vars);
        }
        pane.handle_key(key(KeyCode::Tab), &vars);
        assert_eq!(
            pane.handle_key(key(KeyCode::Enter), &vars),
            RequestPaneAction::Changed
        );
        assert_eq!(
            pane.to_model(&base).expect("valid request").headers,
            vec![KeyValue::new("X-Test", "value")]
        );
        pane.handle_key(key(KeyCode::Tab), &vars);
        pane.handle_key(key(KeyCode::Tab), &vars);
        assert_eq!(
            pane.handle_key(key(KeyCode::Tab), &vars),
            RequestPaneAction::LeaveDown
        );
    }

    #[test]
    fn empty_path_tab_skips_body_for_tab_and_keeps_directional_entry_on_tabs() {
        let vars = Variables::new();
        let mut pane = RequestPane::new(PathBuf::from("."));
        pane.set_active_tab(RequestTab::Path);

        for code in [KeyCode::Down, KeyCode::Char('j'), KeyCode::Enter] {
            assert_eq!(
                pane.handle_key(key(code), &vars),
                RequestPaneAction::Consumed,
                "{code:?}"
            );
            assert!(pane.tab_bar_focused(), "{code:?}");
        }
        assert_eq!(
            pane.handle_key(key(KeyCode::Right), &vars),
            RequestPaneAction::Consumed
        );
        assert_eq!(pane.active_tab(), RequestTab::Query);
        assert_eq!(
            pane.handle_key(key(KeyCode::Left), &vars),
            RequestPaneAction::Consumed
        );
        assert_eq!(pane.active_tab(), RequestTab::Path);
        assert_eq!(
            pane.handle_key(key(KeyCode::Tab), &vars),
            RequestPaneAction::LeaveDown
        );
        assert!(pane.tab_bar_focused());
    }

    #[test]
    fn nonempty_path_tab_enters_body_with_forward_keys() {
        let vars = Variables::new();
        for code in [
            KeyCode::Tab,
            KeyCode::Down,
            KeyCode::Char('j'),
            KeyCode::Enter,
        ] {
            let mut pane = RequestPane::new(PathBuf::from("."));
            pane.load(&RequestModel {
                url: "https://example.test/:id".to_owned(),
                ..RequestModel::default()
            });
            pane.set_active_tab(RequestTab::Path);

            assert_eq!(
                pane.handle_key(key(code), &vars),
                RequestPaneAction::Consumed,
                "{code:?}"
            );
            assert!(!pane.tab_bar_focused(), "{code:?}");
        }
    }

    #[test]
    fn every_request_tab_enters_at_its_first_control() {
        let vars = Variables::new();
        for tab in RequestTab::ALL {
            let mut pane = RequestPane::new(PathBuf::from("."));
            if tab == RequestTab::Path {
                pane.load(&RequestModel {
                    url: "https://example.test/:id".to_owned(),
                    ..RequestModel::default()
                });
            }
            pane.set_active_tab(tab);
            pane.focus_body();
            assert_eq!(
                pane.handle_key(key(KeyCode::BackTab), &vars),
                RequestPaneAction::Consumed,
                "{tab:?}"
            );
            assert!(pane.tab_bar_focused(), "{tab:?}");
        }
    }

    #[test]
    fn every_request_tab_has_finite_forward_and_backward_traversal() {
        let vars = Variables::new();
        for tab in RequestTab::ALL {
            let mut pane = RequestPane::new(PathBuf::from("."));
            if tab == RequestTab::Path {
                pane.load(&RequestModel {
                    url: "https://example.test/:id".to_owned(),
                    ..RequestModel::default()
                });
            }
            pane.set_active_tab(tab);
            pane.focus_body();
            let mut left_down = false;
            for _ in 0..8 {
                if pane.handle_key(key(KeyCode::Tab), &vars) == RequestPaneAction::LeaveDown {
                    left_down = true;
                    break;
                }
            }
            assert!(left_down, "{tab:?} swallowed Tab or looped");

            pane.focus_last_control();
            for _ in 0..8 {
                if pane.tab_bar_focused() {
                    break;
                }
                let _ = pane.handle_key(key(KeyCode::BackTab), &vars);
            }
            assert!(
                pane.tab_bar_focused(),
                "{tab:?} swallowed BackTab or looped"
            );
        }
    }

    #[test]
    fn ctrl_e_routes_each_key_value_request_tab_with_its_exact_target() {
        let variables = Variables::new();
        let request = RequestModel {
            headers: vec![KeyValue::new("Header", "header value")],
            body: Some(BodyContent::Form {
                form_data: vec![KeyValue::new("Form", "form value")],
                content_type: Some(BodyContent::FORM_CONTENT_TYPE.to_owned()),
            }),
            url: "https://example.test/:Path".to_owned(),
            path_params: vec![PathParam {
                name: "Path".to_owned(),
                value: "path value".to_owned(),
            }],
            params: vec![KeyValue::new("Query", "query value")],
            ..RequestModel::default()
        };

        for (tab, expected) in [
            (RequestTab::Headers, "Header"),
            (RequestTab::Body, "Form"),
            (RequestTab::Path, "Path"),
            (RequestTab::Query, "Query"),
        ] {
            let mut pane = RequestPane::new(PathBuf::from("."));
            pane.load(&request);
            pane.set_active_tab(tab);
            pane.focus_body();
            match tab {
                RequestTab::Body => {
                    pane.handle_key(key(KeyCode::Tab), &variables);
                    pane.handle_key(key(KeyCode::Enter), &variables);
                }
                RequestTab::Headers | RequestTab::Query => {
                    pane.handle_key(key(KeyCode::Enter), &variables);
                }
                RequestTab::Path => {}
                _ => unreachable!(),
            }

            assert_eq!(
                pane.handle_key(
                    KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
                    &variables,
                ),
                RequestPaneAction::OpenKeyValueInEditor {
                    target: KeyValueEditTarget {
                        tab,
                        field: KeyValueField::Key,
                    },
                    contents: expected.to_owned(),
                }
            );
        }
    }

    #[test]
    fn key_value_external_output_stays_draft_until_commit() {
        let variables = Variables::new();
        let request = RequestModel {
            headers: vec![KeyValue::new("old", "value")],
            ..RequestModel::default()
        };
        let mut pane = RequestPane::new(PathBuf::from("."));
        pane.load(&request);
        pane.focus_body();
        pane.handle_key(key(KeyCode::Enter), &variables);
        let RequestPaneAction::OpenKeyValueInEditor { target, .. } = pane.handle_key(
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
            &variables,
        ) else {
            panic!("key/value editor action");
        };

        pane.set_active_tab(RequestTab::Query);
        pane.apply_key_value_external_edit(target, "edited")
            .unwrap();
        assert_eq!(pane.to_model(&request).unwrap().headers[0].name, "old");

        pane.set_active_tab(RequestTab::Headers);
        assert_eq!(
            pane.handle_key(key(KeyCode::Enter), &variables),
            RequestPaneAction::Changed
        );
        assert_eq!(pane.to_model(&request).unwrap().headers[0].name, "edited");
    }

    #[test]
    fn external_edits_are_applied_or_rejected_explicitly() {
        let mut pane = RequestPane::new(PathBuf::from("."));
        pane.set_active_tab(RequestTab::Options);
        assert!(pane.apply_external_edit("text").is_err());
        pane.set_active_tab(RequestTab::Info);
        pane.apply_external_edit("new description")
            .expect("info editor");
        assert_eq!(pane.info.to_model().1, "new description");
    }

    #[test]
    fn renaming_path_tokens_rewrites_all_matching_tokens() {
        assert_eq!(
            rewrite_path_parameter("/users/:id/posts/:id", "id", "user"),
            "/users/:user/posts/:user"
        );
    }
}
