use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget as _};
use rusting_core::files::{generate_file_stem, validate_directory, validate_file_name};
use rusting_core::model::REQUEST_SUFFIX;

use super::{Modal, ModalResult, centered, control, frame, percent};
use crate::theme;
use crate::widgets::editor::{Editor, EditorAction};
use crate::widgets::{Input, InputAction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRequestData {
    pub title: String,
    pub file_name: String,
    pub description: String,
    pub directory: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    FileName,
    Description,
    Directory,
    Create,
}

impl Field {
    const ALL: [Self; 5] = [
        Self::Title,
        Self::FileName,
        Self::Description,
        Self::Directory,
        Self::Create,
    ];
}

/// Creates or renames the on-disk identity of a request.
pub struct NewRequestModal {
    data: NewRequestData,
    title: Input,
    file_name: Input,
    description: Editor,
    directory: Input,
    focused: Field,
    file_name_edited: bool,
    title_error: Option<String>,
    file_name_error: Option<String>,
    directory_error: Option<String>,
}

impl NewRequestModal {
    /// The explicit `directory` is authoritative, including when `initial`
    /// contains a different directory from a request being duplicated.
    pub fn new(directory: String, initial: Option<NewRequestData>) -> Self {
        let initial = initial.unwrap_or(NewRequestData {
            title: String::new(),
            file_name: String::new(),
            description: String::new(),
            directory: String::new(),
        });
        let mut title = Input::with_placeholder("Enter a title");
        title.set_value(initial.title);

        let initial_stem = initial
            .file_name
            .strip_suffix(REQUEST_SUFFIX)
            .unwrap_or(&initial.file_name)
            .to_owned();
        let file_name_edited = !initial_stem.is_empty();
        let mut file_name = Input::with_placeholder("Generated from title");
        if file_name_edited {
            file_name.set_value(initial_stem);
        } else {
            file_name.set_value(generate_file_stem(title.value()));
        }

        let mut description = Editor::new();
        description.set_show_line_numbers(false);
        description.set_text(&initial.description);

        let mut directory_input = Input::with_placeholder("Path relative to collection root");
        directory_input.set_value(directory.clone());

        let mut modal = Self {
            data: NewRequestData {
                title: String::new(),
                file_name: String::new(),
                description: String::new(),
                directory,
            },
            title,
            file_name,
            description,
            directory: directory_input,
            focused: Field::Title,
            file_name_edited,
            title_error: None,
            file_name_error: None,
            directory_error: None,
        };
        modal.sync_data();
        modal
    }

    pub fn data(&self) -> &NewRequestData {
        &self.data
    }

    pub fn take(self) -> NewRequestData {
        self.data
    }

    fn sync_data(&mut self) {
        let file_name = self.resolved_file_name();
        let description = self.description.text();
        self.data.title = self.title.value().to_owned();
        self.data.file_name = file_name;
        self.data.description = description;
        self.data.directory = self.directory.value().to_owned();
    }

    fn resolved_file_name(&self) -> String {
        let entered = self.file_name.value().trim();
        let stem = if entered.is_empty() {
            generate_file_stem(self.title.value())
        } else {
            entered.to_owned()
        };
        if stem.ends_with(REQUEST_SUFFIX) {
            stem
        } else {
            format!("{stem}{REQUEST_SUFFIX}")
        }
    }

    fn regenerate_file_name(&mut self) {
        if !self.file_name_edited {
            self.file_name
                .set_value(generate_file_stem(self.title.value()));
        }
    }

    fn move_focus(&mut self, backwards: bool) {
        let current = Field::ALL
            .iter()
            .position(|field| *field == self.focused)
            .unwrap_or(0);
        let next = if backwards {
            (current + Field::ALL.len() - 1) % Field::ALL.len()
        } else {
            (current + 1) % Field::ALL.len()
        };
        self.focused = Field::ALL[next];
    }

    fn validate(&mut self) -> bool {
        self.sync_data();
        self.title_error = self
            .data
            .title
            .trim()
            .is_empty()
            .then(|| "Title cannot be empty.".to_owned());
        self.file_name_error = validate_file_name(&self.data.file_name)
            .err()
            .map(|error| error.to_string());
        self.directory_error = validate_directory(&self.data.directory)
            .err()
            .map(|error| error.to_string());
        self.title_error.is_none()
            && self.file_name_error.is_none()
            && self.directory_error.is_none()
    }

    fn accept_if_valid(&mut self) -> ModalResult {
        if self.validate() {
            ModalResult::Accepted
        } else {
            ModalResult::Open
        }
    }

    fn handle_input(&mut self, key: KeyEvent) -> ModalResult {
        if self.focused == Field::FileName
            && key.code == KeyCode::Char('a')
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.file_name.select_all();
            return ModalResult::Open;
        }
        let action = match self.focused {
            Field::Title => self.title.handle_key(key),
            Field::FileName => self.file_name.handle_key(key),
            Field::Directory => self.directory.handle_key(key),
            Field::Description | Field::Create => return ModalResult::Open,
        };
        match action {
            InputAction::Changed => {
                match self.focused {
                    Field::Title => self.regenerate_file_name(),
                    Field::FileName => self.file_name_edited = true,
                    Field::Directory | Field::Description | Field::Create => {}
                }
                self.sync_data();
                ModalResult::Open
            }
            InputAction::Submitted => self.accept_if_valid(),
            InputAction::LeaveUp => {
                self.move_focus(true);
                ModalResult::Open
            }
            InputAction::LeaveDown => {
                self.move_focus(false);
                ModalResult::Open
            }
            InputAction::Ignored | InputAction::Consumed => ModalResult::Open,
        }
    }
}

impl Modal for NewRequestModal {
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        if key.code == KeyCode::Esc {
            return ModalResult::Cancelled;
        }
        if key.code == KeyCode::Char('n')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && !key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return self.accept_if_valid();
        }
        if key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
        {
            self.move_focus(true);
            return ModalResult::Open;
        }
        if key.code == KeyCode::Tab {
            self.move_focus(false);
            return ModalResult::Open;
        }

        match self.focused {
            Field::Description => match self.description.handle_key(key) {
                EditorAction::Changed => {
                    self.sync_data();
                    ModalResult::Open
                }
                EditorAction::LeaveUp => {
                    self.move_focus(true);
                    ModalResult::Open
                }
                EditorAction::LeaveDown => {
                    self.move_focus(false);
                    ModalResult::Open
                }
                _ => ModalResult::Open,
            },
            Field::Create => match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => self.accept_if_valid(),
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_focus(true);
                    ModalResult::Open
                }
                _ => ModalResult::Open,
            },
            Field::Title | Field::FileName | Field::Directory => self.handle_input(key),
        }
    }

    fn render(&mut self, screen: Rect, buffer: &mut Buffer) {
        let width = percent(screen.width, 75).clamp(36, 82);
        let height = screen.height.clamp(12, 26);
        let area = centered(screen, width, height);
        let inner = frame("New request", area, buffer);
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(inner);

        render_label("Title", false, rows[0], buffer);
        let title_area = control(rows[1], buffer, self.focused == Field::Title);
        self.title
            .render(title_area, buffer, self.focused == Field::Title, &[]);
        render_error(self.title_error.as_deref(), rows[2], buffer);

        render_label("File name", true, rows[3], buffer);
        let file_area = control(rows[4], buffer, self.focused == Field::FileName);
        let suffix_width = REQUEST_SUFFIX.len().min(usize::from(file_area.width)) as u16;
        let file_parts = Layout::horizontal([Constraint::Min(1), Constraint::Length(suffix_width)])
            .split(file_area);
        self.file_name
            .render(file_parts[0], buffer, self.focused == Field::FileName, &[]);
        Paragraph::new(REQUEST_SUFFIX)
            .style(Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM))
            .render(file_parts[1], buffer);
        render_error(self.file_name_error.as_deref(), rows[5], buffer);

        render_label("Description", true, rows[6], buffer);
        let description_area = control(rows[7], buffer, self.focused == Field::Description);
        self.description
            .render(description_area, buffer, self.focused == Field::Description);

        render_label("Path in collection", false, rows[8], buffer);
        let directory_area = control(rows[9], buffer, self.focused == Field::Directory);
        self.directory.render(
            directory_area,
            buffer,
            self.focused == Field::Directory,
            &[],
        );
        render_error(self.directory_error.as_deref(), rows[10], buffer);

        let style = if self.focused == Field::Create {
            theme::selection()
        } else {
            Style::new().fg(theme::MUTED)
        };
        let button_area = control(rows[11], buffer, self.focused == Field::Create);
        Paragraph::new(Line::from(vec![
            Span::styled(" Create request ", style),
            Span::styled("[ctrl+n]", Style::new().fg(theme::MUTED)),
        ]))
        .style(style)
        .centered()
        .render(button_area, buffer);
    }
}

fn render_label(label: &str, optional: bool, area: Rect, buffer: &mut Buffer) {
    let mut spans = vec![Span::raw(label.to_owned())];
    if optional {
        spans.push(Span::styled(
            " optional",
            Style::new().fg(theme::MUTED).add_modifier(Modifier::DIM),
        ));
    }
    Paragraph::new(Line::from(spans)).render(area, buffer);
}

fn render_error(error: Option<&str>, area: Rect, buffer: &mut Buffer) {
    if let Some(error) = error {
        Paragraph::new(error)
            .style(Style::new().fg(theme::ERROR))
            .render(area, buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn explicit_directory_wins_over_initial_data() {
        let initial = NewRequestData {
            title: "Copy me".into(),
            file_name: "copy-me.posting.yaml".into(),
            description: "description".into(),
            directory: "old/path".into(),
        };
        let modal = NewRequestModal::new("new/path".into(), Some(initial));
        assert_eq!(modal.data().directory, "new/path");
        assert_eq!(modal.data().file_name, "copy-me.posting.yaml");
        assert_eq!(modal.data().title, "Copy me");
    }

    #[test]
    fn title_generates_a_file_name_until_the_user_edits_it() {
        let mut modal = NewRequestModal::new(".".into(), None);
        for character in "First title".chars() {
            modal.handle_key(key(KeyCode::Char(character)));
        }
        assert_eq!(modal.data().file_name, "first-title.posting.yaml");

        modal.handle_key(key(KeyCode::Tab));
        modal.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        for character in "custom".chars() {
            modal.handle_key(key(KeyCode::Char(character)));
        }
        modal.handle_key(key(KeyCode::BackTab));
        modal.handle_key(key(KeyCode::Char('!')));
        assert_eq!(modal.data().file_name, "custom.posting.yaml");
    }

    #[test]
    fn ctrl_n_accepts_only_valid_data() {
        let mut invalid = NewRequestModal::new(".".into(), None);
        let create = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(invalid.handle_key(create), ModalResult::Open);
        assert!(invalid.title_error.is_some());

        let initial = NewRequestData {
            title: "Get users".into(),
            file_name: String::new(),
            description: String::new(),
            directory: "ignored".into(),
        };
        let mut valid = NewRequestModal::new("api".into(), Some(initial));
        assert_eq!(valid.handle_key(create), ModalResult::Accepted);
        assert_eq!(valid.take().file_name, "get-users.posting.yaml");
    }

    #[test]
    fn filename_and_directory_errors_keep_the_modal_open() {
        let initial = NewRequestData {
            title: "Invalid target".into(),
            file_name: "bad/name".into(),
            description: String::new(),
            directory: "ignored".into(),
        };
        let mut modal = NewRequestModal::new("../escape".into(), Some(initial));
        let create = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
        assert_eq!(modal.handle_key(create), ModalResult::Open);
        assert_eq!(
            modal.file_name_error.as_deref(),
            Some("Name cannot contain a path separator.")
        );
        assert_eq!(
            modal.directory_error.as_deref(),
            Some("Path cannot escape the collection root.")
        );
    }

    #[test]
    fn escape_cancels_without_mutating_the_result_contract() {
        let mut modal = NewRequestModal::new(".".into(), None);
        assert_eq!(modal.handle_key(key(KeyCode::Esc)), ModalResult::Cancelled);
    }
}
