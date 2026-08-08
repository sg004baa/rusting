//! The URL bar: method selector, URL input, status pill, timing markers, send
//! button, and the variable value preview line beneath them.
//!
//! Four rows high. The top three are the bordered control band; the fourth is
//! the preview line, which shows the value of the variable under the caret so
//! a `$TOKEN` can be checked without leaving the field.
//!
//! The send button is deliberately not focusable. It is a label for `enter`,
//! not a tab stop — posting made the same call, and a focusable button in a
//! keyboard-only client is a stop nobody wants to walk through.

use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget as _};
use rusting_core::{HttpMethod, PathParam, Settings, Variables, urls, variables};
use rusting_http::{PhaseOutcome, Timings};

use crate::theme::{self, MarkerState};
use crate::widgets::fuzzy;
use crate::widgets::highlight;
use crate::widgets::input::{Input, InputAction};
use crate::widgets::popup::{Popup, PopupAction, PopupItem};
use crate::widgets::select::{Select, SelectAction};

/// Bordered method selector.
const METHOD_WIDTH: u16 = 11;
/// Bordered send button.
const SEND_WIDTH: u16 = 10;
/// Three status digits plus a space either side.
const STATUS_WIDTH: u16 = 5;
/// Five phase markers plus a space either side.
const MARKERS_WIDTH: u16 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlBarAction {
    /// Not a URL bar key.
    Ignored,
    /// Handled, nothing for the caller to do.
    Consumed,
    /// The URL changed; the caller re-derives path params.
    Changed,
    /// `enter` in the URL input.
    Send,
    MethodChanged,
    /// `ctrl+y`.
    CopyUrl,
    /// `alt+down` on a `:name` token. The caller jumps to the Path tab.
    JumpToPathParam(String),
    /// The caret tried to leave downwards.
    LeaveDown,
}

/// Which of the two focusable controls is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrlFocus {
    Method,
    Url,
}

/// What an accepted completion should replace.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Completion {
    /// A `$VAR` token: replace exactly the token's byte range.
    Variable { range: Range<usize>, braced: bool },
    /// A base URL: replace the whole value, because the candidate *is* the
    /// start of a URL and anything already typed was a prefix of it.
    BaseUrl,
}

pub struct UrlBar {
    method: Select<HttpMethod>,
    url: Input,
    popup: Popup,
    completion: Option<Completion>,
    focus: UrlFocus,
    base_url_candidates: Vec<String>,
    /// Status of the most recent response. `None` hides the pill.
    status: Option<u16>,
    timings: Timings,
    /// Rects captured during the last render, for jump labels and the popup
    /// anchor. Layout is not known until render time.
    method_area: Rect,
    url_area: Rect,
    caret_anchor: Rect,
}

impl Default for UrlBar {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlBar {
    pub fn new() -> Self {
        let options: Vec<(String, HttpMethod)> = HttpMethod::ALL
            .into_iter()
            .map(|method| (method.as_str().to_owned(), method))
            .collect();
        let mnemonics: Vec<Option<(char, usize)>> = HttpMethod::ALL
            .into_iter()
            .map(|method| Some((method.mnemonic(), method.mnemonic_index())))
            .collect();
        Self {
            method: Select::new(options).with_mnemonics(mnemonics),
            url: Input::with_placeholder("Enter a URL…"),
            popup: Popup::new(),
            completion: None,
            focus: UrlFocus::Url,
            base_url_candidates: Vec::new(),
            status: None,
            timings: Timings::default(),
            method_area: Rect::ZERO,
            url_area: Rect::ZERO,
            caret_anchor: Rect::ZERO,
        }
    }

    pub fn url(&self) -> &str {
        self.url.value()
    }

    pub fn set_url(&mut self, url: &str) {
        self.url.set_value(url);
        self.close_completions();
    }

    pub fn method(&self) -> HttpMethod {
        *self.method.value()
    }

    pub fn set_method(&mut self, method: HttpMethod) {
        let _ = self.method.set_value(&method);
    }

    pub fn focus_url(&mut self) {
        self.focus = UrlFocus::Url;
        self.method.close();
    }

    pub fn focus_method(&mut self) {
        self.focus = UrlFocus::Method;
        self.close_completions();
    }

    /// `scheme://host` candidates gathered from the collection.
    pub fn set_base_url_candidates(&mut self, candidates: Vec<String>) {
        self.base_url_candidates = candidates;
    }

    pub fn set_response(&mut self, status: u16, _reason: &str) {
        self.status = Some(status);
    }

    pub fn clear_response(&mut self) {
        self.status = None;
        self.timings = Timings::default();
    }

    pub fn set_timings(&mut self, timings: &Timings) {
        self.timings = timings.clone();
    }

    pub fn handle_key(&mut self, key: KeyEvent, variables: &Variables) -> UrlBarAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('j') {
            return UrlBarAction::Send;
        }
        if self.popup.is_open() {
            match self.popup.handle_key(key) {
                PopupAction::Accepted(index) => {
                    self.accept_completion(index);
                    return UrlBarAction::Changed;
                }
                PopupAction::Dismissed => {
                    self.close_completions();
                    return UrlBarAction::Consumed;
                }
                PopupAction::Consumed => return UrlBarAction::Consumed,
                PopupAction::Ignored => {}
            }
        }

        match self.focus {
            UrlFocus::Method => self.handle_method_key(key),
            UrlFocus::Url => self.handle_url_key(key, variables),
        }
    }

    fn handle_method_key(&mut self, key: KeyEvent) -> UrlBarAction {
        match self.method.handle_key(key) {
            SelectAction::Changed => UrlBarAction::MethodChanged,
            SelectAction::Consumed => UrlBarAction::Consumed,
            SelectAction::LeaveDown => UrlBarAction::LeaveDown,
            // Nothing sits above the URL bar, so upward escape stays here.
            SelectAction::LeaveUp => UrlBarAction::Consumed,
            SelectAction::Ignored => UrlBarAction::Ignored,
        }
    }

    fn handle_url_key(&mut self, key: KeyEvent, variables: &Variables) -> UrlBarAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        if ctrl && key.code == KeyCode::Char('y') {
            return UrlBarAction::CopyUrl;
        }
        if alt && key.code == KeyCode::Down {
            return match self.path_param_at_caret() {
                Some(name) => UrlBarAction::JumpToPathParam(name),
                None => UrlBarAction::Consumed,
            };
        }
        // `down` opens the completion list when there is something to show.
        // With nothing to show it falls through to the input, which reports
        // `LeaveDown` — so the key never silently does nothing.
        if key.code == KeyCode::Down && key.modifiers.is_empty() {
            self.refresh_completions(variables);
            if self.popup.is_open() {
                return UrlBarAction::Consumed;
            }
        }

        match self.url.handle_key(key) {
            InputAction::Submitted => UrlBarAction::Send,
            InputAction::Changed => {
                self.refresh_completions(variables);
                UrlBarAction::Changed
            }
            InputAction::Consumed => UrlBarAction::Consumed,
            InputAction::LeaveDown => UrlBarAction::LeaveDown,
            InputAction::LeaveUp => UrlBarAction::Consumed,
            InputAction::Ignored => UrlBarAction::Ignored,
        }
    }

    /// The `:name` token the caret sits on, if any.
    fn path_param_at_caret(&self) -> Option<String> {
        let cursor = self.url.cursor();
        urls::find_path_params(self.url.value())
            .into_iter()
            .find(|token| token.start <= cursor && cursor <= token.end)
            .map(|token| token.name)
    }

    fn close_completions(&mut self) {
        self.popup.close();
        self.completion = None;
    }

    /// Rebuilds the completion list for the caret's current context.
    ///
    /// Inside a `$VAR` token the environment's names are offered; anywhere else
    /// the base URLs collected from the collection are.
    fn refresh_completions(&mut self, variables: &Variables) {
        let value = self.url.value();
        let cursor = self.url.cursor();
        let variable = variables::variable_at_cursor(value, cursor).or_else(|| {
            // For completion, the insertion point immediately after an
            // unbraced name is still part of the token: that is where the
            // caret sits while `$HO` is being typed before a `/`, `?`, or any
            // other existing URL suffix. The core hit-test is deliberately
            // narrower for previews in ordinary prose.
            variables::find_variables(value)
                .into_iter()
                .find(|token| !token.braced && cursor > token.start && cursor == token.end)
        });

        if let Some(token) = variable {
            let names: Vec<&str> = variables.keys().map(String::as_str).collect();
            let items = fuzzy::rank(&token.name, &names)
                .into_iter()
                .map(|matched| PopupItem {
                    text: format!("${}", names[matched.index]),
                    // The rendered candidate carries a `$` the ranked name did
                    // not, so every matched column shifts right by one.
                    match_positions: matched.positions.iter().map(|at| at + 1).collect(),
                    style: theme::variable(true),
                })
                .collect::<Vec<_>>();
            self.completion = Some(Completion::Variable {
                range: token.start..token.end,
                braced: token.braced,
            });
            self.popup.open(items);
        } else {
            let candidates: Vec<&str> = self
                .base_url_candidates
                .iter()
                .map(String::as_str)
                .collect();
            let items = fuzzy::rank(value, &candidates)
                .into_iter()
                .map(|matched| PopupItem {
                    text: candidates[matched.index].to_owned(),
                    match_positions: matched.positions,
                    style: theme::url::base(),
                })
                .collect::<Vec<_>>();
            self.completion = Some(Completion::BaseUrl);
            self.popup.open(items);
        }

        if !self.popup.is_open() {
            self.completion = None;
        }
    }

    fn accept_completion(&mut self, index: usize) {
        let Some(item) = self.popup.items().get(index) else {
            return;
        };
        let text = item.text.clone();
        match self.completion.take() {
            Some(Completion::Variable { range, braced }) => {
                let name = text.strip_prefix('$').unwrap_or(&text);
                let replacement = if braced {
                    format!("${{{name}}}")
                } else {
                    format!("${name}")
                };
                self.url.splice(range, &replacement);
            }
            Some(Completion::BaseUrl) => {
                let end = self.url.value().len();
                self.url.splice(0..end, &text);
            }
            None => {}
        }
        self.popup.close();
    }

    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        focused: bool,
        settings: &Settings,
        variables: &Variables,
        path_params: &[PathParam],
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let band = Rect::new(area.x, area.y, area.width, area.height.min(3));
        let layout = BarLayout::compute(band, self.status.is_some(), !self.timings.is_empty());
        self.method_area = layout.method;
        self.url_area = layout.url;

        let method_focused = focused && self.focus == UrlFocus::Method;
        let url_focused = focused && self.focus == UrlFocus::Url;

        self.render_method(layout.method, buffer, method_focused);
        self.render_url(layout.url, buffer, url_focused, variables, path_params);
        if let Some(rect) = layout.status {
            self.render_status(rect, buffer);
        }
        if let Some(rect) = layout.markers {
            self.render_markers(rect, buffer);
        }
        render_send(layout.send, buffer);

        if area.height >= 4 && settings.url_bar.show_value_preview && url_focused {
            let row = Rect::new(area.x, area.y + 3, area.width, 1);
            if let Some(line) = self.preview_line(settings, variables) {
                line.centered().render(row, buffer);
            }
        }
    }

    fn render_method(&mut self, area: Rect, buffer: &mut Buffer, focused: bool) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::border(focused));
        let inner = block.inner(area);
        block.render(area, buffer);
        self.method.render(inner, buffer, focused);
    }

    fn render_url(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        focused: bool,
        variables: &Variables,
        path_params: &[PathParam],
    ) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::border(focused));
        let inner = block.inner(area);
        block.render(area, buffer);

        let caret = focused.then(|| self.url.cursor());
        let highlights = highlight::url(self.url.value(), variables, path_params, caret);
        self.url.render(inner, buffer, focused, &highlights);

        let column = self.url.caret_column(inner.width as usize);
        self.caret_anchor = Rect::new(inner.x + column, inner.y, 1, 1);
    }

    fn render_status(&self, area: Rect, buffer: &mut Buffer) {
        let Some(status) = self.status else {
            return;
        };
        let style = Style::new()
            .fg(theme::status_color(status))
            .add_modifier(Modifier::BOLD);
        Line::from(Span::styled(format!("{status}"), style))
            .centered()
            .render(area, buffer);
    }

    fn render_markers(&self, area: Rect, buffer: &mut Buffer) {
        let mut spans = Vec::with_capacity(7);
        spans.push(Span::raw(" "));
        for (_, outcome) in self.timings.iter() {
            spans.push(Span::styled(
                "\u{25a0}",
                theme::timing_marker(marker_state(outcome)),
            ));
        }
        spans.push(Span::raw(" "));
        Line::from(spans).render(area, buffer);
    }

    /// The `NAME = value` line for the variable under the caret.
    ///
    /// An undefined variable is reported rather than skipped: a silent blank
    /// line is exactly how a typo'd variable name goes unnoticed until the
    /// request fails.
    fn preview_line(&self, settings: &Settings, variables: &Variables) -> Option<Line<'static>> {
        let token = variables::variable_at_cursor(self.url.value(), self.url.cursor())?;
        match variables.get(&token.name) {
            Some(value) => {
                let shown = if settings.url_bar.hide_secrets_in_value_preview
                    && settings.is_secret_name(&token.name)
                {
                    "(hidden)"
                } else {
                    value.as_str()
                };
                Some(Line::from(Span::styled(
                    format!("{} = {}", token.name, shown),
                    Style::new().fg(theme::MUTED),
                )))
            }
            None => Some(Line::from(Span::styled(
                format!("{} = <undefined>", token.name),
                Style::new().fg(theme::ERROR),
            ))),
        }
    }

    /// The completion popup, drawn last so it sits over everything.
    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        self.method
            .render_overlay(inner_of(self.method_area), screen, buffer);
        self.popup.render(self.caret_anchor, screen, buffer);
    }

    pub fn jump_targets(&self) -> Vec<(char, Position)> {
        vec![
            ('1', Position::new(self.method_area.x, self.method_area.y)),
            ('2', Position::new(self.url_area.x, self.url_area.y)),
        ]
    }
}

/// The bordered inner rect of a control, without rebuilding the block.
fn inner_of(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

fn marker_state(outcome: PhaseOutcome) -> MarkerState {
    match outcome {
        PhaseOutcome::Skipped => MarkerState::NotStarted,
        PhaseOutcome::Started => MarkerState::Started,
        PhaseOutcome::Completed(_) => MarkerState::Complete,
        PhaseOutcome::Failed => MarkerState::Failed,
    }
}

fn render_send(area: Rect, buffer: &mut Buffer) {
    if area.width == 0 {
        return;
    }
    // Never focusable, so the border is always the unfocused one.
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::border(false));
    let inner = block.inner(area);
    block.render(area, buffer);
    Line::from(Span::styled(
        "Send",
        Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
    ))
    .centered()
    .render(inner, buffer);
}

/// Where each control sits on the band.
///
/// The status pill and marker strip only take space when they have something
/// to say, so an untouched URL bar gives the whole middle to the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BarLayout {
    method: Rect,
    url: Rect,
    status: Option<Rect>,
    markers: Option<Rect>,
    send: Rect,
}

impl BarLayout {
    fn compute(band: Rect, show_status: bool, show_markers: bool) -> Self {
        let mut remaining = band.width;
        let mut take = |width: u16| {
            let taken = width.min(remaining);
            remaining -= taken;
            taken
        };
        // Fixed controls claim their width first; the URL takes what is left.
        let method_width = take(METHOD_WIDTH);
        let send_width = take(SEND_WIDTH);
        let status_width = if show_status { take(STATUS_WIDTH) } else { 0 };
        let markers_width = if show_markers { take(MARKERS_WIDTH) } else { 0 };
        let url_width = remaining;

        // The unbordered strips sit on the middle row of the band.
        let strip_y = band.y + if band.height >= 3 { 1 } else { 0 };
        let mut x = band.x;
        let method = Rect::new(x, band.y, method_width, band.height);
        x += method_width;
        let url = Rect::new(x, band.y, url_width, band.height);
        x += url_width;
        let status = (status_width > 0).then(|| Rect::new(x, strip_y, status_width, 1));
        x += status_width;
        let markers = (markers_width > 0).then(|| Rect::new(x, strip_y, markers_width, 1));
        x += markers_width;
        let send = Rect::new(x, band.y, send_width, band.height);

        Self {
            method,
            url,
            status,
            markers,
            send,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusting_core::config;
    use rusting_http::Phase;
    use std::time::Duration;

    fn settings() -> Settings {
        Settings::default()
    }

    fn vars(pairs: &[(&str, &str)]) -> Variables {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Renders into a 4-row bar and returns the buffer.
    fn render(bar: &mut UrlBar, settings: &Settings, variables: &Variables) -> Buffer {
        let area = Rect::new(0, 0, 60, 4);
        let mut buffer = Buffer::empty(area);
        bar.render(area, &mut buffer, true, settings, variables, &[]);
        buffer
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().to_owned())
            .collect::<String>()
            .trim()
            .to_owned()
    }

    /// Puts the caret inside the `$NAME` token so the preview has something to
    /// report.
    fn with_caret_in_variable(url: &str, name: &str) -> UrlBar {
        let mut bar = UrlBar::new();
        bar.set_url(url);
        let start = url.find(name).expect("variable must appear in the url");
        bar.url.set_cursor(start + 1);
        bar
    }

    #[test]
    fn preview_shows_the_value_of_the_variable_under_the_caret() {
        let mut bar = with_caret_in_variable("https://$HOST/users", "$HOST");
        let variables = vars(&[("HOST", "api.example.com")]);
        let buffer = render(&mut bar, &settings(), &variables);
        assert_eq!(row(&buffer, 3), "HOST = api.example.com");
    }

    #[test]
    fn preview_hides_the_value_of_a_secret_variable() {
        let mut bar = with_caret_in_variable("https://x/$API_TOKEN", "$API_TOKEN");
        let variables = vars(&[("API_TOKEN", "hunter2")]);
        let buffer = render(&mut bar, &settings(), &variables);
        assert_eq!(row(&buffer, 3), "API_TOKEN = (hidden)");
    }

    #[test]
    fn preview_keeps_the_value_when_secret_hiding_is_off() {
        let mut bar = with_caret_in_variable("https://x/$API_TOKEN", "$API_TOKEN");
        let mut settings = settings();
        settings.url_bar.hide_secrets_in_value_preview = false;
        let variables = vars(&[("API_TOKEN", "hunter2")]);
        let buffer = render(&mut bar, &settings, &variables);
        assert_eq!(row(&buffer, 3), "API_TOKEN = hunter2");
    }

    #[test]
    fn preview_reports_an_undefined_variable_in_the_error_colour() {
        let mut bar = with_caret_in_variable("https://$NOPE/x", "$NOPE");
        let buffer = render(&mut bar, &settings(), &Variables::new());
        let text = row(&buffer, 3);
        assert_eq!(text, "NOPE = <undefined>");

        let column = (0..buffer.area.width)
            .find(|x| buffer[(*x, 3)].symbol() == "N")
            .expect("the preview must be drawn");
        assert_eq!(buffer[(column, 3)].style().fg, Some(theme::ERROR));
    }

    #[test]
    fn preview_is_blank_when_the_caret_is_not_in_a_variable() {
        let mut bar = UrlBar::new();
        bar.set_url("https://example.com");
        let buffer = render(&mut bar, &settings(), &vars(&[("HOST", "x")]));
        assert_eq!(row(&buffer, 3), "");
    }

    #[test]
    fn preview_is_suppressed_by_the_setting() {
        let mut bar = with_caret_in_variable("https://$HOST/x", "$HOST");
        let mut settings = settings();
        settings.url_bar.show_value_preview = false;
        let buffer = render(&mut bar, &settings, &vars(&[("HOST", "api")]));
        assert_eq!(row(&buffer, 3), "");
    }

    #[test]
    fn the_status_pill_is_hidden_until_a_response_arrives() {
        let mut bar = UrlBar::new();
        let buffer = render(&mut bar, &settings(), &Variables::new());
        assert!(!row(&buffer, 1).contains("200"));

        bar.set_response(200, "OK");
        let buffer = render(&mut bar, &settings(), &Variables::new());
        assert!(row(&buffer, 1).contains("200"));
    }

    #[test]
    fn the_status_pill_is_coloured_by_status_class() {
        for (status, expected) in [
            (200u16, theme::SUCCESS),
            (301, theme::WARNING),
            (500, theme::ERROR),
        ] {
            let mut bar = UrlBar::new();
            bar.set_response(status, "");
            let buffer = render(&mut bar, &settings(), &Variables::new());
            let column = (0..buffer.area.width)
                .find(|x| {
                    buffer[(*x, 1)].symbol() == "2"
                        || buffer[(*x, 1)].symbol() == "3"
                        || buffer[(*x, 1)].symbol() == "5"
                })
                .expect("the status must be drawn");
            assert_eq!(
                buffer[(column, 1)].style().fg,
                Some(expected),
                "status {status}"
            );
        }
    }

    #[test]
    fn markers_are_hidden_while_the_timings_are_empty() {
        let mut bar = UrlBar::new();
        let buffer = render(&mut bar, &settings(), &Variables::new());
        assert!(!row(&buffer, 1).contains('\u{25a0}'));

        let mut timings = Timings::default();
        timings.set(
            Phase::Connect,
            PhaseOutcome::Completed(Duration::from_millis(3)),
        );
        bar.set_timings(&timings);
        let buffer = render(&mut bar, &settings(), &Variables::new());
        assert_eq!(row(&buffer, 1).matches('\u{25a0}').count(), 5);
    }

    #[test]
    fn markers_colour_each_phase_by_outcome() {
        let mut timings = Timings::default();
        timings.set(
            Phase::Dns,
            PhaseOutcome::Completed(Duration::from_millis(1)),
        );
        timings.set(Phase::Connect, PhaseOutcome::Failed);
        timings.set(Phase::Tls, PhaseOutcome::Started);
        let mut bar = UrlBar::new();
        bar.set_timings(&timings);
        let buffer = render(&mut bar, &settings(), &Variables::new());

        let columns: Vec<u16> = (0..buffer.area.width)
            .filter(|x| buffer[(*x, 1)].symbol() == "\u{25a0}")
            .collect();
        assert_eq!(columns.len(), 5);
        assert_eq!(
            buffer[(columns[0], 1)].style().fg,
            theme::timing_marker(MarkerState::Complete).fg
        );
        assert_eq!(
            buffer[(columns[1], 1)].style().fg,
            theme::timing_marker(MarkerState::Failed).fg
        );
        assert_eq!(
            buffer[(columns[2], 1)].style().fg,
            theme::timing_marker(MarkerState::Started).fg
        );
        assert_eq!(
            buffer[(columns[3], 1)].style().fg,
            theme::timing_marker(MarkerState::NotStarted).fg
        );
    }

    #[test]
    fn clearing_the_response_hides_both_strips() {
        let mut bar = UrlBar::new();
        bar.set_response(500, "Server Error");
        let mut timings = Timings::default();
        timings.set(Phase::Download, PhaseOutcome::Failed);
        bar.set_timings(&timings);
        bar.clear_response();

        let buffer = render(&mut bar, &settings(), &Variables::new());
        let line = row(&buffer, 1);
        assert!(!line.contains("500"));
        assert!(!line.contains('\u{25a0}'));
    }

    #[test]
    fn alt_down_reports_the_path_param_under_the_caret() {
        let mut bar = UrlBar::new();
        bar.set_url("https://x/users/:userId/posts/:postId");
        bar.url.set_cursor("https://x/users/:us".len());
        let action = bar.handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::ALT),
            &Variables::new(),
        );
        assert_eq!(action, UrlBarAction::JumpToPathParam("userId".to_owned()));
    }

    #[test]
    fn alt_down_outside_a_path_param_is_consumed_without_a_jump() {
        let mut bar = UrlBar::new();
        bar.set_url("https://x/users/:userId");
        bar.url.set_cursor(3);
        let action = bar.handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::ALT),
            &Variables::new(),
        );
        assert_eq!(action, UrlBarAction::Consumed);
    }

    #[test]
    fn enter_in_the_url_input_sends() {
        let mut bar = UrlBar::new();
        bar.set_url("https://example.com");
        assert_eq!(
            bar.handle_key(key(KeyCode::Enter), &Variables::new()),
            UrlBarAction::Send
        );
    }

    #[test]
    fn ctrl_y_copies_the_url() {
        let mut bar = UrlBar::new();
        bar.set_url("https://example.com");
        assert_eq!(
            bar.handle_key(
                KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
                &Variables::new()
            ),
            UrlBarAction::CopyUrl
        );
    }

    #[test]
    fn accepting_a_variable_completion_replaces_only_the_token() {
        let mut bar = UrlBar::new();
        bar.set_url("https://$/users");
        bar.url.set_cursor("https://$".len());
        let variables = vars(&[("HOST", "api.example.com")]);

        assert_eq!(
            bar.handle_key(key(KeyCode::Char('H')), &variables),
            UrlBarAction::Changed
        );
        assert!(bar.popup.is_open());
        assert_eq!(
            bar.handle_key(key(KeyCode::Char('O')), &variables),
            UrlBarAction::Changed
        );
        assert!(bar.popup.is_open());
        assert_eq!(
            bar.handle_key(key(KeyCode::Enter), &variables),
            UrlBarAction::Changed
        );
        assert_eq!(bar.url(), "https://$HOST/users");
    }

    #[test]
    fn accepting_a_braced_variable_completion_keeps_the_braces() {
        let mut bar = UrlBar::new();
        bar.set_url("https://${HO}/users");
        bar.url.set_cursor("https://${HO".len());
        let variables = vars(&[("HOST", "api.example.com")]);
        bar.refresh_completions(&variables);
        bar.accept_completion(0);
        assert_eq!(bar.url(), "https://${HOST}/users");
    }

    #[test]
    fn accepting_a_base_url_completion_replaces_the_whole_value() {
        let mut bar = UrlBar::new();
        bar.set_base_url_candidates(vec!["https://api.example.com".to_owned()]);
        bar.set_url("https://api");
        bar.refresh_completions(&Variables::new());
        assert!(bar.popup.is_open());
        bar.accept_completion(0);
        assert_eq!(bar.url(), "https://api.example.com");
    }

    #[test]
    fn layout_gives_the_freed_strips_back_to_the_url() {
        let band = Rect::new(0, 0, 60, 3);
        let bare = BarLayout::compute(band, false, false);
        let full = BarLayout::compute(band, true, true);
        assert!(bare.status.is_none() && bare.markers.is_none());
        assert_eq!(
            bare.url.width,
            full.url.width + STATUS_WIDTH + MARKERS_WIDTH
        );
        assert_eq!(full.send.x + full.send.width, band.width);
    }

    #[test]
    fn jump_targets_point_at_the_two_focusable_controls() {
        let mut bar = UrlBar::new();
        render(&mut bar, &settings(), &Variables::new());
        let targets = bar.jump_targets();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].0, '1');
        assert_eq!(targets[1].0, '2');
        assert_eq!(targets[0].1, Position::new(0, 0));
        assert_eq!(targets[1].1, Position::new(METHOD_WIDTH, 0));
    }

    #[test]
    fn a_zero_sized_area_renders_nothing() {
        let mut bar = UrlBar::new();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        bar.render(
            Rect::new(0, 0, 0, 0),
            &mut buffer,
            true,
            &settings(),
            &Variables::new(),
            &[],
        );
    }

    #[test]
    fn secret_detection_follows_the_settings_helper() {
        // Guards the preview's contract against a settings change: the pane
        // must not grow its own idea of what a secret is.
        let settings = settings();
        assert!(settings.is_secret_name("API_TOKEN"));
        assert!(!settings.is_secret_name("HOST"));
        let _ = config::SECRET_NAME_MARKERS;
    }

    #[test]
    fn ctrl_j_sends_even_when_the_method_control_is_focused() {
        let mut bar = UrlBar::new();
        bar.focus_method();
        assert_eq!(
            bar.handle_key(
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
                &Variables::new(),
            ),
            UrlBarAction::Send
        );
    }

    #[test]
    fn method_setter_uses_the_exact_public_contract() {
        let mut bar = UrlBar::new();
        bar.set_method(HttpMethod::Patch);
        assert_eq!(bar.method(), HttpMethod::Patch);
    }

    #[test]
    fn unfocused_render_keeps_the_terminal_background_transparent() {
        use ratatui::style::Color;

        let mut bar = UrlBar::new();
        let area = Rect::new(0, 0, 60, 4);
        let mut buffer = Buffer::empty(area);
        bar.render(
            area,
            &mut buffer,
            false,
            &settings(),
            &Variables::new(),
            &[],
        );
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| cell.style().bg == Some(Color::Reset))
        );
    }

    #[test]
    fn url_render_uses_shared_variable_and_path_param_highlighting() {
        let mut bar = UrlBar::new();
        let text = "https://$HOST/:id";
        bar.set_url(text);
        let area = Rect::new(0, 0, 60, 4);
        let mut buffer = Buffer::empty(area);
        let variables = vars(&[("HOST", "api.example.com")]);
        let path_params = vec![PathParam {
            name: "id".to_owned(),
            value: "42".to_owned(),
        }];
        bar.render(
            area,
            &mut buffer,
            true,
            &settings(),
            &variables,
            &path_params,
        );
        let text_x = METHOD_WIDTH + 1;
        let variable_x = text_x + text.find("$HOST").expect("variable") as u16;
        let path_x = text_x + text.find(":id").expect("path param") as u16;
        assert_eq!(buffer[(variable_x, 1)].style().fg, theme::variable(true).fg);
        assert_eq!(buffer[(path_x, 1)].style().fg, theme::path_param(true).fg);
    }
}
