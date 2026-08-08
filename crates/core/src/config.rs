//! Settings, and the layered loader that produces them.
//!
//! Precedence, highest first:
//!
//! 1. process environment (`RUSTING_*`)
//! 2. the `--env` dotenv files, in the order given, later files winning
//! 3. the YAML config file
//! 4. compiled-in defaults
//!
//! This is the conventional order. `posting` deliberately let the config file
//! outrank the environment; that is surprising enough that it is not carried
//! over.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use crate::locations::{ENV_NESTED_SEPARATOR, ENV_PREFIX};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidebarPosition {
    #[default]
    Left,
    Right,
}

/// Where focus lands when the app starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StartupFocus {
    Url,
    Method,
    #[default]
    Collection,
}

/// Where focus moves once a response arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFocus {
    Body,
    Tabs,
}

/// Which request tab is focused when a request is opened from the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOpenFocus {
    Headers,
    Body,
    Query,
    Info,
    Url,
    Method,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HeadingSettings {
    pub visible: bool,
    pub show_host: bool,
    pub show_version: bool,
    /// Overrides the detected hostname.
    pub hostname: Option<String>,
}

impl Default for HeadingSettings {
    fn default() -> Self {
        Self {
            visible: true,
            show_host: true,
            show_version: true,
            hostname: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UrlBarSettings {
    /// Show `NAME = value` for the variable under the caret.
    pub show_value_preview: bool,
    /// Redact the preview when the variable name looks like a secret.
    pub hide_secrets_in_value_preview: bool,
}

impl Default for UrlBarSettings {
    fn default() -> Self {
        Self {
            show_value_preview: true,
            hide_secrets_in_value_preview: true,
        }
    }
}

/// Substrings that mark a variable name as secret.
pub const SECRET_NAME_MARKERS: [&str; 4] = ["secret", "key", "password", "token"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResponseSettings {
    pub prettify_json: bool,
    pub show_size_and_time: bool,
}

impl Default for ResponseSettings {
    fn default() -> Self {
        Self {
            prettify_json: true,
            show_size_and_time: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CollectionBrowserSettings {
    pub position: SidebarPosition,
    pub show_on_startup: bool,
}

impl Default for CollectionBrowserSettings {
    fn default() -> Self {
        Self {
            position: SidebarPosition::Left,
            show_on_startup: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextInputSettings {
    pub blinking_cursor: bool,
}

impl Default for TextInputSettings {
    fn default() -> Self {
        Self {
            blinking_cursor: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FocusSettings {
    pub on_startup: StartupFocus,
    pub on_response: Option<ResponseFocus>,
    pub on_request_open: Option<RequestOpenFocus>,
}

/// Client-side TLS material. All paths, no inline PEM.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SslSettings {
    /// Extra root certificates, in addition to the platform store.
    pub ca_bundle: Option<PathBuf>,
    /// Client certificate chain, PEM.
    pub certificate_path: Option<PathBuf>,
    /// Client private key, PEM. Must be unencrypted.
    pub key_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Expose the process environment as request variables.
    pub use_host_environment: bool,
    /// Reload `.env` files when they change on disk.
    pub watch_env_files: bool,
    /// Reload the collection when request files change on disk.
    pub watch_collection_files: bool,
    /// Write the open request back to disk after every response.
    pub auto_save_on_response: bool,
    /// Command used for `alt+p`. Falls back to `$PAGER`.
    pub pager: Option<String>,
    /// Command used for `alt+p` on JSON content. Falls back to `pager`.
    pub pager_json: Option<String>,
    /// Command used for `ctrl+e`. Falls back to `$EDITOR`.
    pub editor: Option<String>,
    /// Binding id to comma-separated key list. Replaces the default binding.
    pub keymap: std::collections::BTreeMap<String, String>,

    pub heading: HeadingSettings,
    pub url_bar: UrlBarSettings,
    pub response: ResponseSettings,
    pub collection_browser: CollectionBrowserSettings,
    pub text_input: TextInputSettings,
    pub focus: FocusSettings,
    pub ssl: SslSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            use_host_environment: false,
            watch_env_files: true,
            watch_collection_files: true,
            auto_save_on_response: false,
            pager: std::env::var("PAGER").ok().filter(|v| !v.is_empty()),
            pager_json: None,
            editor: std::env::var("EDITOR").ok().filter(|v| !v.is_empty()),
            keymap: std::collections::BTreeMap::new(),
            heading: HeadingSettings::default(),
            url_bar: UrlBarSettings::default(),
            response: ResponseSettings::default(),
            collection_browser: CollectionBrowserSettings::default(),
            text_input: TextInputSettings::default(),
            focus: FocusSettings::default(),
            ssl: SslSettings::default(),
        }
    }
}

impl Settings {
    /// The pager command for a given content language.
    pub fn pager_for(&self, language: Option<&str>) -> Option<&str> {
        if language == Some("json")
            && let Some(pager) = self.pager_json.as_deref()
        {
            return Some(pager);
        }
        self.pager.as_deref()
    }

    /// True when a variable's value must be redacted in the URL bar preview.
    pub fn is_secret_name(&self, name: &str) -> bool {
        if !self.url_bar.hide_secrets_in_value_preview {
            return false;
        }
        let lowered = name.to_ascii_lowercase();
        SECRET_NAME_MARKERS
            .iter()
            .any(|marker| lowered.contains(marker))
    }
}

/// Loads settings from the config file, the given dotenv files, and the
/// process environment.
///
/// `dotenv_values` is passed in rather than read here so the caller can reuse
/// the same parse for request variables.
pub fn load(
    config_file: Option<&Path>,
    dotenv_values: &std::collections::BTreeMap<String, String>,
) -> Result<Settings> {
    let mut merged = match config_file {
        Some(path) if path.is_file() => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("could not read {}", path.display()))?;
            match serde_norway::from_str::<Value>(&text)
                .with_context(|| format!("could not parse {}", path.display()))?
            {
                Value::Mapping(mapping) => mapping,
                // An empty config file parses as null, which is fine.
                Value::Null => Mapping::new(),
                other => anyhow::bail!(
                    "{} must contain a mapping at the top level, found {other:?}",
                    path.display()
                ),
            }
        }
        _ => Mapping::new(),
    };

    overlay_prefixed(
        &mut merged,
        dotenv_values.iter().map(|(k, v)| (k.as_str(), v.as_str())),
    );
    let environment: Vec<(String, String)> = std::env::vars().collect();
    overlay_prefixed(
        &mut merged,
        environment.iter().map(|(k, v)| (k.as_str(), v.as_str())),
    );

    serde_norway::from_value(Value::Mapping(merged)).context("invalid settings")
}

/// Applies every `RUSTING_*` entry of `source` onto `target`, splitting nested
/// keys on `__`.
fn overlay_prefixed<'a>(target: &mut Mapping, source: impl Iterator<Item = (&'a str, &'a str)>) {
    for (key, value) in source {
        let Some(stripped) = strip_prefix_ignore_case(key, ENV_PREFIX) else {
            continue;
        };
        if stripped.is_empty() || value.is_empty() {
            // An empty value means "unset"; it must not clobber a real setting.
            continue;
        }
        let segments: Vec<String> = stripped
            .split(ENV_NESTED_SEPARATOR)
            .map(|segment| segment.to_ascii_lowercase())
            .collect();
        if segments.iter().any(String::is_empty) {
            continue;
        }
        insert_nested(target, &segments, parse_scalar(value));
    }
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if text.len() < prefix.len() {
        return None;
    }
    let (head, rest) = text.split_at(prefix.len());
    head.eq_ignore_ascii_case(prefix).then_some(rest)
}

fn insert_nested(target: &mut Mapping, segments: &[String], value: Value) {
    let (head, rest) = segments.split_first().expect("at least one segment");
    let key = Value::String(head.clone());
    if rest.is_empty() {
        target.insert(key, value);
        return;
    }
    let entry = target
        .entry(key)
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if !entry.is_mapping() {
        *entry = Value::Mapping(Mapping::new());
    }
    let Value::Mapping(nested) = entry else {
        unreachable!("just replaced with a mapping");
    };
    insert_nested(nested, rest, value);
}

/// Environment values arrive as strings; give YAML a chance to type them so
/// `RUSTING_HEADING__VISIBLE=false` becomes a bool.
fn parse_scalar(value: &str) -> Value {
    match serde_norway::from_str::<Value>(value) {
        Ok(parsed @ (Value::Bool(_) | Value::Number(_))) => parsed,
        _ => Value::String(value.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn dotenv(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn write_config(tag: &str, contents: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("rusting-config-{tag}-{}.yaml", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn defaults_are_sane() {
        let settings = Settings::default();
        assert!(!settings.use_host_environment);
        assert!(settings.watch_env_files);
        assert!(settings.response.prettify_json);
        assert_eq!(settings.focus.on_startup, StartupFocus::Collection);
        assert_eq!(settings.collection_browser.position, SidebarPosition::Left);
    }

    #[test]
    fn config_file_overrides_defaults_including_nested_keys() {
        let path = write_config(
            "nested",
            "use_host_environment: true\nresponse:\n  show_size_and_time: false\nfocus:\n  on_startup: url\n",
        );
        let settings = load(Some(&path), &dotenv(&[])).unwrap();
        assert!(settings.use_host_environment);
        assert!(!settings.response.show_size_and_time);
        assert!(
            settings.response.prettify_json,
            "untouched key keeps default"
        );
        assert_eq!(settings.focus.on_startup, StartupFocus::Url);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn dotenv_overrides_the_config_file() {
        let path = write_config("dotenv", "use_host_environment: false\n");
        let settings = load(
            Some(&path),
            &dotenv(&[("RUSTING_USE_HOST_ENVIRONMENT", "true")]),
        )
        .unwrap();
        assert!(settings.use_host_environment);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn nested_env_keys_split_on_double_underscore() {
        let mut merged = Mapping::new();
        overlay_prefixed(
            &mut merged,
            [
                ("RUSTING_HEADING__VISIBLE", "false"),
                ("RUSTING_SSL__CA_BUNDLE", "/tmp/ca.pem"),
                ("PATH", "/usr/bin"),
                ("RUSTING_PAGER", ""),
            ]
            .into_iter(),
        );
        let settings: Settings = serde_norway::from_value(Value::Mapping(merged)).unwrap();
        assert!(!settings.heading.visible);
        assert_eq!(settings.ssl.ca_bundle, Some(PathBuf::from("/tmp/ca.pem")));
    }

    #[test]
    fn empty_env_values_do_not_clobber() {
        let path = write_config("blank", "pager: less\n");
        let settings = load(Some(&path), &dotenv(&[("RUSTING_PAGER", "")])).unwrap();
        assert_eq!(settings.pager.as_deref(), Some("less"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_config_file_is_not_an_error() {
        let settings = load(Some(Path::new("/nonexistent/config.yaml")), &dotenv(&[])).unwrap();
        assert_eq!(settings.focus.on_startup, StartupFocus::Collection);
    }

    #[test]
    fn an_unknown_config_key_is_rejected() {
        let path = write_config("unknown", "definitely_not_a_setting: 1\n");
        let error = load(Some(&path), &dotenv(&[])).unwrap_err();
        assert!(
            format!("{error:#}").contains("definitely_not_a_setting"),
            "{error:#}"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn keymap_is_a_flat_map_of_binding_ids() {
        let path = write_config(
            "keymap",
            "keymap:\n  send-request: ctrl+enter\n  quit: ctrl+c,ctrl+q\n",
        );
        let settings = load(Some(&path), &dotenv(&[])).unwrap();
        assert_eq!(
            settings.keymap.get("send-request").map(String::as_str),
            Some("ctrl+enter")
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pager_for_json_prefers_pager_json() {
        let settings = Settings {
            pager: Some("less".into()),
            pager_json: Some("jless".into()),
            ..Settings::default()
        };
        assert_eq!(settings.pager_for(Some("json")), Some("jless"));
        assert_eq!(settings.pager_for(Some("html")), Some("less"));
        assert_eq!(settings.pager_for(None), Some("less"));
    }

    #[test]
    fn secret_detection_is_case_insensitive_and_gated() {
        let settings = Settings::default();
        assert!(settings.is_secret_name("API_TOKEN"));
        assert!(settings.is_secret_name("my_Password"));
        assert!(!settings.is_secret_name("POST_ID"));

        let settings = Settings {
            url_bar: UrlBarSettings {
                hide_secrets_in_value_preview: false,
                ..UrlBarSettings::default()
            },
            ..Settings::default()
        };
        assert!(!settings.is_secret_name("API_TOKEN"));
    }
}
