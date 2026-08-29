use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use ::notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use anyhow::{Context as _, bail};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use rusting_core::{
    Collection, RequestModel, ScriptHook, ScriptRef,
    collection::{self, LoadFailure, load_request},
    config::{RequestOpenFocus, ResponseFocus, Settings},
    env::Environment,
    files,
};
use rusting_http::{
    Response,
    types::{PhaseEvent, Timings},
};
use rusting_script::{
    engine::Engine,
    types::{Effect, HookStatus, LogLine, Severity},
};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    external,
    focus::Focus,
    keymap::{Action, Keymap},
    layout::{self, Frames, Section},
    modals::{
        ConfirmModal, CopyModal, HelpModal, JumpOverlay, LoadEnvModal, Modal, ModalResult,
        NewRequestData, NewRequestModal, Palette, PaletteItem,
    },
    notify::Toasts,
    panes::{
        collection::{CollectionAction, CollectionPane},
        request::{RequestPane, RequestPaneAction, RequestTab},
        response::{ResponsePane, ResponsePaneAction, ResponseTab},
        url_bar::{UrlBar, UrlBarAction},
    },
    theme,
    widgets::{clipboard::Clipboard, syntax::Language},
};

struct SendFinished {
    generation: u64,
    result: Result<Response, rusting_http::SendError>,
}

struct PendingSend {
    generation: u64,
    request: RequestModel,
    statuses: [HookStatus; 3],
    logs: Vec<LogLine>,
}

enum TerminalMessage {
    Event(Event),
    Error(String),
}

enum WatchMessage {
    Changed(Vec<PathBuf>),
    Error(String),
}

#[derive(Debug, Clone)]
enum ConfirmPurpose {
    Delete(PathBuf),
}

#[derive(Debug)]
enum PalettePurpose {
    Commands(Vec<CommandChoice>),
    Search(Vec<PathBuf>),
}

#[derive(Debug, Clone)]
enum CommandChoice {
    Reset,
    ExpandRequest,
    ExpandResponse,
    ToggleCollection,
    LoadEnv,
    CopyYaml,
    EditScript {
        hook: ScriptHook,
        configured: String,
        reference: ScriptRef,
    },
    Quit,
}

enum ActiveModal {
    NewRequest {
        modal: Box<NewRequestModal>,
        template: Box<Option<RequestModel>>,
    },
    Confirm {
        modal: ConfirmModal,
        purpose: ConfirmPurpose,
    },
    Copy(CopyModal),
    Help(HelpModal),
    Jump(JumpOverlay),
    LoadEnv(LoadEnvModal),
    Palette {
        modal: Palette,
        purpose: PalettePurpose,
    },
}

impl ActiveModal {
    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match self {
            Self::NewRequest { modal, .. } => modal.handle_key(key),
            Self::Confirm { modal, .. } => modal.handle_key(key),
            Self::Copy(modal) => modal.handle_key(key),
            Self::Help(modal) => modal.handle_key(key),
            Self::Jump(modal) => modal.handle_key(key),
            Self::LoadEnv(modal) => modal.handle_key(key),
            Self::Palette { modal, .. } => modal.handle_key(key),
        }
    }

    fn render(&mut self, screen: Rect, buffer: &mut ratatui::buffer::Buffer) {
        match self {
            Self::NewRequest { modal, .. } => modal.render(screen, buffer),
            Self::Confirm { modal, .. } => modal.render(screen, buffer),
            Self::Copy(modal) => modal.render(screen, buffer),
            Self::Help(modal) => modal.render(screen, buffer),
            Self::Jump(modal) => modal.render(screen, buffer),
            Self::LoadEnv(modal) => modal.render(screen, buffer),
            Self::Palette { modal, .. } => modal.render(screen, buffer),
        }
    }
}

pub struct App {
    settings: Settings,
    environment: Environment,
    collection: Collection,
    keymap: Keymap,
    focus: Focus,
    sidebar_visible: bool,
    expanded: Option<Section>,
    collection_pane: CollectionPane,
    url_bar: UrlBar,
    request_pane: RequestPane,
    response_pane: ResponsePane,
    current: RequestModel,
    dirty: bool,
    modal: Option<ActiveModal>,
    toasts: Toasts,
    clipboard: Clipboard,
    script_engine: Engine,
    script_statuses: [HookStatus; 3],
    script_logs: Vec<LogLine>,
    progress_timings: Timings,
    send_generation: u64,
    send_task: Option<JoinHandle<()>>,
    pending_send: Option<PendingSend>,
    quit: bool,
    watcher_dirty: bool,
    event_pause: Arc<AtomicBool>,
    event_paused: Arc<AtomicBool>,
    event_reader_alive: Arc<AtomicBool>,
    post_external_repaint: bool,
}

impl App {
    pub fn new(
        settings: Settings,
        environment: Environment,
        collection: Collection,
        load_failures: Vec<LoadFailure>,
    ) -> anyhow::Result<Self> {
        let keymap = Keymap::new(&settings.keymap)?;
        let sidebar_visible = settings.collection_browser.show_on_startup;
        let focus = Focus::from_startup(settings.focus.on_startup, sidebar_visible);
        let root = collection.path.clone();
        let mut collection_pane = CollectionPane::new(&collection);
        let mut url_bar = UrlBar::new();
        url_bar.set_base_url_candidates(collection_pane.base_urls());
        let mut request_pane = RequestPane::new(root.clone());
        let response_pane = ResponsePane::new();
        let current = RequestModel::default();
        request_pane.load(&current);
        collection_pane.set_open(None);
        let mut toasts = Toasts::default();
        for failure in load_failures {
            toasts.push(
                format!(
                    "Could not load {}: {}",
                    failure.path.display(),
                    failure.message
                ),
                Severity::Error,
            );
        }
        let script_engine = Engine::new(root)?;
        Ok(Self {
            settings,
            environment,
            collection,
            keymap,
            focus,
            sidebar_visible,
            expanded: None,
            collection_pane,
            url_bar,
            request_pane,
            response_pane,
            current,
            dirty: false,
            modal: None,
            toasts,
            clipboard: Clipboard::default(),
            script_engine,
            script_statuses: std::array::from_fn(|_| HookStatus::NotConfigured),
            script_logs: Vec::new(),
            progress_timings: Timings::default(),
            send_generation: 0,
            send_task: None,
            pending_send: None,
            quit: false,
            watcher_dirty: true,
            event_pause: Arc::new(AtomicBool::new(false)),
            event_paused: Arc::new(AtomicBool::new(false)),
            event_reader_alive: Arc::new(AtomicBool::new(false)),
            post_external_repaint: false,
        })
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        let _terminal_guard = TerminalGuard::enter()?;
        let (terminal_tx, mut terminal_rx) = mpsc::unbounded_channel();
        let event_thread = EventThread::spawn(
            terminal_tx,
            Arc::clone(&self.event_pause),
            Arc::clone(&self.event_paused),
            Arc::clone(&self.event_reader_alive),
        );
        let (finished_tx, mut finished_rx) = mpsc::unbounded_channel();
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let (watch_tx, mut watch_rx) = mpsc::unbounded_channel();
        let mut watcher: Option<RecommendedWatcher> = None;
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))
            .context("could not initialize terminal backend")?;
        let mut redraw = tokio::time::interval(Duration::from_millis(50));
        redraw.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        while !self.quit {
            if self.watcher_dirty {
                watcher = self.build_watcher(watch_tx.clone())?;
                self.watcher_dirty = false;
            }
            repaint_after_external(&mut terminal, &mut self.post_external_repaint)?;
            terminal
                .draw(|frame| self.render(frame))
                .context("could not draw terminal UI")?;

            tokio::select! {
                message = terminal_rx.recv() => {
                    match message {
                        Some(TerminalMessage::Event(Event::Key(key))) => {
                            self.handle_key(key, &finished_tx, &progress_tx).await;
                        }
                        Some(TerminalMessage::Event(_)) => {}
                        Some(TerminalMessage::Error(error)) => {
                            self.toasts.push(error, Severity::Error);
                        }
                        None => bail!("terminal event reader stopped unexpectedly"),
                    }
                }
                finished = finished_rx.recv() => {
                    if let Some(finished) = finished {
                        self.finish_send(finished);
                    }
                }
                progress = progress_rx.recv() => {
                    if let Some((generation, phase)) = progress
                        && generation == self.send_generation
                    {
                        self.progress_timings.apply(phase);
                        self.url_bar.set_timings(&self.progress_timings);
                        self.response_pane.set_timings(&self.progress_timings);
                    }
                }
                message = watch_rx.recv() => {
                    match message {
                        Some(WatchMessage::Changed(paths)) => self.handle_files_changed(paths),
                        Some(WatchMessage::Error(error)) => {
                            self.toasts.push(format!("File watcher failed: {error}"), Severity::Error);
                        }
                        None => {}
                    }
                }
                _ = redraw.tick() => {
                    self.toasts.tick();
                }
            }
        }

        if let Some(task) = self.send_task.take() {
            task.abort();
        }
        drop(watcher);
        drop(event_thread);
        terminal
            .show_cursor()
            .context("could not show terminal cursor")?;
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame<'_>) {
        let screen = frame.area();
        let frames = layout::compute(
            screen,
            &self.settings,
            self.sidebar_visible,
            self.expanded,
            self.settings.url_bar.show_value_preview,
        );
        self.render_header_footer(frame, frames);
        let buffer = frame.buffer_mut();
        if let Some(sidebar) = frames.sidebar {
            self.collection_pane
                .render(sidebar, buffer, self.focus == Focus::Collection);
        }
        self.url_bar.render(
            frames.url_bar,
            buffer,
            matches!(self.focus, Focus::Method | Focus::Url | Focus::Send),
            &self.settings,
            self.environment.variables(),
            &self.current.path_params,
        );
        if let Some(area) = frames.request {
            self.request_pane.render(
                area,
                buffer,
                self.focus.request_section(),
                self.environment.variables(),
            );
        }
        if let Some(area) = frames.response {
            self.response_pane
                .render(area, buffer, self.focus.response_section(), &self.settings);
        }

        self.url_bar.render_overlay(screen, buffer);
        self.request_pane.render_overlay(screen, buffer);
        self.response_pane.render_overlay(screen, buffer);
        if let Some(modal) = &mut self.modal {
            modal.render(screen, buffer);
        }
        self.toasts.render(screen, buffer);
    }

    fn render_header_footer(&self, frame: &mut Frame<'_>, frames: Frames) {
        if let Some(area) = frames.header {
            let version = if self.settings.heading.show_version {
                format!(" {}", env!("CARGO_PKG_VERSION"))
            } else {
                String::new()
            };
            let left = Line::from(vec![
                Span::styled("rusting", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    version,
                    Style::default()
                        .fg(theme::MUTED)
                        .add_modifier(Modifier::DIM),
                ),
            ]);
            frame.render_widget(
                Paragraph::new(left),
                Rect::new(area.x, area.y + 1, area.width, 1),
            );
            if self.settings.heading.show_host {
                let user = std::env::var("USER")
                    .or_else(|_| std::env::var("LOGNAME"))
                    .unwrap_or_else(|_| "?".to_owned());
                let host = self
                    .settings
                    .heading
                    .hostname
                    .clone()
                    .or_else(|| std::env::var("HOSTNAME").ok())
                    .or_else(|| {
                        std::fs::read_to_string("/etc/hostname")
                            .ok()
                            .map(|value| value.trim().to_owned())
                            .filter(|value| !value.is_empty())
                    })
                    .unwrap_or_else(|| "?".to_owned());
                frame.render_widget(
                    Paragraph::new(format!("{user}@{host}")).alignment(Alignment::Right),
                    Rect::new(area.x, area.y + 1, area.width, 1),
                );
            }
        }

        let mut spans = Vec::new();
        for action in Action::ALL
            .into_iter()
            .filter(|action| action.show_in_footer())
        {
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                self.keymap.display(action),
                Style::default().fg(theme::ACCENT),
            ));
            spans.push(Span::raw(format!(" {}", action.description())));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), frames.footer);
    }

    async fn handle_key(
        &mut self,
        key: KeyEvent,
        finished_tx: &mpsc::UnboundedSender<SendFinished>,
        progress_tx: &mpsc::UnboundedSender<(u64, PhaseEvent)>,
    ) {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        let action = self.keymap.action_for(key);
        if action == Some(Action::Quit) {
            self.quit = true;
            return;
        }
        if self.modal.is_some() {
            self.handle_modal_key(key);
            return;
        }

        let local_first = key.modifiers.is_empty()
            && matches!(key.code, KeyCode::Char(_))
            && matches!(self.focus, Focus::Url | Focus::RequestBody);
        if local_first {
            let consumed = match self.focus {
                Focus::Url => self.handle_url_key(key, finished_tx, progress_tx).await,
                Focus::RequestBody => self.handle_request_key(key),
                _ => unreachable!(),
            };
            if consumed {
                return;
            }
            if let Some(action) = action {
                self.handle_global(action, key, finished_tx, progress_tx)
                    .await;
            }
            return;
        }

        if let Some(action) = action
            && self
                .handle_global(action, key, finished_tx, progress_tx)
                .await
        {
            return;
        }

        let forward = key.code == KeyCode::Tab && !key.modifiers.contains(KeyModifiers::SHIFT);
        let backward = key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT));
        match self.focus {
            Focus::Collection if forward => self.set_focus(self.focus.next(self.sidebar_visible)),
            Focus::Collection if backward => {
                self.set_focus(self.focus.previous(self.sidebar_visible));
            }
            Focus::Collection => self.handle_collection_key(key),
            Focus::Method | Focus::Url | Focus::Send => {
                let consumed = self.handle_url_key(key, finished_tx, progress_tx).await;
                if !consumed && forward {
                    self.set_focus(self.focus.next(self.sidebar_visible));
                } else if !consumed && backward {
                    self.set_focus(self.focus.previous(self.sidebar_visible));
                }
            }
            Focus::RequestTabs | Focus::RequestBody => {
                let consumed = self.handle_request_key(key);
                if !consumed && forward {
                    self.set_focus(self.focus.next(self.sidebar_visible));
                } else if !consumed && backward {
                    self.set_focus(self.focus.previous(self.sidebar_visible));
                }
            }
            Focus::ResponseTabs | Focus::ResponseBody => {
                let consumed = self.handle_response_key(key);
                if !consumed && forward {
                    self.set_focus(self.focus.next(self.sidebar_visible));
                } else if !consumed && backward {
                    self.set_focus(self.focus.previous(self.sidebar_visible));
                }
            }
        }
    }

    async fn handle_global(
        &mut self,
        action: Action,
        key: KeyEvent,
        finished_tx: &mpsc::UnboundedSender<SendFinished>,
        progress_tx: &mpsc::UnboundedSender<(u64, PhaseEvent)>,
    ) -> bool {
        match action {
            Action::Quit => self.quit = true,
            Action::SendRequest => self.start_send(finished_tx, progress_tx),
            Action::FocusMethod => {
                self.url_bar.focus_method();
                self.set_focus(Focus::Method);
            }
            Action::FocusUrl => {
                self.url_bar.focus_url();
                self.set_focus(Focus::Url);
            }
            Action::SaveRequest => self.save_or_prompt(),
            Action::NewRequest => {
                self.open_new_request(None, self.collection_pane.target_directory())
            }
            Action::ExpandSection => self.toggle_focused_section(),
            Action::ToggleCollection => self.toggle_collection_sidebar(),
            Action::SearchRequests => self.open_search_palette(),
            Action::Commands => self.open_command_palette(),
            Action::Jump => self.open_jump(),
            Action::Help => self.open_help(),
            Action::OpenInPager | Action::OpenInEditor => {
                self.handle_external_global(action, key);
            }
        }
        true
    }

    fn handle_modal_key(&mut self, key: KeyEvent) {
        let Some(mut modal) = self.modal.take() else {
            return;
        };
        match modal.handle_key(key) {
            ModalResult::Open => self.modal = Some(modal),
            ModalResult::Cancelled => {}
            ModalResult::Accepted => self.accept_modal(modal),
        }
    }

    fn accept_modal(&mut self, modal: ActiveModal) {
        match modal {
            ActiveModal::NewRequest { modal, template } => {
                self.create_request(modal.take(), *template);
            }
            ActiveModal::Confirm { purpose, .. } => match purpose {
                ConfirmPurpose::Delete(path) => self.delete_path(&path),
            },
            ActiveModal::Copy(modal) => {
                if let Some(text) = modal.text() {
                    self.copy_to_clipboard(&text);
                }
            }
            ActiveModal::Jump(modal) => {
                if let Some(target) = modal.taken() {
                    self.take_jump(target);
                }
            }
            ActiveModal::LoadEnv(modal) => {
                if let Some(path) = modal.path() {
                    self.load_env_file(path);
                } else {
                    self.toasts
                        .push("No environment file selected", Severity::Error);
                }
            }
            ActiveModal::Palette { modal, purpose } => {
                if let Some(chosen) = modal.chosen() {
                    self.accept_palette(chosen, purpose);
                }
            }
            ActiveModal::Help(_) => {}
        }
    }

    fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
        match focus {
            Focus::Method => self.url_bar.focus_method(),
            Focus::Url => self.url_bar.focus_url(),
            Focus::Send => self.url_bar.focus_send(),
            Focus::RequestTabs => self.request_pane.focus_tab_bar(),
            Focus::RequestBody => self.request_pane.focus_body(),
            Focus::ResponseTabs => self.response_pane.focus_tab_bar(),
            Focus::ResponseBody => self.response_pane.focus_body(),
            Focus::Collection => {}
        }
    }

    fn toggle_collection_sidebar(&mut self) {
        self.sidebar_visible = !self.sidebar_visible;
        if !self.sidebar_visible && self.focus == Focus::Collection {
            self.set_focus(Focus::Url);
        }
    }

    fn handle_collection_key(&mut self, key: KeyEvent) {
        match self.collection_pane.handle_key(key) {
            CollectionAction::Ignored | CollectionAction::Consumed => {}
            CollectionAction::Open(path) => self.open_path(&path),
            CollectionAction::NewRequest { parent } => self.open_new_request(None, parent),
            CollectionAction::Duplicate { path, quick } => self.duplicate_path(&path, quick),
            CollectionAction::Delete { path, confirm } => {
                if confirm {
                    self.modal = Some(ActiveModal::Confirm {
                        modal: ConfirmModal::new(
                            "Delete request",
                            format!("Delete {}?", path.display()),
                            "Delete",
                            "Cancel",
                        ),
                        purpose: ConfirmPurpose::Delete(path),
                    });
                } else {
                    self.delete_path(&path);
                }
            }
            CollectionAction::SearchRequested => self.open_search_palette(),
            CollectionAction::LeaveUp => self.set_focus(Focus::ResponseTabs),
            CollectionAction::LeaveDown => self.set_focus(Focus::Method),
        }
    }

    async fn handle_url_key(
        &mut self,
        key: KeyEvent,
        finished_tx: &mpsc::UnboundedSender<SendFinished>,
        progress_tx: &mpsc::UnboundedSender<(u64, PhaseEvent)>,
    ) -> bool {
        let action = self.url_bar.handle_key(key, self.environment.variables());
        let consumed = !matches!(action, UrlBarAction::Ignored);
        match action {
            UrlBarAction::Ignored | UrlBarAction::Consumed => {}
            UrlBarAction::Changed => {
                self.dirty = true;
                self.request_pane
                    .sync_path_params_from_url(self.url_bar.url());
            }
            UrlBarAction::Send => self.start_send(finished_tx, progress_tx),
            UrlBarAction::MethodChanged => self.dirty = true,
            UrlBarAction::CopyUrl => {
                let url = self.url_bar.url().to_owned();
                self.copy_to_clipboard(&url);
            }
            UrlBarAction::JumpToPathParam(_) => {
                self.request_pane.set_active_tab(RequestTab::Path);
                self.set_focus(Focus::RequestBody);
            }
            UrlBarAction::LeaveDown => self.set_focus(Focus::RequestTabs),
        }
        consumed
    }

    fn handle_request_key(&mut self, key: KeyEvent) -> bool {
        let action = self
            .request_pane
            .handle_key(key, self.environment.variables());
        let consumed = !matches!(action, RequestPaneAction::Ignored);
        self.handle_request_action(action);
        if self.focus.request_section() {
            self.focus = if self.request_pane.tab_bar_focused() {
                Focus::RequestTabs
            } else {
                Focus::RequestBody
            };
        }
        consumed
    }

    fn handle_request_action(&mut self, action: RequestPaneAction) {
        match action {
            RequestPaneAction::Ignored | RequestPaneAction::Consumed => {}
            RequestPaneAction::Changed => self.dirty = true,
            RequestPaneAction::OpenInPager(contents, language) => {
                self.page_contents(&contents, language);
            }
            RequestPaneAction::OpenInEditor(contents, language) => {
                self.edit_request_contents(&contents, language);
            }
            RequestPaneAction::OpenPathInPager(path) => self.page_path(&path),
            RequestPaneAction::OpenPathInEditor(path) => self.edit_script_path(&path),
            RequestPaneAction::CopyRequested => {
                if let Some(row) = self.request_pane.copy_target() {
                    self.modal = Some(ActiveModal::Copy(CopyModal::new(row)));
                } else {
                    self.toasts
                        .push("No copyable row is selected", Severity::Warning);
                }
            }
            RequestPaneAction::UrlRewrite(url) => {
                self.url_bar.set_url(&url);
                self.request_pane.sync_path_params_from_url(&url);
                self.dirty = true;
            }
            RequestPaneAction::JumpToUrlParam(_) => {
                self.url_bar.focus_url();
                self.set_focus(Focus::Url);
            }
            RequestPaneAction::LeaveUp => self.set_focus(Focus::Send),
            RequestPaneAction::LeaveDown => self.set_focus(Focus::ResponseTabs),
        }
    }

    fn handle_response_key(&mut self, key: KeyEvent) -> bool {
        let action = self.response_pane.handle_key(key);
        let consumed = !matches!(action, ResponsePaneAction::Ignored);
        match action {
            ResponsePaneAction::Ignored | ResponsePaneAction::Consumed => {}
            ResponsePaneAction::OpenInPager(contents, language) => {
                self.page_contents(&contents, language);
            }
            ResponsePaneAction::OpenInEditor(contents, language) => {
                self.edit_read_only_contents(&contents, language);
            }
            ResponsePaneAction::LeaveUp => {
                self.request_pane.focus_last_control();
                self.focus = Focus::RequestBody;
            }
            ResponsePaneAction::LeaveDown => {
                if self.sidebar_visible {
                    self.set_focus(Focus::Collection);
                } else {
                    self.set_focus(Focus::Method);
                }
            }
        }
        if self.focus.response_section() {
            self.focus = if self.response_pane.tab_bar_focused() {
                Focus::ResponseTabs
            } else {
                Focus::ResponseBody
            };
        }
        consumed
    }

    fn handle_external_global(&mut self, action: Action, key: KeyEvent) {
        match self.focus {
            Focus::Url if action == Action::OpenInEditor => {
                let current = self.url_bar.url().to_owned();
                let Some(command) = self.settings.editor.clone() else {
                    self.toasts.push("No editor is configured", Severity::Error);
                    return;
                };
                match self.run_external(|| external::edit_in_external(&command, &current, None)) {
                    Ok(edited) => {
                        self.url_bar.set_url(edited.trim_end_matches(['\r', '\n']));
                        self.request_pane
                            .sync_path_params_from_url(self.url_bar.url());
                        self.dirty = true;
                    }
                    Err(error) => self
                        .toasts
                        .push(format!("Editor failed: {error:#}"), Severity::Error),
                }
            }
            Focus::RequestTabs | Focus::RequestBody => {
                let pane_action = self
                    .request_pane
                    .handle_key(key, self.environment.variables());
                self.handle_request_action(pane_action);
            }
            Focus::ResponseTabs | Focus::ResponseBody => {
                let _ = self.handle_response_key(key);
            }
            _ => {}
        }
    }

    fn edit_request_contents(&mut self, contents: &str, language: Option<Language>) {
        let Some(command) = self.settings.editor.clone() else {
            self.toasts.push("No editor is configured", Severity::Error);
            return;
        };
        let result = self.run_external(|| {
            external::edit_in_external(&command, contents, language.map(Language::extension))
        });
        match result {
            Ok(edited) => match self.request_pane.apply_external_edit(&edited) {
                Ok(()) => self.dirty = true,
                Err(error) => self.toasts.push(error, Severity::Error),
            },
            Err(error) => self
                .toasts
                .push(format!("Editor failed: {error:#}"), Severity::Error),
        }
    }

    fn edit_read_only_contents(&mut self, contents: &str, language: Option<Language>) {
        let Some(command) = self.settings.editor.clone() else {
            self.toasts.push("No editor is configured", Severity::Error);
            return;
        };
        let result = self.run_external(|| {
            external::edit_in_external(&command, contents, language.map(Language::extension))
        });
        if let Err(error) = result {
            self.toasts
                .push(format!("Editor failed: {error:#}"), Severity::Error);
        }
    }

    fn page_contents(&mut self, contents: &str, language: Option<Language>) {
        let language_name = language.map(Language::name);
        let Some(command) = self.settings.pager_for(language_name).map(str::to_owned) else {
            self.toasts.push("No pager is configured", Severity::Error);
            return;
        };
        let result = self.run_external(|| {
            external::view_in_pager(&command, contents, language.map(Language::extension))
        });
        if let Err(error) = result {
            self.toasts
                .push(format!("Pager failed: {error:#}"), Severity::Error);
        }
    }

    fn edit_script_path(&mut self, path: &Path) {
        let Some(command) = self.settings.editor.clone() else {
            self.toasts.push("No editor is configured", Severity::Error);
            return;
        };
        self.run_script_editor(&command, path);
    }

    fn edit_configured_script(&mut self, hook: ScriptHook, reference: &ScriptRef) {
        let Some(command) = self.settings.editor.clone() else {
            self.toasts.push("No editor is configured", Severity::Error);
            return;
        };
        let (path, created) = match prepare_script_for_edit(&self.collection.path, hook, reference)
        {
            Ok(prepared) => prepared,
            Err(error) => {
                self.toasts
                    .push(format!("Editor failed: {error:#}"), Severity::Error);
                return;
            }
        };
        if created {
            self.request_pane.refresh_script_candidates();
        }
        self.run_script_editor(&command, &path);
    }

    fn run_script_editor(&mut self, command: &str, path: &Path) {
        match self.run_external(|| external::edit_path_in_external(command, path)) {
            Ok(()) => {
                self.script_engine.invalidate(path);
                self.request_pane.refresh_script_candidates();
                self.toasts
                    .push(format!("Edited {}", path.display()), Severity::Information);
            }
            Err(error) => self
                .toasts
                .push(format!("Editor failed: {error:#}"), Severity::Error),
        }
    }

    fn page_path(&mut self, path: &Path) {
        let Some(command) = self.settings.pager_for(None).map(str::to_owned) else {
            self.toasts.push("No pager is configured", Severity::Error);
            return;
        };
        if let Err(error) = self.run_external(|| external::view_path_in_pager(&command, path)) {
            self.toasts
                .push(format!("Pager failed: {error:#}"), Severity::Error);
        }
    }

    fn run_external<T>(
        &mut self,
        operation: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.event_pause.store(true, Ordering::Release);
        while self.event_reader_alive.load(Ordering::Acquire)
            && !self.event_paused.load(Ordering::Acquire)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        let result = operation();
        self.event_pause.store(false, Ordering::Release);
        while self.event_reader_alive.load(Ordering::Acquire)
            && self.event_paused.load(Ordering::Acquire)
        {
            std::thread::yield_now();
        }
        self.post_external_repaint = true;
        result
    }

    fn toggle_focused_section(&mut self) {
        let target = if self.focus.response_section() {
            Section::Response
        } else {
            Section::Request
        };
        self.expanded = if self.expanded == Some(target) {
            None
        } else {
            Some(target)
        };
    }

    fn open_path(&mut self, path: &Path) {
        match load_request(path) {
            Ok(request) => self.load_model(request),
            Err(error) => self.toasts.push(
                format!("Could not open {}: {error:#}", path.display()),
                Severity::Error,
            ),
        }
    }

    /// The app writes the open request itself (manual save, auto-save on response),
    /// and the watcher reports that write. Re-loading byte-identical content would
    /// clear the response pane and abort an in-flight send, so only genuine external
    /// edits are applied.
    fn reopen_if_changed(&mut self, path: &Path) {
        match load_request(path) {
            Ok(mut request) => {
                // Runtime cookies are serde-skipped and must not affect persisted equality.
                let cookies = std::mem::take(&mut self.current.cookies);
                if request != self.current {
                    request.cookies = cookies;
                    self.load_model(request);
                } else {
                    self.current.cookies = cookies;
                }
            }
            Err(error) => self.toasts.push(
                format!("Could not reload {}: {error:#}", path.display()),
                Severity::Error,
            ),
        }
    }

    fn load_model(&mut self, request: RequestModel) {
        self.current = request;
        self.url_bar.set_method(self.current.method);
        self.url_bar.set_url(&self.current.url);
        self.url_bar.clear_response();
        self.request_pane.load(&self.current);
        self.response_pane.clear();
        if let Some(task) = self.send_task.take() {
            task.abort();
        }
        self.pending_send = None;
        self.send_generation = self.send_generation.wrapping_add(1);
        self.progress_timings = Timings::default();
        self.url_bar.set_timings(&self.progress_timings);
        self.script_statuses = std::array::from_fn(|_| HookStatus::NotConfigured);
        self.script_logs.clear();
        self.collection_pane.set_open(self.current.path.clone());
        self.dirty = false;
        match self.settings.focus.on_request_open {
            Some(RequestOpenFocus::Headers) => {
                self.request_pane.set_active_tab(RequestTab::Headers);
                self.set_focus(Focus::RequestBody);
            }
            Some(RequestOpenFocus::Body) => {
                self.request_pane.set_active_tab(RequestTab::Body);
                self.set_focus(Focus::RequestBody);
            }
            Some(RequestOpenFocus::Query) => {
                self.request_pane.set_active_tab(RequestTab::Query);
                self.set_focus(Focus::RequestBody);
            }
            Some(RequestOpenFocus::Info) => {
                self.request_pane.set_active_tab(RequestTab::Info);
                self.set_focus(Focus::RequestBody);
            }
            Some(RequestOpenFocus::Path) => {
                self.request_pane.set_active_tab(RequestTab::Path);
                self.set_focus(Focus::RequestBody);
            }
            Some(RequestOpenFocus::Url) => self.set_focus(Focus::Url),
            Some(RequestOpenFocus::Method) => self.set_focus(Focus::Method),
            None => {}
        }
    }

    fn model_from_ui(&self) -> Result<RequestModel, String> {
        let mut base = self.current.clone();
        base.method = self.url_bar.method();
        base.url = self.url_bar.url().to_owned();
        self.request_pane.to_model(&base)
    }

    fn save_or_prompt(&mut self) {
        let model = match self.model_from_ui() {
            Ok(model) => model,
            Err(error) => {
                self.toasts.push(error, Severity::Error);
                return;
            }
        };
        if model.path.is_none() {
            self.open_new_request(Some(model), self.collection_pane.target_directory());
            return;
        }
        match collection::save_request(&model) {
            Ok(()) => {
                self.current = model;
                self.dirty = false;
                self.reload_collection();
                self.toasts.push("Request saved", Severity::Information);
            }
            Err(error) => self
                .toasts
                .push(format!("Save failed: {error:#}"), Severity::Error),
        }
    }

    fn open_new_request(&mut self, template: Option<RequestModel>, directory: PathBuf) {
        let relative = directory
            .strip_prefix(&self.collection.path)
            .unwrap_or(Path::new("."))
            .to_string_lossy()
            .into_owned();
        let initial = template.as_ref().map(|request| NewRequestData {
            title: request.name.clone(),
            file_name: request
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            description: request.description.clone(),
            directory: relative.clone(),
        });
        self.modal = Some(ActiveModal::NewRequest {
            modal: Box::new(NewRequestModal::new(relative, initial)),
            template: Box::new(template),
        });
    }

    fn create_request(&mut self, data: NewRequestData, template: Option<RequestModel>) {
        let directory = if data.directory == "." {
            self.collection.path.clone()
        } else {
            self.collection.path.join(&data.directory)
        };
        let file_name = files::unique_file_name(&directory, &data.file_name);
        let path = directory.join(file_name);
        let mut request = template.unwrap_or_default();
        request.name = data.title;
        request.description = data.description;
        request.path = Some(path.clone());
        match collection::save_request(&request) {
            Ok(()) => {
                self.reload_collection();
                self.load_model(request);
                self.toasts
                    .push(format!("Created {}", path.display()), Severity::Information);
            }
            Err(error) => self
                .toasts
                .push(format!("Create failed: {error:#}"), Severity::Error),
        }
    }

    fn duplicate_path(&mut self, path: &Path, quick: bool) {
        let request = match load_request(path) {
            Ok(request) => request,
            Err(error) => {
                self.toasts
                    .push(format!("Duplicate failed: {error:#}"), Severity::Error);
                return;
            }
        };
        let directory = path.parent().unwrap_or(&self.collection.path).to_path_buf();
        if !quick {
            self.open_new_request(Some(request), directory);
            return;
        }
        let desired = files::generate_file_name(&request.name);
        let file_name = files::unique_file_name(&directory, &desired);
        let mut duplicate = request;
        duplicate.path = Some(directory.join(file_name));
        match collection::save_request(&duplicate) {
            Ok(()) => {
                self.reload_collection();
                self.load_model(duplicate);
                self.toasts
                    .push("Request duplicated", Severity::Information);
            }
            Err(error) => self
                .toasts
                .push(format!("Duplicate failed: {error:#}"), Severity::Error),
        }
    }

    fn delete_path(&mut self, path: &Path) {
        let request = match load_request(path) {
            Ok(request) => request,
            Err(error) => {
                self.toasts
                    .push(format!("Delete failed: {error:#}"), Severity::Error);
                return;
            }
        };
        match collection::delete_request(&request) {
            Ok(()) => {
                if self.current.path.as_deref() == Some(path) {
                    self.load_model(RequestModel::default());
                }
                self.reload_collection();
                self.toasts.push("Request deleted", Severity::Information);
            }
            Err(error) => self
                .toasts
                .push(format!("Delete failed: {error:#}"), Severity::Error),
        }
    }

    fn reload_collection(&mut self) {
        match Collection::from_directory(&self.collection.path) {
            Ok(loaded) => {
                self.collection = loaded.collection;
                self.collection_pane.reload(&self.collection);
                self.collection_pane.set_open(self.current.path.clone());
                self.url_bar
                    .set_base_url_candidates(self.collection_pane.base_urls());
                for failure in loaded.failures {
                    self.toasts.push(
                        format!(
                            "Could not load {}: {}",
                            failure.path.display(),
                            failure.message
                        ),
                        Severity::Error,
                    );
                }
            }
            Err(error) => self.toasts.push(
                format!("Collection reload failed: {error:#}"),
                Severity::Error,
            ),
        }
    }

    fn start_send(
        &mut self,
        finished_tx: &mpsc::UnboundedSender<SendFinished>,
        progress_tx: &mpsc::UnboundedSender<(u64, PhaseEvent)>,
    ) {
        if let Some(task) = self.send_task.take() {
            task.abort();
        }
        self.pending_send = None;
        let mut request = match self.model_from_ui() {
            Ok(request) => request,
            Err(error) => {
                self.toasts.push(error, Severity::Error);
                return;
            }
        };
        let mut statuses = std::array::from_fn(|_| HookStatus::NotConfigured);
        let mut logs = Vec::new();

        if let Some(script) = request.script_ref(ScriptHook::Setup) {
            let outcome = self
                .script_engine
                .run_setup(&script, self.environment.variables());
            statuses[0] = outcome.status.clone();
            logs.extend(outcome.logs.clone());
            self.apply_effects(outcome.effects);
            if self.hook_failed(&outcome.status, "Setup") {
                self.update_script_output(statuses, logs);
                return;
            }
        }

        if let Err(error) =
            rusting_core::template::apply(&mut request, self.environment.variables())
        {
            self.toasts.push(
                format!("Could not resolve {}: {}", error.field, error.source),
                Severity::Error,
            );
            self.update_script_output(statuses, logs);
            return;
        }

        if let Some(script) = request.script_ref(ScriptHook::OnRequest) {
            let outcome = self.script_engine.run_on_request(
                &script,
                &mut request,
                self.environment.variables(),
            );
            statuses[1] = outcome.status.clone();
            logs.extend(outcome.logs.clone());
            self.apply_effects(outcome.effects);
            if self.hook_failed(&outcome.status, "Pre-request") {
                self.update_script_output(statuses, logs);
                return;
            }
        }

        self.send_generation = self.send_generation.wrapping_add(1);
        let generation = self.send_generation;
        self.progress_timings = Timings::default();
        self.url_bar.set_timings(&self.progress_timings);
        self.response_pane.set_timings(&self.progress_timings);
        self.pending_send = Some(PendingSend {
            generation,
            request: request.clone(),
            statuses,
            logs,
        });

        let ssl = self.settings.ssl.clone();
        let cookies = self.current.cookies.clone();
        let finished_tx = finished_tx.clone();
        let progress_tx = progress_tx.clone();
        self.send_task = Some(tokio::spawn(async move {
            let (request_progress_tx, mut request_progress_rx) = mpsc::unbounded_channel();
            let send =
                rusting_http::send::send(&request, &ssl, &cookies, Some(request_progress_tx));
            tokio::pin!(send);
            loop {
                tokio::select! {
                    result = &mut send => {
                        let _ = finished_tx.send(SendFinished { generation, result });
                        break;
                    }
                    Some(phase) = request_progress_rx.recv() => {
                        let _ = progress_tx.send((generation, phase));
                    }
                }
            }
        }));
    }

    fn finish_send(&mut self, finished: SendFinished) {
        let Some(mut pending) = self.pending_send.take() else {
            return;
        };
        if finished.generation != pending.generation {
            self.pending_send = Some(pending);
            return;
        }
        self.send_task = None;
        match finished.result {
            Ok(response) => {
                pending.request.cookies = response.cookies.clone();
                if let Some(script) = pending.request.script_ref(ScriptHook::OnResponse) {
                    let outcome = self.script_engine.run_on_response(
                        &script,
                        &response,
                        &pending.request,
                        self.environment.variables(),
                    );
                    pending.statuses[2] = outcome.status.clone();
                    pending.logs.extend(outcome.logs);
                    self.apply_effects(outcome.effects);
                    self.hook_failed(&outcome.status, "Post-response");
                }
                self.current.cookies = response.cookies.clone();
                self.progress_timings = response.timings.clone();
                self.url_bar.set_response(response.status, &response.reason);
                self.url_bar.set_timings(&response.timings);
                self.response_pane.set_response(&response, &self.settings);
                self.response_pane.set_timings(&response.timings);
                self.update_script_output(pending.statuses, pending.logs);
                if self.settings.auto_save_on_response && self.current.path.is_some() {
                    self.save_or_prompt();
                }
                match self.settings.focus.on_response {
                    Some(ResponseFocus::Body) => self.set_focus(Focus::ResponseBody),
                    Some(ResponseFocus::Tabs) => self.set_focus(Focus::ResponseTabs),
                    None => {}
                }
                self.toasts.push(
                    format!("{} {}", response.status, response.reason),
                    Severity::Information,
                );
            }
            Err(error) => {
                self.apply_failed_send_state();
                self.toasts
                    .push(format!("Request failed: {error}"), Severity::Error);
                self.update_script_output(pending.statuses, pending.logs);
            }
        }
    }

    fn apply_failed_send_state(&mut self) {
        self.url_bar.clear_response();
        self.response_pane.clear();
        self.url_bar.set_timings(&self.progress_timings);
        self.response_pane.set_timings(&self.progress_timings);
    }

    fn hook_failed(&mut self, status: &HookStatus, label: &str) -> bool {
        if let HookStatus::Error(error) = status {
            self.toasts
                .push(format!("{label} script failed: {error}"), Severity::Error);
            true
        } else {
            false
        }
    }

    fn apply_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::SetVariable { name, value } => {
                    self.environment.set_session_variable(name, value);
                }
                Effect::ClearVariable { name } => self.environment.clear_session_variable(&name),
                Effect::ClearAllVariables => self.environment.clear_session(),
                Effect::Notify { message, severity } => self.toasts.push(message, severity),
            }
        }
    }

    fn update_script_output(&mut self, statuses: [HookStatus; 3], logs: Vec<LogLine>) {
        self.script_statuses = statuses;
        self.script_logs = logs;
        self.response_pane.set_script_output(
            [
                ("Setup", self.script_statuses[0].clone()),
                ("Pre-request", self.script_statuses[1].clone()),
                ("Post-response", self.script_statuses[2].clone()),
            ],
            &self.script_logs,
        );
    }

    fn open_help(&mut self) {
        let title = match self.focus {
            Focus::Collection => "Collection",
            Focus::Method => "Method",
            Focus::Url => "URL",
            Focus::Send => "Send",
            Focus::RequestTabs | Focus::RequestBody => "Request",
            Focus::ResponseTabs | Focus::ResponseBody => "Response",
        };
        let description = match self.focus {
            Focus::Collection => "Browse, open, duplicate, and delete requests in the collection.",
            Focus::Method => "Choose the HTTP method. Mnemonic keys select a method directly.",
            Focus::Url => "Edit the request URL. Variables and :path parameters are highlighted.",
            Focus::Send => "Send the current request.",
            Focus::RequestTabs | Focus::RequestBody => {
                "Edit request headers, body, authentication, scripts, and options."
            }
            Focus::ResponseTabs | Focus::ResponseBody => {
                "Inspect the response, script output, timings, and sent request."
            }
        };
        let bindings = Action::ALL
            .into_iter()
            .map(|action| (self.keymap.display(action), action.description().to_owned()))
            .collect();
        self.modal = Some(ActiveModal::Help(HelpModal::new(
            title.to_owned(),
            description,
            bindings,
        )));
    }

    fn open_jump(&mut self) {
        let mut targets = self.url_bar.jump_targets();
        if self.sidebar_visible {
            targets.extend(self.collection_pane.jump_targets());
        }
        targets.extend(self.request_pane.jump_targets());
        targets.extend(self.response_pane.jump_targets());
        self.modal = Some(ActiveModal::Jump(JumpOverlay::new(targets)));
    }

    fn take_jump(&mut self, target: char) {
        match target {
            '1' => self.set_focus(Focus::Method),
            '2' => self.set_focus(Focus::Url),
            '\t' if self.sidebar_visible => self.set_focus(Focus::Collection),
            key => {
                if let Some(tab) = RequestTab::ALL
                    .into_iter()
                    .find(|tab| tab.jump_key() == key)
                {
                    self.request_pane.set_active_tab(tab);
                    self.set_focus(Focus::RequestTabs);
                } else if let Some(tab) = ResponseTab::ALL
                    .into_iter()
                    .find(|tab| tab.jump_key() == key)
                {
                    self.response_pane.set_active_tab(tab);
                    self.set_focus(Focus::ResponseTabs);
                }
            }
        }
    }

    fn open_command_palette(&mut self) {
        let mut choices = Vec::new();
        if self.expanded.is_some() {
            choices.push(CommandChoice::Reset);
        }
        choices.extend([
            CommandChoice::ExpandRequest,
            CommandChoice::ExpandResponse,
            CommandChoice::ToggleCollection,
            CommandChoice::LoadEnv,
            CommandChoice::CopyYaml,
        ]);
        choices.extend(self.request_pane.configured_script_hooks().into_iter().map(
            |(hook, configured, reference)| CommandChoice::EditScript {
                hook,
                configured,
                reference,
            },
        ));
        choices.push(CommandChoice::Quit);
        let items = command_palette_items(&choices);
        self.modal = Some(ActiveModal::Palette {
            modal: Palette::new("Type a command", items),
            purpose: PalettePurpose::Commands(choices),
        });
    }

    fn open_search_palette(&mut self) {
        let requests = self.collection.requests_recursive();
        let mut paths = Vec::new();
        let mut items = Vec::new();
        for request in requests {
            let Some(path) = request.path.clone() else {
                continue;
            };
            let id = paths.len();
            paths.push(path);
            items.push(PaletteItem {
                label: request.name.clone(),
                hint: Some(request.method.as_str().to_owned()),
                search_extra: Some(request.url.clone()),
                id,
            });
        }
        self.modal = Some(ActiveModal::Palette {
            modal: Palette::new("Search requests", items),
            purpose: PalettePurpose::Search(paths),
        });
    }

    fn accept_palette(&mut self, chosen: usize, purpose: PalettePurpose) {
        match purpose {
            PalettePurpose::Commands(choices) => {
                if let Some(choice) = choices.get(chosen).cloned() {
                    match choice {
                        CommandChoice::Reset => self.expanded = None,
                        CommandChoice::ExpandRequest => self.expanded = Some(Section::Request),
                        CommandChoice::ExpandResponse => self.expanded = Some(Section::Response),
                        CommandChoice::ToggleCollection => self.toggle_collection_sidebar(),
                        CommandChoice::LoadEnv => self.open_load_env(),
                        CommandChoice::CopyYaml => match self.model_from_ui() {
                            Ok(model) => match rusting_core::yaml::to_string(&model) {
                                Ok(yaml) => self.copy_to_clipboard(&yaml),
                                Err(error) => self.toasts.push(
                                    format!("Could not serialize request: {error}"),
                                    Severity::Error,
                                ),
                            },
                            Err(error) => self.toasts.push(error, Severity::Error),
                        },
                        CommandChoice::EditScript {
                            hook, reference, ..
                        } => self.edit_configured_script(hook, &reference),
                        CommandChoice::Quit => self.quit = true,
                    }
                }
            }
            PalettePurpose::Search(paths) => {
                if let Some(path) = paths.get(chosen) {
                    let path = path.clone();
                    self.open_path(&path);
                }
            }
        }
    }

    fn open_load_env(&mut self) {
        match (
            std::env::current_dir().context("could not determine current directory"),
            rusting_core::locations::config_directory(),
        ) {
            (Ok(working_directory), Ok(config_directory)) => {
                self.modal = Some(ActiveModal::LoadEnv(LoadEnvModal::new(
                    working_directory,
                    config_directory,
                )));
            }
            (Err(error), _) | (_, Err(error)) => {
                self.toasts.push(
                    format!("Could not open env loader: {error:#}"),
                    Severity::Error,
                );
            }
        }
    }

    fn load_env_file(&mut self, path: PathBuf) {
        if !path.is_file() {
            self.toasts.push(
                format!("Environment file does not exist: {}", path.display()),
                Severity::Error,
            );
            return;
        }
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                self.toasts.push(
                    format!("Could not resolve {}: {error}", path.display()),
                    Severity::Error,
                );
                return;
            }
        };
        let mut files = self.environment.files.clone();
        if !files.contains(&path) {
            files.push(path.clone());
        }
        match self.environment.set_files(files) {
            Ok(()) => {
                self.watcher_dirty = true;
                self.toasts
                    .push(format!("Loaded {}", path.display()), Severity::Information);
            }
            Err(error) => self.toasts.push(
                format!("Could not load env file: {error:#}"),
                Severity::Error,
            ),
        }
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        match self.clipboard.set_text(text.to_owned()) {
            Ok(()) => self
                .toasts
                .push("Copied to clipboard", Severity::Information),
            Err(error) => self
                .toasts
                .push(format!("Clipboard failed: {error}"), Severity::Error),
        }
    }

    fn build_watcher(
        &self,
        tx: mpsc::UnboundedSender<WatchMessage>,
    ) -> anyhow::Result<Option<RecommendedWatcher>> {
        if !self.settings.watch_env_files && !self.settings.watch_collection_files {
            return Ok(None);
        }
        let mut watcher =
            ::notify::recommended_watcher(move |event: ::notify::Result<::notify::Event>| {
                match event {
                    Ok(event) if !event.paths.is_empty() && is_mutation(&event.kind) => {
                        let _ = tx.send(WatchMessage::Changed(event.paths));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        let _ = tx.send(WatchMessage::Error(error.to_string()));
                    }
                }
            })
            .context("could not create file watcher")?;
        if self.settings.watch_collection_files {
            watcher
                .watch(&self.collection.path, RecursiveMode::Recursive)
                .with_context(|| format!("could not watch {}", self.collection.path.display()))?;
        }
        if self.settings.watch_env_files {
            for path in &self.environment.files {
                watcher
                    .watch(path, RecursiveMode::NonRecursive)
                    .with_context(|| format!("could not watch {}", path.display()))?;
            }
        }
        Ok(Some(watcher))
    }

    fn handle_files_changed(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let env_changed = self.settings.watch_env_files
            && paths.iter().any(|path| {
                self.environment
                    .files
                    .iter()
                    .any(|env| same_path(path, env))
            });
        if env_changed {
            match self.environment.reload() {
                Ok(()) => self
                    .toasts
                    .push("Environment reloaded", Severity::Information),
                Err(error) => self.toasts.push(
                    format!("Environment reload failed: {error:#}"),
                    Severity::Error,
                ),
            }
        }

        let collection_changed = self.settings.watch_collection_files
            && paths
                .iter()
                .any(|path| path.starts_with(&self.collection.path));
        if collection_changed {
            for path in &paths {
                self.script_engine.invalidate(path);
            }
            self.reload_collection();
            if !self.dirty
                && let Some(open) = self.current.path.clone()
                && paths.iter().any(|path| same_path(path, &open))
                && open.is_file()
            {
                self.reopen_if_changed(&open);
            }
            self.toasts
                .push("Collection reloaded", Severity::Information);
        }
    }
}

fn command_palette_items(choices: &[CommandChoice]) -> Vec<PaletteItem> {
    choices
        .iter()
        .enumerate()
        .map(|(id, choice)| {
            let (label, hint, search_extra) = match choice {
                CommandChoice::Reset => ("view: Reset".to_owned(), None, None),
                CommandChoice::ExpandRequest => {
                    ("view: Expand request section".to_owned(), None, None)
                }
                CommandChoice::ExpandResponse => {
                    ("view: Expand response section".to_owned(), None, None)
                }
                CommandChoice::ToggleCollection => {
                    ("view: Toggle collection browser".to_owned(), None, None)
                }
                CommandChoice::LoadEnv => ("environment: Load env file".to_owned(), None, None),
                CommandChoice::CopyYaml => ("export: copy as YAML".to_owned(), None, None),
                CommandChoice::EditScript {
                    hook, configured, ..
                } => (
                    format!("scripts: Edit {} hook", hook.label()),
                    Some(configured.clone()),
                    Some(configured.clone()),
                ),
                CommandChoice::Quit => ("app: Quit rusting".to_owned(), None, None),
            };
            PaletteItem {
                label,
                hint,
                search_extra,
                id,
            }
        })
        .collect()
}

fn prepare_script_for_edit(
    collection_root: &Path,
    hook: ScriptHook,
    reference: &ScriptRef,
) -> anyhow::Result<(PathBuf, bool)> {
    let resolved = if reference.path.is_absolute() {
        reference.path.clone()
    } else {
        collection_root.join(&reference.path)
    };
    if resolved.is_file() {
        return Ok((resolved, false));
    }
    match fs::symlink_metadata(&resolved) {
        Ok(_) => bail!("script path is not a regular file: {}", resolved.display()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("could not inspect script {}", resolved.display()));
        }
    }
    if reference.path.is_absolute() {
        bail!(
            "cannot create script outside the collection: {}",
            reference.path.display()
        );
    }

    let mut parts = Vec::new();
    for component in reference.path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!(
                    "cannot create script from unsafe path {}",
                    reference.path.display()
                );
            }
        }
    }
    let file_name = parts
        .pop()
        .context("script reference does not contain a file name")?;
    let canonical_root = collection_root.canonicalize().with_context(|| {
        format!(
            "could not resolve collection root {}",
            collection_root.display()
        )
    })?;
    if !canonical_root.is_dir() {
        bail!(
            "collection root is not a directory: {}",
            collection_root.display()
        );
    }

    let mut parent = canonical_root.clone();
    for directory in parts {
        parent.push(directory);
        ensure_script_directory(&parent)?;
    }
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("could not resolve script directory {}", parent.display()))?;
    if !canonical_parent.starts_with(&canonical_root) {
        bail!(
            "cannot create script outside the collection: {}",
            resolved.display()
        );
    }
    let creation_path = canonical_parent.join(file_name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&creation_path)
        .with_context(|| format!("could not create script {}", resolved.display()))?;
    let arguments = match hook {
        ScriptHook::Setup => "rusting",
        ScriptHook::OnRequest => "request, rusting",
        ScriptHook::OnResponse => "response, rusting",
    };
    let starter = format!(
        "export function {}({arguments}) {{\n}}\n",
        reference.function
    );
    if let Err(error) = file.write_all(starter.as_bytes()) {
        drop(file);
        let _ = fs::remove_file(&creation_path);
        return Err(error).with_context(|| format!("could not initialize {}", resolved.display()));
    }
    Ok((resolved, true))
}

fn ensure_script_directory(path: &Path) -> anyhow::Result<()> {
    loop {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!(
                    "cannot create script through symlinked directory {}",
                    path.display()
                );
            }
            Ok(metadata) if metadata.is_dir() => return Ok(()),
            Ok(_) => bail!("script parent is not a directory: {}", path.display()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::create_dir(path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("could not create script directory {}", path.display())
                    });
                }
            },
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("could not inspect script directory {}", path.display())
                });
            }
        }
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .canonicalize()
            .ok()
            .zip(right.canonicalize().ok())
            .is_some_and(|(left, right)| left == right)
}

/// Only real filesystem mutations should trigger a reload. inotify also reports
/// reads (`EventKind::Access`), and reloading re-reads the collection, which would
/// feed the watcher its own events forever.
fn is_mutation(kind: &::notify::EventKind) -> bool {
    matches!(
        kind,
        ::notify::EventKind::Create(_)
            | ::notify::EventKind::Modify(_)
            | ::notify::EventKind::Remove(_)
    )
}

struct EventThread {
    running: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EventThread {
    fn spawn(
        tx: mpsc::UnboundedSender<TerminalMessage>,
        pause: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        alive: Arc<AtomicBool>,
    ) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let worker_pause = Arc::clone(&pause);
        alive.store(true, Ordering::Release);
        let thread = std::thread::spawn(move || {
            while worker_running.load(Ordering::Acquire) {
                if worker_pause.load(Ordering::Acquire) {
                    paused.store(true, Ordering::Release);
                    while worker_running.load(Ordering::Acquire)
                        && worker_pause.load(Ordering::Acquire)
                    {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    paused.store(false, Ordering::Release);
                    continue;
                }
                match event::poll(Duration::from_millis(50)) {
                    Ok(true) => match event::read() {
                        Ok(event) => {
                            if tx.send(TerminalMessage::Event(event)).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(TerminalMessage::Error(format!(
                                "terminal input failed: {error}"
                            )));
                            break;
                        }
                    },
                    Ok(false) => {}
                    Err(error) => {
                        let _ = tx.send(TerminalMessage::Error(format!(
                            "terminal polling failed: {error}"
                        )));
                        break;
                    }
                }
            }
            paused.store(false, Ordering::Release);
            alive.store(false, Ordering::Release);
        });
        Self {
            running,
            pause,
            thread: Some(thread),
        }
    }
}

impl Drop for EventThread {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.pause.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

type PanicHook = dyn for<'a, 'b> Fn(&'a std::panic::PanicHookInfo<'b>) + Send + Sync + 'static;

struct TerminalGuard {
    previous_hook: Arc<PanicHook>,
}

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode().context("could not enable terminal raw mode")?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            restore_terminal_best_effort();
            return Err(error).context("could not enter alternate screen");
        }
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
        let _ = execute!(stdout, PushKeyboardEnhancementFlags(flags));
        if let Err(error) = stdout.flush() {
            restore_terminal_best_effort();
            return Err(error).context("could not flush terminal setup");
        }

        let previous_hook: Arc<PanicHook> = Arc::from(std::panic::take_hook());
        let panic_hook = Arc::clone(&previous_hook);
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal_best_effort();
            panic_hook(info);
        }));
        Ok(Self { previous_hook })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal_best_effort();
        if !std::thread::panicking() {
            let previous_hook = Arc::clone(&self.previous_hook);
            let _ = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| previous_hook(info)));
        }
    }
}

fn restore_terminal_best_effort() {
    let mut stdout = io::stdout();
    let _ = execute!(
        stdout,
        PopKeyboardEnhancementFlags,
        Show,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
    let _ = stdout.flush();
}

fn repaint_after_external<B>(
    terminal: &mut Terminal<B>,
    repaint_requested: &mut bool,
) -> anyhow::Result<()>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
{
    if !*repaint_requested {
        return Ok(());
    }
    let area: Rect = terminal
        .size()
        .context("could not query terminal size before post-external repaint")?
        .into();
    terminal
        .resize(area)
        .context("could not invalidate terminal buffers after external command")?;
    *repaint_requested = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, buffer::Buffer};

    #[test]
    fn path_comparison_accepts_identical_nonexistent_paths() {
        let path = Path::new("not-present/rusting-request.posting.yaml");
        assert!(same_path(path, path));
    }

    #[test]
    fn watcher_ignores_read_only_filesystem_events() {
        use ::notify::event::{
            AccessKind, AccessMode, CreateKind, DataChange, EventKind, ModifyKind, RemoveKind,
        };

        assert!(is_mutation(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(is_mutation(&EventKind::Create(CreateKind::File)));
        assert!(is_mutation(&EventKind::Remove(RemoveKind::File)));
        assert!(!is_mutation(&EventKind::Access(AccessKind::Read)));
        assert!(!is_mutation(&EventKind::Access(AccessKind::Open(
            AccessMode::Read
        ))));
        assert!(!is_mutation(&EventKind::Any));
    }

    #[test]
    fn command_palette_lists_only_configured_hooks_with_their_references() {
        let settings = Settings {
            watch_env_files: false,
            watch_collection_files: false,
            ..Settings::default()
        };
        let root = tempfile::tempdir().unwrap();
        let collection = Collection::new(root.path());
        let environment = Environment::load(Vec::new(), false).unwrap();
        let mut app = App::new(settings, environment, collection, Vec::new()).unwrap();
        app.request_pane.load(&RequestModel {
            scripts: rusting_core::Scripts {
                setup: Some("scripts/hooks.js:prepare".to_owned()),
                on_request: None,
                on_response: Some("scripts/hooks.js:inspect".to_owned()),
            },
            ..RequestModel::default()
        });

        app.open_command_palette();
        let Some(ActiveModal::Palette {
            purpose: PalettePurpose::Commands(choices),
            ..
        }) = app.modal.as_ref()
        else {
            panic!("command palette");
        };
        let edit_choices = choices
            .iter()
            .filter_map(|choice| match choice {
                CommandChoice::EditScript {
                    hook,
                    configured,
                    reference,
                } => Some((*hook, configured.as_str(), reference.function.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            edit_choices,
            vec![
                (ScriptHook::Setup, "scripts/hooks.js:prepare", "prepare"),
                (
                    ScriptHook::OnResponse,
                    "scripts/hooks.js:inspect",
                    "inspect"
                ),
            ]
        );

        let items = command_palette_items(choices);
        let script_items = items
            .iter()
            .filter(|item| item.label.starts_with("scripts:"))
            .collect::<Vec<_>>();
        assert_eq!(script_items.len(), 2);
        assert_eq!(script_items[0].label, "scripts: Edit Setup hook");
        assert_eq!(
            script_items[0].hint.as_deref(),
            Some("scripts/hooks.js:prepare")
        );
        assert_eq!(
            script_items[0].search_extra.as_deref(),
            Some("scripts/hooks.js:prepare")
        );
        assert_eq!(script_items[1].label, "scripts: Edit Post-response hook");
    }

    #[test]
    fn script_starters_export_the_configured_function_with_hook_arguments() {
        let root = tempfile::tempdir().unwrap();
        for (hook, configured, expected) in [
            (
                ScriptHook::Setup,
                "scripts/setup.js:prepare",
                "export function prepare(rusting) {\n}\n",
            ),
            (
                ScriptHook::OnRequest,
                "scripts/request.js:beforeSend",
                "export function beforeSend(request, rusting) {\n}\n",
            ),
            (
                ScriptHook::OnResponse,
                "scripts/response.js:inspect",
                "export function inspect(response, rusting) {\n}\n",
            ),
        ] {
            let reference = ScriptRef::parse(configured, hook).unwrap();
            let (path, created) = prepare_script_for_edit(root.path(), hook, &reference).unwrap();
            assert!(created);
            assert_eq!(path, root.path().join(&reference.path));
            assert_eq!(fs::read_to_string(path).unwrap(), expected);
        }
    }

    #[test]
    fn missing_nested_script_is_created_without_overwriting_an_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let reference =
            ScriptRef::parse("scripts/nested/hook.js:prepare", ScriptHook::Setup).unwrap();
        let (path, created) =
            prepare_script_for_edit(root.path(), ScriptHook::Setup, &reference).unwrap();
        assert!(created);
        fs::write(&path, "existing contents").unwrap();

        let (same_path, created) =
            prepare_script_for_edit(root.path(), ScriptHook::Setup, &reference).unwrap();
        assert_eq!(same_path, path);
        assert!(!created);
        assert_eq!(fs::read_to_string(path).unwrap(), "existing contents");
    }

    #[test]
    fn missing_scripts_outside_or_traversing_the_collection_are_rejected() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().join("collection");
        fs::create_dir(&root).unwrap();
        let traversal = ScriptRef::parse("../escape.js", ScriptHook::Setup).unwrap();
        assert!(prepare_script_for_edit(&root, ScriptHook::Setup, &traversal).is_err());
        assert!(!workspace.path().join("escape.js").exists());

        let absolute_path = workspace.path().join("absolute.js");
        let absolute = ScriptRef {
            path: absolute_path.clone(),
            function: "setup".to_owned(),
        };
        assert!(prepare_script_for_edit(&root, ScriptHook::Setup, &absolute).is_err());
        assert!(!absolute_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn missing_script_creation_rejects_a_symlink_escape() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().join("collection");
        let outside = workspace.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("linked")).unwrap();
        let reference = ScriptRef::parse("linked/hook.js", ScriptHook::Setup).unwrap();

        assert!(prepare_script_for_edit(&root, ScriptHook::Setup, &reference).is_err());
        assert!(!outside.join("hook.js").exists());
    }

    #[test]
    fn focus_section_expansion_is_reversible() {
        let settings = Settings {
            watch_env_files: false,
            watch_collection_files: false,
            ..Settings::default()
        };
        let root = tempfile::tempdir().unwrap();
        let collection = Collection::new(root.path());
        let environment = Environment::load(Vec::new(), false).unwrap();
        let mut app = App::new(settings, environment, collection, Vec::new()).unwrap();
        app.set_focus(Focus::ResponseBody);
        app.toggle_focused_section();
        assert_eq!(app.expanded, Some(Section::Response));
        app.toggle_focused_section();
        assert_eq!(app.expanded, None);
    }

    #[test]
    fn command_palette_hiding_collection_repairs_collection_focus() {
        let settings = Settings {
            watch_env_files: false,
            watch_collection_files: false,
            ..Settings::default()
        };
        let root = tempfile::tempdir().unwrap();
        let collection = Collection::new(root.path());
        let environment = Environment::load(Vec::new(), false).unwrap();
        let mut app = App::new(settings, environment, collection, Vec::new()).unwrap();
        app.set_focus(Focus::Collection);

        app.accept_palette(
            0,
            PalettePurpose::Commands(vec![CommandChoice::ToggleCollection]),
        );

        assert!(!app.sidebar_visible);
        assert_eq!(app.focus, Focus::Url);
    }

    #[test]
    fn failed_send_state_clears_the_previous_response_and_status() {
        let settings = Settings {
            watch_env_files: false,
            watch_collection_files: false,
            ..Settings::default()
        };
        let root = tempfile::tempdir().unwrap();
        let collection = Collection::new(root.path());
        let environment = Environment::load(Vec::new(), false).unwrap();
        let mut app = App::new(settings, environment, collection, Vec::new()).unwrap();
        let response = Response {
            status: 200,
            reason: "OK".to_owned(),
            url: "https://example.test".to_owned(),
            headers: Vec::new(),
            cookies: Vec::new(),
            body: b"old response".to_vec(),
            timings: Timings::default(),
            sent: rusting_http::types::SentRequest::default(),
        };
        app.url_bar.set_response(response.status, &response.reason);
        app.response_pane
            .set_response(&response, &Settings::default());
        assert!(app.response_pane.has_response());

        app.apply_failed_send_state();

        assert!(!app.response_pane.has_response());
        let area = Rect::new(0, 0, 80, 4);
        let mut buffer = Buffer::empty(area);
        app.url_bar.render(
            area,
            &mut buffer,
            true,
            &Settings::default(),
            app.environment.variables(),
            &[],
        );
        let rendered = buffer
            .content()
            .iter()
            .fold(String::new(), |mut rendered, cell| {
                rendered.push_str(cell.symbol());
                rendered
            });
        assert!(!rendered.contains("200"));
    }

    #[test]
    fn self_written_request_file_with_session_cookies_does_not_clear_the_response() {
        let settings = Settings {
            watch_env_files: false,
            watch_collection_files: true,
            ..Settings::default()
        };
        let root = tempfile::tempdir().unwrap();
        let collection = Collection::new(root.path());
        let environment = Environment::load(Vec::new(), false).unwrap();
        let mut app = App::new(settings, environment, collection, Vec::new()).unwrap();

        let path = root.path().join("probe.posting.yaml");
        let model = RequestModel {
            url: "https://example.com/first".to_owned(),
            path: Some(path.clone()),
            ..RequestModel::default()
        };
        collection::save_request(&model).unwrap();
        app.open_path(&path);
        assert_eq!(app.current.url, "https://example.com/first");

        let cookie = rusting_core::KeyValue::new("session", "redirect");
        app.current.cookies.push(cookie.clone());
        let response = Response {
            status: 302,
            reason: "Found".to_owned(),
            url: "https://example.com/first".to_owned(),
            headers: Vec::new(),
            cookies: vec![cookie.clone()],
            body: b"redirect response".to_vec(),
            timings: Timings::default(),
            sent: rusting_http::types::SentRequest::default(),
        };
        app.url_bar.set_response(response.status, &response.reason);
        app.response_pane
            .set_response(&response, &Settings::default());

        collection::save_request(&app.current).unwrap();
        assert!(load_request(&path).unwrap().cookies.is_empty());

        // The saved YAML has no serde-skipped cookies, but is otherwise the app's own write.
        app.send_generation = 7;
        app.handle_files_changed(vec![path.clone()]);
        assert_eq!(app.send_generation, 7);
        assert_eq!(app.current.cookies, vec![cookie.clone()]);
        assert!(app.response_pane.has_response());
        let area = Rect::new(0, 0, 80, 4);
        let mut buffer = Buffer::empty(area);
        app.url_bar.render(
            area,
            &mut buffer,
            true,
            &Settings::default(),
            app.environment.variables(),
            &[],
        );
        let rendered = buffer
            .content()
            .iter()
            .fold(String::new(), |mut rendered, cell| {
                rendered.push_str(cell.symbol());
                rendered
            });
        assert!(rendered.contains("302"));

        // A genuine external edit is still picked up.
        let edited = RequestModel {
            url: "https://example.com/second".to_owned(),
            ..app.current.clone()
        };
        collection::save_request(&edited).unwrap();
        app.handle_files_changed(vec![path.clone()]);
        assert_eq!(app.send_generation, 8);
        assert_eq!(app.current.url, "https://example.com/second");
        assert_eq!(app.current.cookies, vec![cookie]);
    }

    #[test]
    fn external_handoffs_request_repaint_on_success_and_failure() {
        let settings = Settings {
            watch_env_files: false,
            watch_collection_files: false,
            ..Settings::default()
        };
        let root = tempfile::tempdir().unwrap();
        let collection = Collection::new(root.path());
        let environment = Environment::load(Vec::new(), false).unwrap();
        let mut app = App::new(settings, environment, collection, Vec::new()).unwrap();

        app.run_external(|| Ok(())).unwrap();
        assert!(app.post_external_repaint);
        assert!(!app.event_pause.load(Ordering::Acquire));

        app.post_external_repaint = false;
        let result = app.run_external(|| Err::<(), _>(anyhow::anyhow!("child failed")));
        assert!(result.is_err());
        assert!(app.post_external_repaint);
        assert!(!app.event_pause.load(Ordering::Acquire));
    }

    fn app_for_key_dispatch() -> (App, tempfile::TempDir) {
        let settings = Settings {
            watch_env_files: false,
            watch_collection_files: false,
            ..Settings::default()
        };
        let root = tempfile::tempdir().unwrap();
        let collection = Collection::new(root.path());
        let environment = Environment::load(Vec::new(), false).unwrap();
        let app = App::new(settings, environment, collection, Vec::new()).unwrap();
        (app, root)
    }

    fn key_dispatch_senders() -> (
        mpsc::UnboundedSender<SendFinished>,
        mpsc::UnboundedSender<(u64, PhaseEvent)>,
    ) {
        let (finished_tx, _finished_rx) = mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = mpsc::unbounded_channel();
        (finished_tx, progress_tx)
    }

    #[tokio::test]
    async fn slash_is_inserted_into_a_focused_script_input() {
        let (mut app, _root) = app_for_key_dispatch();
        let (finished_tx, progress_tx) = key_dispatch_senders();
        app.request_pane.load(&RequestModel {
            scripts: rusting_core::Scripts {
                setup: Some("scripts".to_owned()),
                on_request: None,
                on_response: None,
            },
            ..RequestModel::default()
        });
        app.request_pane.set_active_tab(RequestTab::Scripts);
        app.set_focus(Focus::RequestBody);

        app.handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &finished_tx,
            &progress_tx,
        )
        .await;

        let model = app.request_pane.to_model(&app.current).unwrap();
        assert_eq!(model.scripts.setup.as_deref(), Some("scripts/"));
        assert!(app.modal.is_none());
    }

    #[tokio::test]
    async fn slash_is_inserted_into_the_focused_url() {
        let (mut app, _root) = app_for_key_dispatch();
        let (finished_tx, progress_tx) = key_dispatch_senders();
        app.url_bar.set_url("https://example.test");
        app.set_focus(Focus::Url);

        app.handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &finished_tx,
            &progress_tx,
        )
        .await;

        assert_eq!(app.url_bar.url(), "https://example.test/");
        assert!(app.modal.is_none());
    }

    #[tokio::test]
    async fn slash_opens_request_search_from_the_request_tab_bar() {
        let (mut app, _root) = app_for_key_dispatch();
        let (finished_tx, progress_tx) = key_dispatch_senders();
        app.set_focus(Focus::RequestTabs);

        app.handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &finished_tx,
            &progress_tx,
        )
        .await;

        assert!(matches!(
            app.modal.as_ref(),
            Some(ActiveModal::Palette {
                purpose: PalettePurpose::Search(_),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn slash_opens_request_search_when_the_request_control_ignores_it() {
        let (mut app, _root) = app_for_key_dispatch();
        let (finished_tx, progress_tx) = key_dispatch_senders();
        app.request_pane.set_active_tab(RequestTab::Options);
        app.set_focus(Focus::RequestBody);

        app.handle_key(
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            &finished_tx,
            &progress_tx,
        )
        .await;

        assert!(matches!(
            app.modal.as_ref(),
            Some(ActiveModal::Palette {
                purpose: PalettePurpose::Search(_),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn tab_moves_url_send_request_and_response_to_collection() {
        let settings = Settings {
            watch_env_files: false,
            watch_collection_files: false,
            ..Settings::default()
        };
        let root = tempfile::tempdir().unwrap();
        let collection = Collection::new(root.path());
        let environment = Environment::load(Vec::new(), false).unwrap();
        let mut app = App::new(settings, environment, collection, Vec::new()).unwrap();
        let (finished_tx, _finished_rx) = mpsc::unbounded_channel();
        let (progress_tx, _progress_rx) = mpsc::unbounded_channel();
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);

        app.set_focus(Focus::Url);
        app.handle_key(tab, &finished_tx, &progress_tx).await;
        assert_eq!(app.focus, Focus::Send);
        app.handle_key(tab, &finished_tx, &progress_tx).await;
        assert_eq!(app.focus, Focus::RequestTabs);
        app.handle_key(tab, &finished_tx, &progress_tx).await;
        assert_eq!(app.focus, Focus::RequestBody);
        app.handle_key(backtab, &finished_tx, &progress_tx).await;
        assert_eq!(app.focus, Focus::RequestTabs);
        app.handle_key(backtab, &finished_tx, &progress_tx).await;
        assert_eq!(app.focus, Focus::Send);

        app.set_focus(Focus::ResponseTabs);
        app.handle_key(tab, &finished_tx, &progress_tx).await;
        assert_eq!(app.focus, Focus::Collection);
        app.sidebar_visible = false;
        app.set_focus(Focus::ResponseBody);
        app.handle_key(tab, &finished_tx, &progress_tx).await;
        assert_eq!(app.focus, Focus::Method);
    }

    #[test]
    fn post_external_resize_forces_unchanged_frame_to_be_repainted() {
        let backend = TestBackend::new(8, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new("restored"), frame.area()))
            .unwrap();

        terminal.backend_mut().resize(0, 0);
        terminal.backend_mut().resize(8, 1);
        assert_eq!(
            terminal.backend().buffer(),
            &Buffer::empty(Rect::new(0, 0, 8, 1))
        );

        let mut repaint_requested = true;
        repaint_after_external(&mut terminal, &mut repaint_requested).unwrap();
        assert!(!repaint_requested);
        terminal
            .draw(|frame| frame.render_widget(Paragraph::new("restored"), frame.area()))
            .unwrap();

        assert_eq!(
            terminal.backend().buffer(),
            &Buffer::with_lines(["restored"])
        );
    }
}
