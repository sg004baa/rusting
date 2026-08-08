use std::collections::BTreeMap;

use anyhow::{Context as _, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    SendRequest,
    FocusMethod,
    FocusUrl,
    SaveRequest,
    NewRequest,
    ExpandSection,
    ToggleCollection,
    SearchRequests,
    Commands,
    Jump,
    Help,
    Quit,
    OpenInPager,
    OpenInEditor,
}

impl Action {
    pub const ALL: [Action; 14] = [
        Self::SendRequest,
        Self::FocusMethod,
        Self::FocusUrl,
        Self::SaveRequest,
        Self::NewRequest,
        Self::ExpandSection,
        Self::ToggleCollection,
        Self::SearchRequests,
        Self::Commands,
        Self::Jump,
        Self::Help,
        Self::Quit,
        Self::OpenInPager,
        Self::OpenInEditor,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::SendRequest => "send-request",
            Self::FocusMethod => "focus-method",
            Self::FocusUrl => "focus-url",
            Self::SaveRequest => "save-request",
            Self::NewRequest => "new-request",
            Self::ExpandSection => "expand-section",
            Self::ToggleCollection => "toggle-collection",
            Self::SearchRequests => "search-requests",
            Self::Commands => "commands",
            Self::Jump => "jump",
            Self::Help => "help",
            Self::Quit => "quit",
            Self::OpenInPager => "open-in-pager",
            Self::OpenInEditor => "open-in-editor",
        }
    }

    pub const fn default_keys(self) -> &'static str {
        match self {
            Self::SendRequest => "ctrl+j",
            Self::FocusMethod => "ctrl+t",
            Self::FocusUrl => "ctrl+l",
            Self::SaveRequest => "ctrl+s",
            Self::NewRequest => "ctrl+n",
            Self::ExpandSection => "ctrl+m",
            Self::ToggleCollection => "ctrl+h",
            Self::SearchRequests => "/",
            Self::Commands => "ctrl+p",
            Self::Jump => "ctrl+o",
            Self::Help => "?",
            Self::Quit => "ctrl+c",
            Self::OpenInPager => "alt+p",
            Self::OpenInEditor => "ctrl+e",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::SendRequest => "Send request",
            Self::FocusMethod => "Focus method",
            Self::FocusUrl => "Focus URL",
            Self::SaveRequest => "Save request",
            Self::NewRequest => "New request",
            Self::ExpandSection => "Expand focused section",
            Self::ToggleCollection => "Toggle collection browser",
            Self::SearchRequests => "Search requests",
            Self::Commands => "Open command palette",
            Self::Jump => "Jump to a control",
            Self::Help => "Show help",
            Self::Quit => "Quit rusting",
            Self::OpenInPager => "Open in pager",
            Self::OpenInEditor => "Open in editor",
        }
    }

    pub const fn show_in_footer(self) -> bool {
        matches!(
            self,
            Self::SendRequest
                | Self::SaveRequest
                | Self::NewRequest
                | Self::Commands
                | Self::Jump
                | Self::Help
                | Self::Quit
        )
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.id() == id)
    }
}

#[derive(Debug, Clone)]
pub struct Keymap {
    bindings: Vec<(Action, Vec<KeyEvent>)>,
}

impl Keymap {
    pub fn new(overrides: &BTreeMap<String, String>) -> anyhow::Result<Self> {
        for id in overrides.keys() {
            if Action::from_id(id).is_none() {
                bail!("unknown keymap action {id:?}");
            }
        }

        let mut bindings = Vec::with_capacity(Action::ALL.len());
        for action in Action::ALL {
            let configured = overrides
                .get(action.id())
                .map(String::as_str)
                .unwrap_or_else(|| action.default_keys());
            if configured.trim().is_empty() {
                bail!("keymap action {:?} has no keys", action.id());
            }
            let keys = configured
                .split(',')
                .map(|key| {
                    let key = key.trim();
                    if key.is_empty() {
                        bail!("empty key in binding for {:?}", action.id());
                    }
                    parse_key(key).with_context(|| format!("invalid binding for {:?}", action.id()))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            bindings.push((action, keys));
        }
        Ok(Self { bindings })
    }

    pub fn action_for(&self, key: KeyEvent) -> Option<Action> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        let key = normalized(key);
        let matches = |action| {
            self.keys_for(action)
                .iter()
                .copied()
                .map(normalized)
                .any(|bound| bound == key)
        };
        if matches(Action::Quit) {
            return Some(Action::Quit);
        }
        Action::ALL
            .into_iter()
            .filter(|action| *action != Action::Quit)
            .find(|action| matches(*action))
    }

    pub fn keys_for(&self, action: Action) -> &[KeyEvent] {
        self.bindings
            .iter()
            .find_map(|(candidate, keys)| (*candidate == action).then_some(keys.as_slice()))
            .unwrap_or(&[])
    }

    pub fn display(&self, action: Action) -> String {
        self.keys_for(action)
            .iter()
            .copied()
            .map(format_key)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn normalized(key: KeyEvent) -> KeyEvent {
    let mut modifiers = key.modifiers;
    let code = match key.code {
        KeyCode::BackTab => {
            modifiers.insert(KeyModifiers::SHIFT);
            KeyCode::Tab
        }
        KeyCode::Char(character) if modifiers.contains(KeyModifiers::SHIFT) => {
            KeyCode::Char(character.to_ascii_lowercase())
        }
        code => code,
    };
    KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

pub fn parse_key(text: &str) -> anyhow::Result<KeyEvent> {
    let text = text.trim().to_ascii_lowercase();
    if text.is_empty() {
        bail!("key cannot be empty");
    }
    let parts = text.split('+').collect::<Vec<_>>();
    let (key_name, modifier_names) = parts
        .split_last()
        .expect("a non-empty string always has one component");
    if key_name.is_empty() || modifier_names.iter().any(|part| part.is_empty()) {
        bail!("malformed key {text:?}");
    }

    let mut modifiers = KeyModifiers::NONE;
    for modifier in modifier_names {
        let flag = match *modifier {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "alt" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            "super" => KeyModifiers::SUPER,
            "hyper" => KeyModifiers::HYPER,
            "meta" => KeyModifiers::META,
            other => bail!("unknown modifier {other:?}"),
        };
        if modifiers.contains(flag) {
            bail!("duplicate modifier {modifier:?}");
        }
        modifiers.insert(flag);
    }

    let code = match *key_name {
        "backspace" => KeyCode::Backspace,
        "enter" | "return" => KeyCode::Enter,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "page-up" => KeyCode::PageUp,
        "pagedown" | "page-down" => KeyCode::PageDown,
        "tab" => KeyCode::Tab,
        "backtab" => {
            modifiers.insert(KeyModifiers::SHIFT);
            KeyCode::BackTab
        }
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        name if name.starts_with('f') && name.len() > 1 => {
            let number = name[1..]
                .parse::<u8>()
                .with_context(|| format!("invalid function key {name:?}"))?;
            if !(1..=24).contains(&number) {
                bail!("function key is outside f1..f24: {name:?}");
            }
            KeyCode::F(number)
        }
        name => {
            let mut characters = name.chars();
            let Some(character) = characters.next() else {
                bail!("key cannot be empty");
            };
            if characters.next().is_some() {
                bail!("unknown key name {name:?}");
            }
            KeyCode::Char(character)
        }
    };
    Ok(KeyEvent::new(code, modifiers))
}

pub fn format_key(key: KeyEvent) -> String {
    let key = normalized(key);
    let mut parts = Vec::new();
    for (modifier, label) in [
        (KeyModifiers::CONTROL, "ctrl"),
        (KeyModifiers::ALT, "alt"),
        (KeyModifiers::SHIFT, "shift"),
        (KeyModifiers::SUPER, "super"),
        (KeyModifiers::HYPER, "hyper"),
        (KeyModifiers::META, "meta"),
    ] {
        if key.modifiers.contains(modifier) {
            parts.push(label.to_owned());
        }
    }
    let name = match key.code {
        KeyCode::Backspace => "backspace".to_owned(),
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Left => "left".to_owned(),
        KeyCode::Right => "right".to_owned(),
        KeyCode::Up => "up".to_owned(),
        KeyCode::Down => "down".to_owned(),
        KeyCode::Home => "home".to_owned(),
        KeyCode::End => "end".to_owned(),
        KeyCode::PageUp => "pageup".to_owned(),
        KeyCode::PageDown => "pagedown".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::BackTab => "tab".to_owned(),
        KeyCode::Delete => "delete".to_owned(),
        KeyCode::Insert => "insert".to_owned(),
        KeyCode::F(number) => format!("f{number}"),
        KeyCode::Char(' ') => "space".to_owned(),
        KeyCode::Char(character) => character.to_string(),
        KeyCode::Esc => "esc".to_owned(),
        KeyCode::Null => "null".to_owned(),
        KeyCode::CapsLock => "capslock".to_owned(),
        KeyCode::ScrollLock => "scrolllock".to_owned(),
        KeyCode::NumLock => "numlock".to_owned(),
        KeyCode::PrintScreen => "printscreen".to_owned(),
        KeyCode::Pause => "pause".to_owned(),
        KeyCode::Menu => "menu".to_owned(),
        KeyCode::KeypadBegin => "keypad-begin".to_owned(),
        KeyCode::Media(_) | KeyCode::Modifier(_) => "unknown".to_owned(),
    };
    parts.push(name);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_modified_keys() {
        let key = parse_key("ctrl+shift+p").unwrap();
        assert_eq!(key.code, KeyCode::Char('p'));
        assert_eq!(format_key(key), "ctrl+shift+p");
        assert_eq!(format_key(parse_key("alt+enter").unwrap()), "alt+enter");
        assert_eq!(format_key(parse_key("f24").unwrap()), "f24");
    }

    #[test]
    fn defaults_use_non_function_keys_for_help_and_external_programs() {
        let keymap = Keymap::new(&BTreeMap::new()).unwrap();
        assert_eq!(keymap.display(Action::Help), "?");
        assert_eq!(keymap.display(Action::OpenInPager), "alt+p");
        assert_eq!(keymap.display(Action::OpenInEditor), "ctrl+e");
        assert_eq!(
            keymap.action_for(parse_key("?").unwrap()),
            Some(Action::Help)
        );
        assert_eq!(
            keymap.action_for(parse_key("alt+p").unwrap()),
            Some(Action::OpenInPager)
        );
        assert_eq!(
            keymap.action_for(parse_key("ctrl+e").unwrap()),
            Some(Action::OpenInEditor)
        );
        for old_default in ["f1", "f3", "f4"] {
            assert_eq!(keymap.action_for(parse_key(old_default).unwrap()), None);
        }
    }

    #[test]
    fn rejects_unknown_or_malformed_bindings() {
        assert!(parse_key("ctrl+wat+p").is_err());
        assert!(parse_key("ctrl+").is_err());
        assert!(parse_key("f25").is_err());
        let mut overrides = BTreeMap::new();
        overrides.insert("not-an-action".to_owned(), "f2".to_owned());
        assert!(Keymap::new(&overrides).is_err());
    }

    #[test]
    fn overrides_replace_defaults_and_quit_wins_collisions() {
        let mut overrides = BTreeMap::new();
        overrides.insert("send-request".to_owned(), "f2, alt+enter".to_owned());
        overrides.insert("quit".to_owned(), "f2".to_owned());
        let keymap = Keymap::new(&overrides).unwrap();
        assert_eq!(keymap.action_for(parse_key("ctrl+j").unwrap()), None);
        assert_eq!(
            keymap.action_for(parse_key("f2").unwrap()),
            Some(Action::Quit)
        );
        assert_eq!(keymap.display(Action::SendRequest), "f2, alt+enter");
    }
}
