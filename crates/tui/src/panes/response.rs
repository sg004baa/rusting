//! Six-tab response viewer.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Widget as _};
use rusting_core::{KeyValue, Settings};
use rusting_http::{PhaseOutcome, SentRequest, Timings};
use rusting_script::{HookStatus, LogLine, Stream};

use crate::panes::body::{EditorFooter, FooterAction};
use crate::theme;
use crate::widgets::editor::{Editor, EditorAction};
use crate::widgets::syntax::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseTab {
    Body,
    Headers,
    Cookies,
    Scripts,
    Timings,
    SentRequest,
}

impl ResponseTab {
    pub const ALL: [ResponseTab; 6] = [
        ResponseTab::Body,
        ResponseTab::Headers,
        ResponseTab::Cookies,
        ResponseTab::Scripts,
        ResponseTab::Timings,
        ResponseTab::SentRequest,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            ResponseTab::Body => "Body",
            ResponseTab::Headers => "Headers",
            ResponseTab::Cookies => "Cookies",
            ResponseTab::Scripts => "Scripts",
            ResponseTab::Timings => "Timings",
            ResponseTab::SentRequest => "Sent Request",
        }
    }

    pub const fn jump_key(self) -> char {
        match self {
            ResponseTab::Body => 'a',
            ResponseTab::Headers => 's',
            ResponseTab::Cookies => 'd',
            ResponseTab::Scripts => 'f',
            ResponseTab::Timings => 'g',
            ResponseTab::SentRequest => 'h',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponsePaneAction {
    Ignored,
    Consumed,
    OpenInPager(String, Option<Language>),
    OpenInEditor(String, Option<Language>),
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneFocus {
    Tabs,
    Content,
    Footer,
}

#[derive(Debug, Clone)]
struct ResponseMeta {
    status: u16,
    reason: String,
    size: usize,
    total: Option<std::time::Duration>,
}

pub struct ResponsePane {
    active_tab: ResponseTab,
    focus: PaneFocus,
    meta: Option<ResponseMeta>,
    body: Editor,
    body_footer: EditorFooter,
    headers: Editor,
    cookies: Editor,
    script_statuses: [(&'static str, HookStatus); 3],
    logs: Vec<LogLine>,
    log_scroll: usize,
    timings: Timings,
    sent: Editor,
    tab_positions: Vec<(char, Position)>,
}

impl Default for ResponsePane {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponsePane {
    pub fn new() -> Self {
        let mut body = Editor::new();
        body.set_read_only(true);
        let headers = read_only_viewer();
        let cookies = read_only_viewer();
        let sent = read_only_viewer();

        Self {
            active_tab: ResponseTab::Body,
            focus: PaneFocus::Tabs,
            meta: None,
            body,
            body_footer: EditorFooter::new(),
            headers,
            cookies,
            script_statuses: [
                ("Setup", HookStatus::NotConfigured),
                ("Pre-request", HookStatus::NotConfigured),
                ("Post-response", HookStatus::NotConfigured),
            ],
            logs: Vec::new(),
            log_scroll: 0,
            timings: Timings::default(),
            sent,
            tab_positions: Vec::new(),
        }
    }

    pub fn active_tab(&self) -> ResponseTab {
        self.active_tab
    }

    pub fn set_active_tab(&mut self, tab: ResponseTab) {
        self.active_tab = tab;
    }

    pub fn tab_bar_focused(&self) -> bool {
        self.focus == PaneFocus::Tabs
    }

    pub fn focus_tab_bar(&mut self) {
        self.focus = PaneFocus::Tabs;
    }

    pub fn focus_body(&mut self) {
        self.active_tab = ResponseTab::Body;
        self.focus = PaneFocus::Content;
    }

    pub fn has_response(&self) -> bool {
        self.meta.is_some()
    }

    pub fn set_response(&mut self, response: &rusting_http::Response, settings: &Settings) {
        let raw = response.text();
        let text = if settings.response.prettify_json {
            serde_json::from_slice::<serde_json::Value>(&response.body)
                .and_then(|value| serde_json::to_string_pretty(&value))
                .unwrap_or_else(|_| raw.into_owned())
        } else {
            raw.into_owned()
        };
        self.body.set_text(&text);
        self.body
            .set_language(response.language().and_then(Language::from_name));
        self.body.set_show_line_numbers(!text.is_empty());
        self.body_footer.sync_from_editor(&self.body);

        self.headers.set_text(&format_key_values(&response.headers));
        self.cookies.set_text(&format_key_values(&response.cookies));
        self.timings = response.timings.clone();
        self.sent.set_text(&format_sent_request(&response.sent));
        self.meta = Some(ResponseMeta {
            status: response.status,
            reason: response.reason.clone(),
            size: response.body.len(),
            total: response.timings.total,
        });
    }

    pub fn set_script_output(
        &mut self,
        statuses: [(&'static str, HookStatus); 3],
        logs: &[LogLine],
    ) {
        self.script_statuses = statuses;
        self.logs = logs.to_vec();
        self.log_scroll = 0;
    }

    pub fn set_timings(&mut self, timings: &Timings) {
        self.timings = timings.clone();
        if let Some(meta) = &mut self.meta {
            meta.total = timings.total;
        }
    }

    pub fn clear(&mut self) {
        self.meta = None;
        self.body.set_text("");
        self.body.set_language(None);
        self.body.set_show_line_numbers(false);
        self.body_footer.sync_from_editor(&self.body);
        self.headers.set_text("");
        self.cookies.set_text("");
        self.script_statuses = [
            ("Setup", HookStatus::NotConfigured),
            ("Pre-request", HookStatus::NotConfigured),
            ("Post-response", HookStatus::NotConfigured),
        ];
        self.logs.clear();
        self.log_scroll = 0;
        self.timings = Timings::default();
        self.sent.set_text("");
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ResponsePaneAction {
        if key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
        {
            return ResponsePaneAction::LeaveUp;
        }
        if key.code == KeyCode::Tab {
            return ResponsePaneAction::LeaveDown;
        }

        match self.focus {
            PaneFocus::Tabs => self.handle_tab_key(key),
            PaneFocus::Content => self.handle_content_key(key),
            PaneFocus::Footer => self.handle_footer_key(key),
        }
    }

    fn handle_tab_key(&mut self, key: KeyEvent) -> ResponsePaneAction {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                self.move_tab(-1);
                ResponsePaneAction::Consumed
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.move_tab(1);
                ResponsePaneAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Enter => {
                self.focus = PaneFocus::Content;
                ResponsePaneAction::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => ResponsePaneAction::LeaveUp,
            _ => ResponsePaneAction::Ignored,
        }
    }

    fn move_tab(&mut self, delta: isize) {
        let current = ResponseTab::ALL
            .iter()
            .position(|tab| *tab == self.active_tab)
            .unwrap_or(0) as isize;
        let len = ResponseTab::ALL.len() as isize;
        self.active_tab = ResponseTab::ALL[(current + delta).rem_euclid(len) as usize];
    }

    fn handle_content_key(&mut self, key: KeyEvent) -> ResponsePaneAction {
        match self.active_tab {
            ResponseTab::Body => match self.body.handle_key(key) {
                EditorAction::OpenInPager => {
                    ResponsePaneAction::OpenInPager(self.body.text(), self.body.language())
                }
                EditorAction::OpenInEditor => {
                    ResponsePaneAction::OpenInEditor(self.body.text(), self.body.language())
                }
                EditorAction::LeaveUp => {
                    self.focus = PaneFocus::Tabs;
                    ResponsePaneAction::Consumed
                }
                EditorAction::LeaveDown => {
                    self.focus = PaneFocus::Footer;
                    ResponsePaneAction::Consumed
                }
                EditorAction::Ignored => ResponsePaneAction::Ignored,
                EditorAction::Consumed | EditorAction::Changed => ResponsePaneAction::Consumed,
            },
            ResponseTab::Headers => {
                handle_read_only_editor(&mut self.headers, &mut self.focus, key)
            }
            ResponseTab::Cookies => {
                handle_read_only_editor(&mut self.cookies, &mut self.focus, key)
            }
            ResponseTab::Scripts => self.handle_log_key(key),
            ResponseTab::Timings => self.handle_static_key(key),
            ResponseTab::SentRequest => {
                handle_read_only_editor(&mut self.sent, &mut self.focus, key)
            }
        }
    }

    fn handle_footer_key(&mut self, key: KeyEvent) -> ResponsePaneAction {
        match self.body_footer.handle_key(key, &mut self.body) {
            FooterAction::LeaveUp => {
                self.focus = PaneFocus::Content;
                ResponsePaneAction::Consumed
            }
            FooterAction::LeaveDown => ResponsePaneAction::LeaveDown,
            FooterAction::Ignored => ResponsePaneAction::Ignored,
            FooterAction::Consumed | FooterAction::Changed => ResponsePaneAction::Consumed,
        }
    }

    fn handle_log_key(&mut self, key: KeyEvent) -> ResponsePaneAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if self.log_scroll == 0 => {
                self.focus = PaneFocus::Tabs;
                ResponsePaneAction::Consumed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
                ResponsePaneAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.log_scroll = self
                    .log_scroll
                    .saturating_add(1)
                    .min(self.logs.len().saturating_sub(1));
                ResponsePaneAction::Consumed
            }
            _ => ResponsePaneAction::Ignored,
        }
    }

    fn handle_static_key(&mut self, key: KeyEvent) -> ResponsePaneAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.focus = PaneFocus::Tabs;
                ResponsePaneAction::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') => ResponsePaneAction::Consumed,
            _ => ResponsePaneAction::Ignored,
        }
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, focused: bool, settings: &Settings) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::border(focused))
            .title(self.title_line(focused));
        if settings.response.show_size_and_time
            && let Some(subtitle) = self.subtitle_line()
        {
            block = block.title_bottom(subtitle);
        }
        let inner = block.inner(area);
        block.render(area, buffer);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let tabs = Rect::new(inner.x, inner.y, inner.width, 1);
        self.render_tabs(tabs, buffer, focused && self.focus == PaneFocus::Tabs);
        let content = Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            inner.height.saturating_sub(1),
        );
        let content_focused = focused && self.focus == PaneFocus::Content;
        match self.active_tab {
            ResponseTab::Body => self.render_body(content, buffer, content_focused),
            ResponseTab::Headers => render_read_only_editor(
                &mut self.headers,
                "No headers",
                content,
                buffer,
                content_focused,
            ),
            ResponseTab::Cookies => render_read_only_editor(
                &mut self.cookies,
                "No cookies",
                content,
                buffer,
                content_focused,
            ),
            ResponseTab::Scripts => self.render_scripts(content, buffer),
            ResponseTab::Timings => self.render_timings(content, buffer),
            ResponseTab::SentRequest => {
                if self.has_response() {
                    self.sent.render(content, buffer, content_focused);
                } else {
                    render_centered(
                        "Send a request to view the final sent request.",
                        content,
                        buffer,
                    );
                }
            }
        }
    }

    fn title_line(&self, focused: bool) -> Line<'static> {
        let mut spans = vec![Span::styled("Response", theme::border_title(focused))];
        if let Some(meta) = &self.meta {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("{} {}", meta.status, meta.reason),
                Style::new()
                    .fg(theme::status_color(meta.status))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        Line::from(spans).right_aligned()
    }

    fn subtitle_line(&self) -> Option<Line<'static>> {
        let meta = self.meta.as_ref()?;
        let total = meta.total?;
        Some(
            Line::from(Span::styled(
                format!(
                    "{} in {:.2}ms",
                    human_size(meta.size),
                    total.as_secs_f64() * 1000.0
                ),
                Style::new().fg(theme::MUTED),
            ))
            .right_aligned(),
        )
    }

    fn render_tabs(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        self.tab_positions.clear();
        let mut spans = Vec::with_capacity(ResponseTab::ALL.len());
        let mut x = area.x;
        for tab in ResponseTab::ALL {
            let text = format!(" {} ", tab.label());
            let target_x = x.min(area.right().saturating_sub(1));
            self.tab_positions
                .push((tab.jump_key(), Position::new(target_x, area.y)));
            x = x.saturating_add(text.len() as u16);
            let style = if tab == self.active_tab {
                if focused {
                    theme::selection()
                } else {
                    Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
                }
            } else {
                Style::new().fg(theme::MUTED)
            };
            spans.push(Span::styled(text, style));
        }
        Line::from(spans).render(area, buffer);
    }

    fn render_body(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        if area.height == 0 {
            return;
        }
        let footer_height = EditorFooter::HEIGHT.min(area.height);
        let editor_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(footer_height),
        );
        let footer_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(footer_height),
            area.width,
            footer_height,
        );
        if self.body.is_empty() {
            render_centered("No response body", editor_area, buffer);
        } else {
            self.body.render(editor_area, buffer, focused);
        }
        self.body_footer.render(
            footer_area,
            buffer,
            self.focus == PaneFocus::Footer,
            &self.body,
        );
    }

    fn render_scripts(&self, area: Rect, buffer: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        let column_width = area.width / 3;
        for (index, (label, status)) in self.script_statuses.iter().enumerate() {
            let x = area.x + column_width.saturating_mul(index as u16);
            let width = if index == 2 {
                area.x + area.width - x
            } else {
                column_width
            };
            Line::from(Span::styled(*label, Style::new().fg(theme::MUTED)))
                .centered()
                .render(Rect::new(x, area.y, width, 1), buffer);
            if area.height > 1 {
                let (text, style) = hook_status(status);
                Line::from(Span::styled(text, style))
                    .centered()
                    .render(Rect::new(x, area.y + 1, width, 1), buffer);
            }
        }
        if area.height <= 2 {
            return;
        }
        let log_area = Rect::new(area.x, area.y + 2, area.width, area.height - 2);
        let lines = self.logs.iter().skip(self.log_scroll).map(|line| {
            let (prefix, style) = match line.stream {
                Stream::Out => (" out ", Style::new()),
                Stream::Err => (" err ", Style::new().fg(theme::ERROR)),
            };
            Line::from(vec![
                Span::styled(
                    prefix,
                    Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM),
                ),
                Span::styled(line.text.as_str(), style),
            ])
        });
        Paragraph::new(lines.collect::<Vec<_>>()).render(log_area, buffer);
    }

    fn render_timings(&self, area: Rect, buffer: &mut Buffer) {
        if self.timings.is_empty() {
            render_centered("Send a request to view timings.", area, buffer);
            return;
        }
        let mut lines = self
            .timings
            .iter()
            .map(|(phase, outcome)| {
                let (value, style) = match outcome {
                    PhaseOutcome::Completed(duration) => (
                        format!("{:.2}ms", duration.as_secs_f64() * 1000.0),
                        Style::new().fg(theme::SUCCESS),
                    ),
                    PhaseOutcome::Started => {
                        ("waiting".to_owned(), Style::new().fg(theme::WARNING))
                    }
                    PhaseOutcome::Failed => ("failed".to_owned(), Style::new().fg(theme::ERROR)),
                    PhaseOutcome::Skipped => (
                        "-".to_owned(),
                        Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM),
                    ),
                };
                Line::from(vec![
                    Span::styled(
                        format!("{}: ", phase.label()),
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(value, style),
                ])
            })
            .collect::<Vec<_>>();
        if let Some(total) = self.timings.total {
            lines.push(Line::from(vec![
                Span::styled("Total: ", Style::new().add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("{:.2}ms", total.as_secs_f64() * 1000.0),
                    Style::new().fg(theme::SUCCESS),
                ),
            ]));
        }
        Paragraph::new(lines).render(area, buffer);
    }

    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        if self.active_tab == ResponseTab::Body {
            self.body_footer.render_overlay(screen, buffer);
        }
    }

    pub fn jump_targets(&self) -> Vec<(char, Position)> {
        self.tab_positions.clone()
    }
}

fn read_only_viewer() -> Editor {
    let mut editor = Editor::new();
    editor.set_read_only(true);
    editor.set_language(None);
    editor.set_soft_wrap(true);
    editor.set_show_line_numbers(false);
    editor
}

fn handle_read_only_editor(
    editor: &mut Editor,
    focus: &mut PaneFocus,
    key: KeyEvent,
) -> ResponsePaneAction {
    match editor.handle_key(key) {
        EditorAction::OpenInPager => ResponsePaneAction::OpenInPager(editor.text(), None),
        EditorAction::OpenInEditor => ResponsePaneAction::OpenInEditor(editor.text(), None),
        EditorAction::LeaveUp => {
            *focus = PaneFocus::Tabs;
            ResponsePaneAction::Consumed
        }
        EditorAction::LeaveDown => ResponsePaneAction::LeaveDown,
        EditorAction::Ignored => ResponsePaneAction::Ignored,
        EditorAction::Consumed | EditorAction::Changed => ResponsePaneAction::Consumed,
    }
}

fn render_read_only_editor(
    editor: &mut Editor,
    empty_message: &str,
    area: Rect,
    buffer: &mut Buffer,
    focused: bool,
) {
    if editor.is_empty() {
        render_centered(empty_message, area, buffer);
    } else {
        editor.render(area, buffer, focused);
    }
}

fn hook_status(status: &HookStatus) -> (&'static str, Style) {
    match status {
        HookStatus::NotConfigured => (
            "-",
            Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM),
        ),
        HookStatus::Success => ("Success ✔︎", Style::new().fg(theme::SUCCESS)),
        HookStatus::Error(_) => ("Error ⨯", Style::new().fg(theme::ERROR)),
    }
}

fn format_key_values(rows: &[KeyValue]) -> String {
    let capacity = rows
        .iter()
        .map(|row| row.name.len() + row.value.len() + 2)
        .sum::<usize>()
        + rows.len().saturating_sub(1);
    let mut text = String::with_capacity(capacity);
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        text.push_str(&row.name);
        text.push_str(": ");
        text.push_str(&row.value);
    }
    text
}

fn format_sent_request(request: &SentRequest) -> String {
    let headers = if request.headers.is_empty() {
        "(no headers)".to_owned()
    } else {
        format_key_values(&request.headers)
    };
    let body = request.body.as_deref().unwrap_or("(empty body)");
    format!(
        "{} {}\n\nHeaders\n{}\n\nBody\n{}",
        request.method, request.url, headers, body
    )
}

fn human_size(bytes: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.2} {}", UNITS[unit])
    }
}

fn render_centered(text: &str, area: Rect, buffer: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    Line::from(Span::styled(text, theme::placeholder()))
        .centered()
        .render(
            Rect::new(area.x, area.y + area.height / 2, area.width, 1),
            buffer,
        );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use rusting_http::{Phase, Response};

    fn response(body: &[u8], content_type: Option<&str>) -> Response {
        Response {
            status: 201,
            reason: "Created".to_owned(),
            url: "https://example.test/items".to_owned(),
            headers: content_type
                .map(|value| vec![KeyValue::new("Content-Type", value)])
                .unwrap_or_default(),
            cookies: vec![KeyValue::new("sid", "abc")],
            body: body.to_vec(),
            timings: Timings::default(),
            sent: SentRequest {
                method: "POST".to_owned(),
                url: "https://example.test/items?q=1".to_owned(),
                headers: vec![KeyValue::new("X-Test", "yes")],
                body: Some("payload".to_owned()),
            },
        }
    }

    use rusting_core::KeyValue;
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

    #[test]
    fn tabs_have_the_specified_labels_and_jump_keys() {
        assert_eq!(
            ResponseTab::ALL.map(ResponseTab::label),
            [
                "Body",
                "Headers",
                "Cookies",
                "Scripts",
                "Timings",
                "Sent Request"
            ]
        );
        assert_eq!(
            ResponseTab::ALL.map(ResponseTab::jump_key),
            ['a', 's', 'd', 'f', 'g', 'h']
        );
    }

    #[test]
    fn valid_json_is_prettified_and_invalid_json_is_preserved() {
        let mut pane = ResponsePane::new();
        pane.set_response(
            &response(br#"{"a":1}"#, Some("application/json")),
            &Settings::default(),
        );
        assert_eq!(pane.body.text(), "{\n  \"a\": 1\n}");

        pane.set_response(
            &response(b"{not-json", Some("application/json")),
            &Settings::default(),
        );
        assert_eq!(pane.body.text(), "{not-json");
    }

    #[test]
    fn prettification_can_be_disabled() {
        let mut settings = Settings::default();
        settings.response.prettify_json = false;
        let mut pane = ResponsePane::new();
        pane.set_response(
            &response(br#"{"a":1}"#, Some("application/json")),
            &settings,
        );
        assert_eq!(pane.body.text(), r#"{"a":1}"#);
    }

    #[test]
    fn response_populates_headers_cookies_language_and_sent_request() {
        let mut response = response(b"ok", Some("text/html"));
        response
            .headers
            .push(KeyValue::new("X-Second", "exact value"));
        response.cookies.push(KeyValue::new("theme", "light mode"));
        let mut pane = ResponsePane::new();
        pane.set_response(&response, &Settings::default());
        assert!(pane.has_response());
        assert_eq!(
            pane.headers.text(),
            "Content-Type: text/html\nX-Second: exact value"
        );
        assert_eq!(pane.cookies.text(), "sid: abc\ntheme: light mode");
        assert_eq!(pane.body.language(), Some(Language::Html));
        let sent = pane.sent.text();
        assert!(sent.starts_with("POST https://example.test/items?q=1"));
        assert!(sent.contains("Headers\nX-Test: yes"));
        assert!(sent.ends_with("Body\npayload"));
    }

    #[test]
    fn header_and_cookie_editors_soft_wrap_long_rows_in_narrow_content() {
        let value = "0123456789abcdefghijkl";
        let mut response = response(b"ok", None);
        response.headers = vec![KeyValue::new("X-Long", value)];
        response.cookies = vec![KeyValue::new("Cookie", value)];
        let mut pane = ResponsePane::new();
        pane.set_response(&response, &Settings::default());
        let area = Rect::new(0, 0, 18, 7);

        for (tab, first_row) in [
            (ResponseTab::Headers, "X-Long: 01234567"),
            (ResponseTab::Cookies, "Cookie: 01234567"),
        ] {
            pane.set_active_tab(tab);
            let editor = match tab {
                ResponseTab::Headers => &pane.headers,
                ResponseTab::Cookies => &pane.cookies,
                _ => unreachable!(),
            };
            assert!(editor.soft_wrap());
            assert!(editor.read_only());
            assert!(!editor.show_line_numbers());

            let mut buffer = Buffer::empty(area);
            pane.render(area, &mut buffer, true, &Settings::default());
            let first = (1..17).map(|x| buffer[(x, 2)].symbol()).collect::<String>();
            let second = (1..17).map(|x| buffer[(x, 3)].symbol()).collect::<String>();
            assert_eq!(first, first_row);
            assert_eq!(second.trim_end(), "89abcdefghijkl");
        }
    }

    #[test]
    fn header_and_cookie_editors_support_visual_yank_and_external_actions() {
        let mut pane = ResponsePane::new();
        pane.set_response(&response(b"ok", Some("text/plain")), &Settings::default());
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        for (tab, text, first_character) in [
            (ResponseTab::Headers, "Content-Type: text/plain", "C"),
            (ResponseTab::Cookies, "sid: abc", "s"),
        ] {
            pane.set_active_tab(tab);
            pane.focus_tab_bar();
            assert_eq!(
                pane.handle_key(key(KeyCode::Down)),
                ResponsePaneAction::Consumed
            );
            assert_eq!(
                pane.handle_key(key(KeyCode::Char('v'))),
                ResponsePaneAction::Consumed
            );
            assert!(match tab {
                ResponseTab::Headers => pane.headers.visual_mode(),
                ResponseTab::Cookies => pane.cookies.visual_mode(),
                _ => unreachable!(),
            });
            assert_eq!(
                pane.handle_key(key(KeyCode::Right)),
                ResponsePaneAction::Consumed
            );
            assert_eq!(
                match tab {
                    ResponseTab::Headers => pane.headers.selected_text(),
                    ResponseTab::Cookies => pane.cookies.selected_text(),
                    _ => unreachable!(),
                }
                .as_deref(),
                Some(first_character)
            );
            assert_eq!(
                pane.handle_key(key(KeyCode::Char('y'))),
                ResponsePaneAction::Consumed
            );
            assert!(!match tab {
                ResponseTab::Headers => pane.headers.visual_mode(),
                ResponseTab::Cookies => pane.cookies.visual_mode(),
                _ => unreachable!(),
            });
            assert_eq!(
                pane.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT)),
                ResponsePaneAction::OpenInPager(text.to_owned(), None)
            );
            assert_eq!(
                pane.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL,)),
                ResponsePaneAction::OpenInEditor(text.to_owned(), None)
            );
        }
    }

    #[test]
    fn header_and_cookie_editor_edges_return_to_tabs_and_leave_the_pane() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        for tab in [ResponseTab::Headers, ResponseTab::Cookies] {
            let mut pane = ResponsePane::new();
            pane.set_response(&response(b"ok", Some("text/plain")), &Settings::default());
            pane.set_active_tab(tab);

            for (enter, leave) in [
                (KeyCode::Down, KeyCode::Up),
                (KeyCode::Char('j'), KeyCode::Char('k')),
                (KeyCode::Enter, KeyCode::Up),
            ] {
                assert!(pane.tab_bar_focused());
                assert_eq!(pane.handle_key(key(enter)), ResponsePaneAction::Consumed);
                assert!(!pane.tab_bar_focused());
                assert_eq!(pane.handle_key(key(leave)), ResponsePaneAction::Consumed);
                assert!(pane.tab_bar_focused());
            }

            assert_eq!(
                pane.handle_key(key(KeyCode::Down)),
                ResponsePaneAction::Consumed
            );
            assert_eq!(
                pane.handle_key(key(KeyCode::Down)),
                ResponsePaneAction::LeaveDown
            );
        }
    }

    #[test]
    fn sent_request_formats_empty_headers_and_body_explicitly() {
        let text = format_sent_request(&SentRequest {
            method: "GET".to_owned(),
            url: "https://x".to_owned(),
            headers: Vec::new(),
            body: None,
        });
        assert!(text.contains("Headers\n(no headers)"));
        assert!(text.contains("Body\n(empty body)"));
    }

    #[test]
    fn script_statuses_and_streams_use_the_required_text_and_colours() {
        let mut pane = ResponsePane::new();
        pane.set_active_tab(ResponseTab::Scripts);
        pane.set_script_output(
            [
                ("Setup", HookStatus::Success),
                ("Pre-request", HookStatus::Error("boom".to_owned())),
                ("Post-response", HookStatus::NotConfigured),
            ],
            &[
                LogLine {
                    stream: Stream::Out,
                    text: "hello".to_owned(),
                },
                LogLine {
                    stream: Stream::Err,
                    text: "bad".to_owned(),
                },
            ],
        );
        let area = Rect::new(0, 0, 70, 12);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true, &Settings::default());
        let text = rendered_text(&buffer);
        assert!(text.contains("Success ✔︎"));
        assert!(text.contains("Error ⨯"));
        assert!(text.contains(" out hello"));
        assert!(text.contains(" err bad"));
        let err = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "b")
            .expect("err log");
        assert_ne!(err.style().bg, theme::cursor().bg);
    }

    #[test]
    fn timings_render_all_outcomes_and_total() {
        let mut timings = Timings::default();
        timings.set(
            Phase::Dns,
            PhaseOutcome::Completed(Duration::from_micros(12_340)),
        );
        timings.set(Phase::Connect, PhaseOutcome::Started);
        timings.set(Phase::Tls, PhaseOutcome::Failed);
        timings.total = Some(Duration::from_millis(20));
        let mut pane = ResponsePane::new();
        pane.set_active_tab(ResponseTab::Timings);
        pane.set_timings(&timings);
        let area = Rect::new(0, 0, 60, 12);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true, &Settings::default());
        let text = rendered_text(&buffer);
        assert!(text.contains("DNS: 12.34ms"));
        assert!(text.contains("Connect: waiting"));
        assert!(text.contains("TLS: failed"));
        assert!(text.contains("Download: -"));
        assert!(text.contains("Total: 20.00ms"));
    }

    #[test]
    fn empty_response_tabs_explain_their_state() {
        let mut pane = ResponsePane::new();
        let area = Rect::new(0, 0, 70, 12);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true, &Settings::default());
        assert!(rendered_text(&buffer).contains("No response body"));

        for (tab, message) in [
            (ResponseTab::Headers, "No headers"),
            (ResponseTab::Cookies, "No cookies"),
        ] {
            pane.set_active_tab(tab);
            let mut buffer = Buffer::empty(area);
            pane.render(area, &mut buffer, true, &Settings::default());
            assert!(rendered_text(&buffer).contains(message));
        }

        pane.set_active_tab(ResponseTab::Timings);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true, &Settings::default());
        assert!(rendered_text(&buffer).contains("Send a request to view timings."));

        pane.set_active_tab(ResponseTab::SentRequest);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true, &Settings::default());
        assert!(rendered_text(&buffer).contains("Send a request to view the final sent request."));
    }

    #[test]
    fn clear_removes_all_response_state() {
        let mut pane = ResponsePane::new();
        pane.set_response(&response(b"body", None), &Settings::default());
        pane.clear();
        assert!(!pane.has_response());
        assert!(pane.body.is_empty());
        assert!(pane.headers.is_empty());
        assert!(pane.cookies.is_empty());
        assert!(pane.timings.is_empty());
        assert!(pane.sent.is_empty());
    }

    #[test]
    fn tab_bar_navigation_wraps_and_enters_content() {
        let mut pane = ResponsePane::new();
        assert_eq!(
            pane.handle_key(KeyEvent::new(
                KeyCode::Char('h'),
                crossterm::event::KeyModifiers::NONE,
            )),
            ResponsePaneAction::Consumed
        );
        assert_eq!(pane.active_tab(), ResponseTab::SentRequest);
        assert_eq!(
            pane.handle_key(KeyEvent::new(
                KeyCode::Char('l'),
                crossterm::event::KeyModifiers::NONE,
            )),
            ResponsePaneAction::Consumed
        );
        assert_eq!(pane.active_tab(), ResponseTab::Body);
        assert_eq!(
            pane.handle_key(KeyEvent::new(
                KeyCode::Down,
                crossterm::event::KeyModifiers::NONE,
            )),
            ResponsePaneAction::Consumed
        );
        assert!(!pane.tab_bar_focused());
    }

    #[test]
    fn tab_and_backtab_leave_from_tabs_or_content_immediately() {
        let tab = KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE);
        let backtab = KeyEvent::new(KeyCode::BackTab, crossterm::event::KeyModifiers::SHIFT);
        let down = KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE);
        let mut pane = ResponsePane::new();
        assert_eq!(pane.handle_key(tab), ResponsePaneAction::LeaveDown);
        assert_eq!(pane.handle_key(backtab), ResponsePaneAction::LeaveUp);
        assert_eq!(pane.handle_key(down), ResponsePaneAction::Consumed);
        assert!(!pane.tab_bar_focused());
        assert_eq!(pane.handle_key(tab), ResponsePaneAction::LeaveDown);
        assert_eq!(pane.handle_key(backtab), ResponsePaneAction::LeaveUp);
        pane.focus_tab_bar();
        assert_eq!(
            pane.handle_key(KeyEvent::new(
                KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            )),
            ResponsePaneAction::Consumed
        );
        assert!(!pane.tab_bar_focused());
    }

    #[test]
    fn rendering_records_all_jump_targets_without_painting_a_pane_background() {
        let mut pane = ResponsePane::new();
        let area = Rect::new(0, 0, 80, 12);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, false, &Settings::default());
        assert_eq!(
            pane.jump_targets()
                .iter()
                .map(|target| target.0)
                .collect::<Vec<_>>(),
            vec!['a', 's', 'd', 'f', 'g', 'h']
        );
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| cell.style().bg == Some(ratatui::style::Color::Reset))
        );
    }

    #[test]
    fn title_and_optional_size_timing_subtitle_are_rendered() {
        let mut value = response(&[b'x'; 2_048], None);
        value.timings.total = Some(Duration::from_millis(12));
        let mut pane = ResponsePane::new();
        pane.set_response(&value, &Settings::default());
        let area = Rect::new(0, 0, 80, 12);
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true, &Settings::default());
        let text = rendered_text(&buffer);
        assert!(text.contains("Response 201 Created"));
        assert!(text.contains("2.00 KB in 12.00ms"));

        let mut settings = Settings::default();
        settings.response.show_size_and_time = false;
        let mut buffer = Buffer::empty(area);
        pane.render(area, &mut buffer, true, &settings);
        assert!(!rendered_text(&buffer).contains("2.00 KB in 12.00ms"));
    }
}
