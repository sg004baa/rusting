//! Request body selection, raw editor and form-data editor.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Widget as _};
use rusting_core::{BodyContent, RequestModel, Variables};

use crate::panes::key_value::{KeyValueAction, KeyValueEditor};
use crate::theme;
use crate::widgets::checkbox::{Checkbox, CheckboxAction};
use crate::widgets::editor::{Editor, EditorAction};
use crate::widgets::select::{Select, SelectAction};
use crate::widgets::syntax::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    None,
    Raw,
    Form,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyAction {
    Ignored,
    Consumed,
    Changed,
    OpenInPager,
    OpenInEditor,
    CopyRequested,
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterAction {
    Ignored,
    Consumed,
    Changed,
    LeaveUp,
    LeaveDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FooterFocus {
    Language,
    Wrap,
}

/// Shared status and editor controls used by request and response body editors.
pub struct EditorFooter {
    language: Select<Option<Language>>,
    wrap: Checkbox,
    focus: FooterFocus,
    language_area: Rect,
}

impl EditorFooter {
    pub const HEIGHT: u16 = 3;

    pub fn new() -> Self {
        Self {
            language: Select::new(vec![
                ("JSON".into(), Some(Language::Json)),
                ("HTML".into(), Some(Language::Html)),
                ("CSS".into(), Some(Language::Css)),
                ("Text".into(), None),
            ]),
            wrap: Checkbox::new("Wrap", false),
            focus: FooterFocus::Language,
            language_area: Rect::ZERO,
        }
    }

    pub fn sync_from_editor(&mut self, editor: &Editor) {
        if !self.language.is_open() {
            self.language.set_value(&editor.language());
        }
        self.wrap.checked = editor.soft_wrap();
    }

    pub fn handle_key(&mut self, key: KeyEvent, editor: &mut Editor) -> FooterAction {
        match self.focus {
            FooterFocus::Language => match self.language.handle_key(key) {
                SelectAction::Changed => {
                    editor.set_language(*self.language.value());
                    FooterAction::Changed
                }
                SelectAction::Consumed => FooterAction::Consumed,
                SelectAction::LeaveUp => FooterAction::LeaveUp,
                SelectAction::LeaveDown => {
                    self.focus = FooterFocus::Wrap;
                    FooterAction::Consumed
                }
                SelectAction::Ignored if key.code == KeyCode::Tab => {
                    self.focus = FooterFocus::Wrap;
                    FooterAction::Consumed
                }
                SelectAction::Ignored => FooterAction::Ignored,
            },
            FooterFocus::Wrap => match self.wrap.handle_key(key) {
                CheckboxAction::Toggled => {
                    editor.set_soft_wrap(self.wrap.checked);
                    FooterAction::Changed
                }
                CheckboxAction::Consumed => FooterAction::Consumed,
                CheckboxAction::LeaveUp => {
                    self.focus = FooterFocus::Language;
                    FooterAction::Consumed
                }
                CheckboxAction::LeaveDown => FooterAction::LeaveDown,
                CheckboxAction::Ignored if key.code == KeyCode::BackTab => {
                    self.focus = FooterFocus::Language;
                    FooterAction::Consumed
                }
                CheckboxAction::Ignored if key.code == KeyCode::Tab => FooterAction::LeaveDown,
                CheckboxAction::Ignored => FooterAction::Ignored,
            },
        }
    }

    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, focused: bool, editor: &Editor) {
        if area.is_empty() {
            return;
        }
        let control_width = 14.min(area.width / 3);
        let wrap_width = 12.min(area.width / 3);
        let [status_area, language_area, wrap_area] = Layout::horizontal([
            Constraint::Min(0),
            Constraint::Length(control_width),
            Constraint::Length(wrap_width),
        ])
        .areas(area);
        self.language_area = language_area;

        let (line, column) = editor.cursor_display();
        let mut spans = Vec::new();
        if editor.visual_mode() {
            spans.push(Span::styled("Visual", theme::selection()));
            spans.push(Span::raw("  "));
        }
        if editor.read_only() {
            spans.push(Span::styled("read-only", theme::disabled()));
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!("{line}:{column}"),
            theme::placeholder(),
        ));
        Line::from(spans).render(status_area, buffer);

        let language_focused = focused && self.focus == FooterFocus::Language;
        let language_inner = bordered(language_area, buffer, language_focused, None);
        self.language
            .render(language_inner, buffer, language_focused);
        let wrap_focused = focused && self.focus == FooterFocus::Wrap;
        let wrap_inner = bordered(wrap_area, buffer, wrap_focused, None);
        self.wrap.render(wrap_inner, buffer, wrap_focused);
    }

    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        self.language
            .render_overlay(self.language_area, screen, buffer);
    }
}

impl Default for EditorFooter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Kind,
    Content,
    Footer,
}

pub struct BodyTab {
    kind: Select<BodyKind>,
    raw: Editor,
    form: KeyValueEditor,
    footer: EditorFooter,
    focus: Focus,
    kind_area: Rect,
}

impl BodyTab {
    pub fn new() -> Self {
        let mut raw = Editor::new();
        raw.set_language(Some(Language::Json));
        let mut footer = EditorFooter::new();
        footer.sync_from_editor(&raw);
        Self {
            kind: Select::new(vec![
                ("None".into(), BodyKind::None),
                ("Raw (json, text, etc.)".into(), BodyKind::Raw),
                ("Form data (x-www-form-urlencoded)".into(), BodyKind::Form),
            ]),
            raw,
            form: KeyValueEditor::new(["Key", "Value"], "Add", "No form fields"),
            footer,
            focus: Focus::Kind,
            kind_area: Rect::ZERO,
        }
    }

    pub fn load(&mut self, request: &RequestModel) {
        match &request.body {
            None => {
                self.kind.set_value(&BodyKind::None);
                self.raw.set_text("");
                self.form.set_rows(Vec::new());
            }
            Some(BodyContent::Raw {
                content,
                content_type,
            }) => {
                self.kind.set_value(&BodyKind::Raw);
                self.raw.set_text(content);
                self.raw
                    .set_language(language_for_content_type(content_type.as_deref()));
                self.form.set_rows(Vec::new());
            }
            Some(BodyContent::Form { form_data, .. }) => {
                self.kind.set_value(&BodyKind::Form);
                self.form.set_rows(form_data.clone());
                self.raw.set_text("");
            }
        }
        self.footer.sync_from_editor(&self.raw);
        self.focus = Focus::Kind;
        self.kind.close();
    }

    pub fn to_model(&self) -> Option<BodyContent> {
        match self.kind.value() {
            BodyKind::None => None,
            BodyKind::Raw => Some(BodyContent::Raw {
                content: self.raw.text(),
                content_type: content_type_for_language(self.raw.language()).map(str::to_owned),
            }),
            BodyKind::Form => Some(BodyContent::Form {
                form_data: self.form.rows().to_vec(),
                content_type: Some(BodyContent::FORM_CONTENT_TYPE.to_owned()),
            }),
        }
    }

    pub fn has_content(&self) -> bool {
        match self.kind.value() {
            BodyKind::None => false,
            BodyKind::Raw => !self.raw.is_empty(),
            BodyKind::Form => !self.form.rows().is_empty(),
        }
    }

    pub fn is_editing(&self) -> bool {
        matches!(self.kind.value(), BodyKind::Form) && self.form.is_editing()
    }

    pub fn handle_key(&mut self, key: KeyEvent, variables: &Variables) -> BodyAction {
        match self.focus {
            Focus::Kind => match self.kind.handle_key(key) {
                SelectAction::Changed => BodyAction::Changed,
                SelectAction::Consumed => BodyAction::Consumed,
                SelectAction::LeaveUp => BodyAction::LeaveUp,
                SelectAction::LeaveDown => {
                    self.focus = Focus::Content;
                    BodyAction::Consumed
                }
                SelectAction::Ignored => BodyAction::Ignored,
            },
            Focus::Content => self.handle_content_key(key, variables),
            Focus::Footer => match self.footer.handle_key(key, &mut self.raw) {
                FooterAction::Ignored => BodyAction::Ignored,
                FooterAction::Consumed => BodyAction::Consumed,
                FooterAction::Changed => BodyAction::Changed,
                FooterAction::LeaveUp => {
                    self.focus = Focus::Content;
                    BodyAction::Consumed
                }
                FooterAction::LeaveDown => BodyAction::LeaveDown,
            },
        }
    }

    fn handle_content_key(&mut self, key: KeyEvent, variables: &Variables) -> BodyAction {
        match self.kind.value() {
            BodyKind::None => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.focus = Focus::Kind;
                    BodyAction::Consumed
                }
                KeyCode::Down | KeyCode::Char('j') => BodyAction::LeaveDown,
                _ => BodyAction::Ignored,
            },
            BodyKind::Raw => match self.raw.handle_key(key) {
                EditorAction::Ignored => BodyAction::Ignored,
                EditorAction::Consumed => BodyAction::Consumed,
                EditorAction::Changed => BodyAction::Changed,
                EditorAction::OpenInPager => BodyAction::OpenInPager,
                EditorAction::OpenInEditor => BodyAction::OpenInEditor,
                EditorAction::LeaveUp => {
                    self.focus = Focus::Kind;
                    BodyAction::Consumed
                }
                EditorAction::LeaveDown => {
                    self.focus = Focus::Footer;
                    BodyAction::Consumed
                }
            },
            BodyKind::Form => match self.form.handle_key(key, variables) {
                KeyValueAction::Ignored => BodyAction::Ignored,
                KeyValueAction::Consumed => BodyAction::Consumed,
                KeyValueAction::Changed => BodyAction::Changed,
                KeyValueAction::CopyRequested => BodyAction::CopyRequested,
                KeyValueAction::LeaveUp => {
                    self.focus = Focus::Kind;
                    BodyAction::Consumed
                }
                KeyValueAction::LeaveDown => BodyAction::LeaveDown,
            },
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
        let [kind_area, content_area] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);
        self.kind_area = kind_area;
        let kind_focused = focused && self.focus == Focus::Kind;
        let inner = bordered(kind_area, buffer, kind_focused, Some("Body type"));
        self.kind.render(inner, buffer, kind_focused);

        match self.kind.value() {
            BodyKind::None => Paragraph::new("No request body")
                .style(theme::placeholder())
                .centered()
                .render(content_area, buffer),
            BodyKind::Raw => {
                let [editor_area, footer_area] = Layout::vertical([
                    Constraint::Min(0),
                    Constraint::Length(EditorFooter::HEIGHT),
                ])
                .areas(content_area);
                self.raw
                    .render(editor_area, buffer, focused && self.focus == Focus::Content);
                self.footer.render(
                    footer_area,
                    buffer,
                    focused && self.focus == Focus::Footer,
                    &self.raw,
                );
            }
            BodyKind::Form => self.form.render(
                content_area,
                buffer,
                focused && self.focus == Focus::Content,
                variables,
            ),
        }
    }

    pub fn render_overlay(&mut self, screen: Rect, buffer: &mut Buffer) {
        self.kind.render_overlay(self.kind_area, screen, buffer);
        match self.kind.value() {
            BodyKind::Raw => self.footer.render_overlay(screen, buffer),
            BodyKind::Form => self.form.render_overlay(screen, buffer),
            BodyKind::None => {}
        }
    }

    pub(crate) fn selected_form_row(&self) -> Option<&rusting_core::KeyValue> {
        if *self.kind.value() == BodyKind::Form {
            self.form.table.selected()
        } else {
            None
        }
    }

    pub(crate) fn raw_text(&self) -> String {
        self.raw.text()
    }

    pub(crate) fn raw_language(&self) -> Option<Language> {
        self.raw.language()
    }

    pub(crate) fn apply_external_edit(&mut self, text: &str) -> bool {
        if *self.kind.value() != BodyKind::Raw {
            return false;
        }
        self.raw.set_text(text);
        true
    }
}

impl Default for BodyTab {
    fn default() -> Self {
        Self::new()
    }
}

fn language_for_content_type(content_type: Option<&str>) -> Option<Language> {
    let content_type = content_type?.split(';').next()?.trim();
    match content_type {
        "application/json" | "text/json" => Some(Language::Json),
        "text/html" | "application/xhtml+xml" => Some(Language::Html),
        "text/css" => Some(Language::Css),
        _ => None,
    }
}

fn content_type_for_language(language: Option<Language>) -> Option<&'static str> {
    match language {
        Some(Language::Json) => Some("application/json"),
        Some(Language::Html) => Some("text/html"),
        Some(Language::Css) => Some("text/css"),
        None => None,
    }
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
    fn all_body_modes_round_trip() {
        let mut tab = BodyTab::new();
        assert_eq!(tab.to_model(), None);

        let raw = RequestModel {
            body: Some(BodyContent::Raw {
                content: "{\"ok\":true}".into(),
                content_type: Some("application/json".into()),
            }),
            ..RequestModel::default()
        };
        tab.load(&raw);
        assert_eq!(tab.to_model(), raw.body);

        let form = RequestModel {
            body: Some(BodyContent::Form {
                form_data: vec![rusting_core::KeyValue::new("a", "1")],
                content_type: Some(BodyContent::FORM_CONTENT_TYPE.into()),
            }),
            ..RequestModel::default()
        };
        tab.load(&form);
        assert_eq!(tab.to_model(), form.body);
        assert!(tab.has_content());
    }
}
