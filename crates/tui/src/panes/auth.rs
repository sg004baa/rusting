//! Request authentication controls.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Paragraph, Widget as _};
use rusting_core::model::{BearerTokenAuth, UserPassAuth};
use rusting_core::{Auth, AuthKind, RequestModel, Variables};

use crate::theme;
use crate::widgets::highlight;
use crate::widgets::input::{Input, InputAction};
use crate::widgets::select::{Select, SelectAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthAction {
    Ignored,
    Consumed,
    Changed,
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Kind,
    First,
    Secret,
}

pub struct AuthTab {
    kind: Select<Option<AuthKind>>,
    basic_username: Input,
    basic_password: Input,
    digest_username: Input,
    digest_password: Input,
    bearer_token: Input,
    focus: Focus,
    kind_area: Rect,
}

impl AuthTab {
    pub fn new() -> Self {
        let mut basic_password = Input::with_placeholder("Password");
        basic_password.password = true;
        let mut digest_password = Input::with_placeholder("Password");
        digest_password.password = true;
        let mut bearer_token = Input::with_placeholder("Token");
        bearer_token.password = true;
        Self {
            kind: Select::new(vec![
                ("No Auth".into(), None),
                ("Basic".into(), Some(AuthKind::Basic)),
                ("Digest".into(), Some(AuthKind::Digest)),
                ("Bearer Token".into(), Some(AuthKind::BearerToken)),
            ]),
            basic_username: Input::with_placeholder("Username"),
            basic_password,
            digest_username: Input::with_placeholder("Username"),
            digest_password,
            bearer_token,
            focus: Focus::Kind,
            kind_area: Rect::ZERO,
        }
    }

    pub fn load(&mut self, request: &RequestModel) {
        let auth = request.auth.clone().unwrap_or_default();
        self.kind.set_value(&auth.kind);
        let basic = auth.basic.unwrap_or_default();
        self.basic_username.set_value(basic.username);
        self.basic_password.set_value(basic.password);
        let digest = auth.digest.unwrap_or_default();
        self.digest_username.set_value(digest.username);
        self.digest_password.set_value(digest.password);
        self.bearer_token
            .set_value(auth.bearer_token.unwrap_or_default().token);
        self.focus = Focus::Kind;
        self.kind.close();
    }

    pub fn to_model(&self) -> Option<Auth> {
        let kind = *self.kind.value();
        kind?;
        let basic = (!self.basic_username.is_empty()
            || !self.basic_password.is_empty()
            || kind == Some(AuthKind::Basic))
        .then(|| UserPassAuth {
            username: self.basic_username.value().to_owned(),
            password: self.basic_password.value().to_owned(),
        });
        let digest = (!self.digest_username.is_empty()
            || !self.digest_password.is_empty()
            || kind == Some(AuthKind::Digest))
        .then(|| UserPassAuth {
            username: self.digest_username.value().to_owned(),
            password: self.digest_password.value().to_owned(),
        });
        let bearer_token = (!self.bearer_token.is_empty() || kind == Some(AuthKind::BearerToken))
            .then(|| BearerTokenAuth {
                token: self.bearer_token.value().to_owned(),
            });
        Some(Auth {
            kind,
            basic,
            digest,
            bearer_token,
        })
    }

    pub fn focus_first_control(&mut self) {
        self.focus = Focus::Kind;
        self.kind.close();
    }

    pub fn focus_last_control(&mut self) {
        self.kind.close();
        self.focus = match self.kind.value() {
            None => Focus::Kind,
            Some(AuthKind::Basic | AuthKind::Digest) => Focus::Secret,
            Some(AuthKind::BearerToken) => Focus::First,
        };
    }

    pub fn handle_key(&mut self, key: KeyEvent, variables: &Variables) -> AuthAction {
        if self.focus == Focus::Kind && key.code == KeyCode::Tab {
            self.kind.close();
            return if self.kind.value().is_none() {
                AuthAction::LeaveDown
            } else {
                self.focus = Focus::First;
                AuthAction::Consumed
            };
        }
        if self.focus == Focus::Kind && key.code == KeyCode::BackTab {
            self.kind.close();
            return AuthAction::LeaveUp;
        }

        match self.focus {
            Focus::Kind => match self.kind.handle_key(key) {
                SelectAction::Changed => AuthAction::Changed,
                SelectAction::Consumed => AuthAction::Consumed,
                SelectAction::LeaveUp => AuthAction::LeaveUp,
                SelectAction::LeaveDown => {
                    if self.kind.value().is_none() {
                        AuthAction::LeaveDown
                    } else {
                        self.focus = Focus::First;
                        AuthAction::Consumed
                    }
                }
                SelectAction::Ignored => AuthAction::Ignored,
            },
            Focus::First | Focus::Secret => self.handle_input(key, variables),
        }
    }

    fn handle_input(&mut self, key: KeyEvent, _variables: &Variables) -> AuthAction {
        if key.code == KeyCode::Tab {
            return self.move_down();
        }
        if key.code == KeyCode::BackTab {
            return self.move_up();
        }
        let action = match (self.kind.value(), self.focus) {
            (Some(AuthKind::Basic), Focus::First) => self.basic_username.handle_key(key),
            (Some(AuthKind::Basic), Focus::Secret) => self.basic_password.handle_key(key),
            (Some(AuthKind::Digest), Focus::First) => self.digest_username.handle_key(key),
            (Some(AuthKind::Digest), Focus::Secret) => self.digest_password.handle_key(key),
            (Some(AuthKind::BearerToken), _) => self.bearer_token.handle_key(key),
            (None, _) | (_, Focus::Kind) => return AuthAction::Consumed,
        };
        match action {
            InputAction::Changed => AuthAction::Changed,
            InputAction::Consumed | InputAction::Submitted => AuthAction::Consumed,
            InputAction::LeaveUp => self.move_up(),
            InputAction::LeaveDown => self.move_down(),
            InputAction::Ignored => AuthAction::Ignored,
        }
    }

    fn move_up(&mut self) -> AuthAction {
        self.focus = match self.focus {
            Focus::Kind => return AuthAction::LeaveUp,
            Focus::First => Focus::Kind,
            Focus::Secret => Focus::First,
        };
        AuthAction::Consumed
    }

    fn move_down(&mut self) -> AuthAction {
        self.focus = match (self.focus, self.kind.value()) {
            (Focus::Kind, _) => Focus::First,
            (Focus::First, Some(AuthKind::Basic | AuthKind::Digest)) => Focus::Secret,
            (Focus::First | Focus::Secret, _) => return AuthAction::LeaveDown,
        };
        AuthAction::Consumed
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
        let [kind_area, fields_area, description] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(6),
            Constraint::Min(1),
        ])
        .areas(area);
        self.kind_area = kind_area;
        let kind_focused = focused && self.focus == Focus::Kind;
        let inner = bordered(kind_area, buffer, kind_focused, Some("Authentication"));
        self.kind.render(inner, buffer, kind_focused);

        match self.kind.value() {
            None => {
                Line::from("No authentication selected.")
                    .centered()
                    .render(fields_area, buffer);
            }
            Some(AuthKind::Basic) => render_pair(
                &mut self.basic_username,
                &mut self.basic_password,
                fields_area,
                buffer,
                focused,
                self.focus,
                variables,
            ),
            Some(AuthKind::Digest) => render_pair(
                &mut self.digest_username,
                &mut self.digest_password,
                fields_area,
                buffer,
                focused,
                self.focus,
                variables,
            ),
            Some(AuthKind::BearerToken) => {
                let token_area = Rect::new(
                    fields_area.x,
                    fields_area.y,
                    fields_area.width,
                    3.min(fields_area.height),
                );
                let token_focused = focused && matches!(self.focus, Focus::First | Focus::Secret);
                let inner = bordered(token_area, buffer, token_focused, Some("Token"));
                let highlights = highlight::variables(
                    self.bearer_token.value(),
                    variables,
                    token_focused.then(|| self.bearer_token.cursor()),
                );
                self.bearer_token
                    .render(inner, buffer, token_focused, &highlights);
            }
        }
        Paragraph::new("Authorization headers will be generated when the request is sent.")
            .style(theme::placeholder())
            .centered()
            .render(description, buffer);
    }

    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        self.kind.render_overlay(self.kind_area, screen, buffer);
    }
}

impl Default for AuthTab {
    fn default() -> Self {
        Self::new()
    }
}

fn render_pair(
    username: &mut Input,
    password: &mut Input,
    area: Rect,
    buffer: &mut Buffer,
    focused: bool,
    focus: Focus,
    variables: &Variables,
) {
    let [user_area, pass_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Length(3)]).areas(area);
    let user_focused = focused && focus == Focus::First;
    let pass_focused = focused && focus == Focus::Secret;
    let user_inner = bordered(user_area, buffer, user_focused, Some("Username"));
    let pass_inner = bordered(pass_area, buffer, pass_focused, Some("Password"));
    let user_highlights = highlight::variables(
        username.value(),
        variables,
        user_focused.then(|| username.cursor()),
    );
    let pass_highlights = highlight::variables(
        password.value(),
        variables,
        pass_focused.then(|| password.cursor()),
    );
    username.render(user_inner, buffer, user_focused, &user_highlights);
    password.render(pass_inner, buffer, pass_focused, &pass_highlights);
}

fn bordered(area: Rect, buffer: &mut Buffer, focused: bool, title: Option<&str>) -> Rect {
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::border(focused));
    if let Some(title) = title {
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
    fn switching_auth_kind_preserves_other_credentials() {
        let request = RequestModel {
            auth: Some(Auth::basic("alice", "secret")),
            ..RequestModel::default()
        };
        let mut tab = AuthTab::new();
        tab.load(&request);
        tab.bearer_token.set_value("token");
        tab.kind.set_value(&Some(AuthKind::BearerToken));
        let bearer = tab.to_model().expect("bearer auth");
        assert_eq!(bearer.basic.expect("preserved basic").username, "alice");
        assert_eq!(bearer.bearer_token.expect("bearer").token, "token");
        tab.kind.set_value(&Some(AuthKind::Basic));
        assert_eq!(
            tab.to_model()
                .expect("basic")
                .basic
                .expect("payload")
                .password,
            "secret"
        );
    }
}
