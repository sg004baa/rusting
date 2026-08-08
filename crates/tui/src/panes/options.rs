//! Per-request transport options.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Paragraph, Widget as _};
use rusting_core::{Options, RequestModel, Variables};

use crate::theme;
use crate::widgets::checkbox::{Checkbox, CheckboxAction};
use crate::widgets::highlight;
use crate::widgets::input::{Input, InputAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsAction {
    Ignored,
    Consumed,
    Changed,
    LeaveUp,
    LeaveDown,
}

pub struct OptionsTab {
    follow_redirects: Checkbox,
    verify_ssl: Checkbox,
    attach_cookies: Checkbox,
    proxy_url: Input,
    timeout: Input,
    focus: usize,
}

impl OptionsTab {
    pub fn new() -> Self {
        let defaults = Options::default();
        let mut timeout = Input::with_placeholder("Seconds");
        timeout.set_value(defaults.timeout.to_string());
        Self {
            follow_redirects: Checkbox::new("Follow redirects", defaults.follow_redirects),
            verify_ssl: Checkbox::new("Verify SSL certificates", defaults.verify_ssl),
            attach_cookies: Checkbox::new("Attach cookies", defaults.attach_cookies),
            proxy_url: Input::with_placeholder("http://proxy.example:8080"),
            timeout,
            focus: 0,
        }
    }

    pub fn load(&mut self, request: &RequestModel) {
        self.follow_redirects.checked = request.options.follow_redirects;
        self.verify_ssl.checked = request.options.verify_ssl;
        self.attach_cookies.checked = request.options.attach_cookies;
        self.proxy_url.set_value(&request.options.proxy_url);
        self.timeout.set_value(request.options.timeout.to_string());
        self.focus = 0;
    }

    pub fn to_model(&self) -> Result<Options, String> {
        let timeout = self
            .timeout
            .value()
            .trim()
            .parse::<f64>()
            .map_err(|_| "Timeout must be a number of seconds.".to_owned())?;
        if !timeout.is_finite() || timeout <= 0.0 {
            return Err("Timeout must be a positive, finite number of seconds.".to_owned());
        }
        Ok(Options {
            follow_redirects: self.follow_redirects.checked,
            verify_ssl: self.verify_ssl.checked,
            attach_cookies: self.attach_cookies.checked,
            proxy_url: self.proxy_url.value().to_owned(),
            timeout,
        })
    }

    pub fn handle_key(&mut self, key: KeyEvent, _variables: &Variables) -> OptionsAction {
        if key.code == KeyCode::Tab {
            return self.move_down();
        }
        if key.code == KeyCode::BackTab {
            return self.move_up();
        }
        let action = match self.focus {
            0 => map_checkbox(self.follow_redirects.handle_key(key)),
            1 => map_checkbox(self.verify_ssl.handle_key(key)),
            2 => map_checkbox(self.attach_cookies.handle_key(key)),
            3 => map_input(self.proxy_url.handle_key(key)),
            4 => map_input(self.timeout.handle_key(key)),
            _ => OptionsAction::Ignored,
        };
        match action {
            OptionsAction::LeaveUp => self.move_up(),
            OptionsAction::LeaveDown => self.move_down(),
            other => other,
        }
    }

    fn move_up(&mut self) -> OptionsAction {
        if self.focus == 0 {
            OptionsAction::LeaveUp
        } else {
            self.focus -= 1;
            OptionsAction::Consumed
        }
    }

    fn move_down(&mut self) -> OptionsAction {
        if self.focus == 4 {
            OptionsAction::LeaveDown
        } else {
            self.focus += 1;
            OptionsAction::Consumed
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
        let [controls, explanation] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(area);
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(controls);
        render_checkbox(
            &self.follow_redirects,
            rows[0],
            buffer,
            focused && self.focus == 0,
        );
        render_checkbox(
            &self.verify_ssl,
            rows[1],
            buffer,
            focused && self.focus == 1,
        );
        render_checkbox(
            &self.attach_cookies,
            rows[2],
            buffer,
            focused && self.focus == 2,
        );
        let proxy_focused = focused && self.focus == 3;
        let proxy_inner = bordered(rows[3], buffer, proxy_focused, "Proxy URL", None);
        let proxy_highlights = highlight::variables(
            self.proxy_url.value(),
            variables,
            proxy_focused.then(|| self.proxy_url.cursor()),
        );
        self.proxy_url
            .render(proxy_inner, buffer, proxy_focused, &proxy_highlights);
        let timeout_focused = focused && self.focus == 4;
        let invalid_timeout = self
            .timeout
            .value()
            .trim()
            .parse::<f64>()
            .map_or(true, |value| !value.is_finite() || value <= 0.0);
        let timeout_inner = bordered(
            rows[4],
            buffer,
            timeout_focused,
            "Timeout",
            invalid_timeout.then(|| Style::new().fg(theme::ERROR)),
        );
        self.timeout
            .render(timeout_inner, buffer, timeout_focused, &[]);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::border(false))
            .title("About this option");
        let inner = block.inner(explanation);
        block.render(explanation, buffer);
        Paragraph::new(DESCRIPTIONS[self.focus])
            .wrap(ratatui::widgets::Wrap { trim: true })
            .render(inner, buffer);
    }

    pub fn render_overlay(&mut self, _screen: Rect, _buffer: &mut Buffer) {}
}

impl Default for OptionsTab {
    fn default() -> Self {
        Self::new()
    }
}

const DESCRIPTIONS: [&str; 5] = [
    "Follow HTTP redirects until the final response is reached.",
    "Verify the server certificate and hostname. Disable only for trusted development servers.",
    "Attach cookies collected during this rusting session to the request.",
    "Route this request through the given HTTP or HTTPS proxy. Leave blank for no proxy.",
    "Maximum total request duration in seconds. It must be a positive number.",
];

fn map_checkbox(action: CheckboxAction) -> OptionsAction {
    match action {
        CheckboxAction::Toggled => OptionsAction::Changed,
        CheckboxAction::Consumed => OptionsAction::Consumed,
        CheckboxAction::LeaveUp => OptionsAction::LeaveUp,
        CheckboxAction::LeaveDown => OptionsAction::LeaveDown,
        CheckboxAction::Ignored => OptionsAction::Ignored,
    }
}

fn map_input(action: InputAction) -> OptionsAction {
    match action {
        InputAction::Changed => OptionsAction::Changed,
        InputAction::Consumed | InputAction::Submitted => OptionsAction::Consumed,
        InputAction::LeaveUp => OptionsAction::LeaveUp,
        InputAction::LeaveDown => OptionsAction::LeaveDown,
        InputAction::Ignored => OptionsAction::Ignored,
    }
}

fn render_checkbox(checkbox: &Checkbox, area: Rect, buffer: &mut Buffer, focused: bool) {
    let inner = bordered(area, buffer, focused, "", None);
    checkbox.render(inner, buffer, focused);
}

fn bordered(
    area: Rect,
    buffer: &mut Buffer,
    focused: bool,
    title: &str,
    override_style: Option<Style>,
) -> Rect {
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(override_style.unwrap_or_else(|| theme::border(focused)));
    if !title.is_empty() {
        block = block.title(Line::from(title).style(theme::border_title(focused)));
    }
    let inner = block.inner(area);
    block.render(area, buffer);
    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_timeout_is_an_error_not_a_default() {
        let mut tab = OptionsTab::new();
        tab.timeout.set_value("not-a-number");
        assert_eq!(
            tab.to_model(),
            Err("Timeout must be a number of seconds.".to_owned())
        );
        tab.timeout.set_value("0");
        assert!(tab.to_model().is_err());
    }

    #[test]
    fn options_round_trip() {
        let request = RequestModel {
            options: Options {
                follow_redirects: false,
                verify_ssl: false,
                attach_cookies: false,
                proxy_url: "http://localhost:8080".into(),
                timeout: 12.5,
            },
            ..RequestModel::default()
        };
        let mut tab = OptionsTab::new();
        tab.load(&request);
        assert_eq!(tab.to_model().expect("valid options"), request.options);
    }
}
