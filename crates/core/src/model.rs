//! The on-disk request model.
//!
//! A collection is a directory tree; every `*.posting.yaml` file inside it
//! deserializes into a [`RequestModel`]. Unknown keys are ignored so files
//! written by other tools still load.
//!
//! Serialization omits any field equal to its default, which is what keeps the
//! files small and readable. [`crate::yaml`] owns the emitter.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The file suffix identifying a request inside a collection directory.
pub const REQUEST_SUFFIX: &str = ".posting.yaml";

/// Version stamped into newly written request files.
pub const RUSTING_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    pub const ALL: [HttpMethod; 7] = [
        HttpMethod::Get,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Delete,
        HttpMethod::Patch,
        HttpMethod::Head,
        HttpMethod::Options,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
        }
    }

    /// Sort rank used when ordering requests inside a collection node.
    pub const fn sort_rank(self) -> u8 {
        match self {
            HttpMethod::Get => 0,
            HttpMethod::Post => 1,
            HttpMethod::Put => 2,
            HttpMethod::Patch => 3,
            HttpMethod::Delete => 4,
            HttpMethod::Head => 5,
            HttpMethod::Options => 6,
        }
    }

    /// The mnemonic letter used to pick this method from the selector.
    pub const fn mnemonic(self) -> char {
        match self {
            HttpMethod::Get => 'g',
            HttpMethod::Post => 'p',
            HttpMethod::Put => 'u',
            HttpMethod::Delete => 'd',
            HttpMethod::Patch => 'a',
            HttpMethod::Head => 'h',
            HttpMethod::Options => 'o',
        }
    }

    /// Index of the mnemonic letter within [`Self::as_str`], for underlining.
    pub const fn mnemonic_index(self) -> usize {
        match self {
            HttpMethod::Get
            | HttpMethod::Post
            | HttpMethod::Delete
            | HttpMethod::Head
            | HttpMethod::Options => 0,
            HttpMethod::Put | HttpMethod::Patch => 1,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|method| method.as_str().eq_ignore_ascii_case(text))
    }
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A `name`/`value` row with an enable toggle. Backs headers, query params,
/// form fields and cookies — all four are the same shape on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

impl KeyValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }
}

impl Default for KeyValue {
    fn default() -> Self {
        Self {
            name: String::new(),
            value: String::new(),
            enabled: true,
        }
    }
}

/// A `:name` placeholder extracted from the URL path. Has no enable toggle:
/// every placeholder in the URL must be filled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathParam {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    Basic,
    Digest,
    BearerToken,
}

impl AuthKind {
    pub const ALL: [AuthKind; 3] = [AuthKind::Basic, AuthKind::Digest, AuthKind::BearerToken];

    pub const fn label(self) -> &'static str {
        match self {
            AuthKind::Basic => "Basic",
            AuthKind::Digest => "Digest",
            AuthKind::BearerToken => "Bearer Token",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPassAuth {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BearerTokenAuth {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
}

/// Auth is a flat struct rather than a tagged union: `kind` selects which
/// payload is live, and the other payloads survive a round trip so switching
/// auth type in the UI does not destroy the credentials you already typed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auth {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AuthKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basic: Option<UserPassAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<UserPassAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<BearerTokenAuth>,
}

impl Auth {
    pub fn basic(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            kind: Some(AuthKind::Basic),
            basic: Some(UserPassAuth {
                username: username.into(),
                password: password.into(),
            }),
            ..Self::default()
        }
    }

    pub fn digest(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            kind: Some(AuthKind::Digest),
            digest: Some(UserPassAuth {
                username: username.into(),
                password: password.into(),
            }),
            ..Self::default()
        }
    }

    pub fn bearer(token: impl Into<String>) -> Self {
        Self {
            kind: Some(AuthKind::BearerToken),
            bearer_token: Some(BearerTokenAuth {
                token: token.into(),
            }),
            ..Self::default()
        }
    }

    /// True when there is nothing worth writing to disk.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// How the request body is supplied. `None` means no body at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BodyContent {
    Raw {
        content: String,
        /// Mirrors the editor's language selection; drives the generated
        /// `content-type` header when the user has not set one themselves.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    Form {
        form_data: Vec<KeyValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

impl BodyContent {
    pub const FORM_CONTENT_TYPE: &'static str = "application/x-www-form-urlencoded";
    pub const MULTIPART_CONTENT_TYPE: &'static str = "multipart/form-data";

    pub fn content_type(&self) -> Option<&str> {
        match self {
            BodyContent::Raw { content_type, .. } | BodyContent::Form { content_type, .. } => {
                content_type.as_deref()
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scripts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_response: Option<String>,
}

impl Scripts {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Which lifecycle hook a script reference belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptHook {
    Setup,
    OnRequest,
    OnResponse,
}

impl ScriptHook {
    pub const ALL: [ScriptHook; 3] = [
        ScriptHook::Setup,
        ScriptHook::OnRequest,
        ScriptHook::OnResponse,
    ];

    /// The default exported function name, which equals the YAML key.
    pub const fn default_function(self) -> &'static str {
        match self {
            ScriptHook::Setup => "setup",
            ScriptHook::OnRequest => "on_request",
            ScriptHook::OnResponse => "on_response",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            ScriptHook::Setup => "Setup",
            ScriptHook::OnRequest => "Pre-request",
            ScriptHook::OnResponse => "Post-response",
        }
    }
}

/// A `path/to/script.js:function_name` reference. The function name is
/// optional and defaults to [`ScriptHook::default_function`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptRef {
    pub path: PathBuf,
    pub function: String,
}

impl ScriptRef {
    /// Splits a raw `scripts.*` value on the last colon only when the suffix is
    /// a valid JavaScript export identifier.
    pub fn parse(raw: &str, hook: ScriptHook) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        match raw.rsplit_once(':') {
            Some((path, function))
                if !path.is_empty() && is_javascript_export_identifier(function) =>
            {
                Some(Self {
                    path: PathBuf::from(path),
                    function: function.to_owned(),
                })
            }
            _ => Some(Self {
                path: PathBuf::from(raw),
                function: hook.default_function().to_owned(),
            }),
        }
    }
}

fn is_javascript_export_identifier(identifier: &str) -> bool {
    let mut bytes = identifier.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first == b'_' || first == b'$' || first.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte == b'$' || byte.is_ascii_alphanumeric())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Options {
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub follow_redirects: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub verify_ssl: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub attach_cookies: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proxy_url: String,
    #[serde(
        default = "default_timeout",
        skip_serializing_if = "is_default_timeout"
    )]
    pub timeout: f64,
}

pub const DEFAULT_TIMEOUT: f64 = 5.0;

impl Default for Options {
    fn default() -> Self {
        Self {
            follow_redirects: true,
            verify_ssl: true,
            attach_cookies: true,
            proxy_url: String::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl Options {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// One `*.posting.yaml` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestModel {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "is_get")]
    pub method: HttpMethod,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<Auth>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path_params: Vec<PathParam>,
    #[serde(default, skip_serializing_if = "Scripts::is_empty")]
    pub scripts: Scripts,
    #[serde(default, skip_serializing_if = "Options::is_default")]
    pub options: Options,

    /// Where this request lives on disk. Injected at load time, never read
    /// from or written to YAML. `None` means "not saved yet".
    #[serde(skip)]
    pub path: Option<PathBuf>,
    /// Cookies harvested from previous responses in this session. Session
    /// state, never persisted.
    #[serde(skip)]
    pub cookies: Vec<KeyValue>,
}

impl Default for RequestModel {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            method: HttpMethod::Get,
            url: String::new(),
            body: None,
            auth: None,
            headers: Vec::new(),
            params: Vec::new(),
            path_params: Vec::new(),
            scripts: Scripts::default(),
            options: Options::default(),
            path: None,
            cookies: Vec::new(),
        }
    }
}

impl RequestModel {
    /// Ordering key used for sorted insertion into the collection tree.
    pub fn sort_key(&self) -> (u8, &str) {
        (self.method.sort_rank(), self.name.as_str())
    }

    /// The script reference for a hook, if configured.
    pub fn script_ref(&self, hook: ScriptHook) -> Option<ScriptRef> {
        let raw = match hook {
            ScriptHook::Setup => self.scripts.setup.as_deref(),
            ScriptHook::OnRequest => self.scripts.on_request.as_deref(),
            ScriptHook::OnResponse => self.scripts.on_response.as_deref(),
        }?;
        ScriptRef::parse(raw, hook)
    }

    /// Enabled rows only, in declaration order.
    pub fn enabled_headers(&self) -> impl Iterator<Item = &KeyValue> {
        self.headers.iter().filter(|kv| kv.enabled)
    }

    pub fn enabled_params(&self) -> impl Iterator<Item = &KeyValue> {
        self.params.iter().filter(|kv| kv.enabled)
    }

    pub fn enabled_cookies(&self) -> impl Iterator<Item = &KeyValue> {
        self.cookies.iter().filter(|kv| kv.enabled)
    }

    /// True when the user has set a `content-type` header themselves, in which
    /// case the body's own content type must not be injected.
    pub fn has_explicit_content_type(&self) -> bool {
        self.enabled_headers()
            .any(|kv| kv.name.eq_ignore_ascii_case("content-type"))
    }
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_get(method: &HttpMethod) -> bool {
    *method == HttpMethod::Get
}

fn default_timeout() -> f64 {
    DEFAULT_TIMEOUT
}

fn is_default_timeout(timeout: &f64) -> bool {
    *timeout == DEFAULT_TIMEOUT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_ref_defaults_function_to_hook_name() {
        let parsed = ScriptRef::parse("scripts/hooks.js", ScriptHook::OnResponse).unwrap();
        assert_eq!(parsed.path, PathBuf::from("scripts/hooks.js"));
        assert_eq!(parsed.function, "on_response");
    }

    #[test]
    fn script_ref_takes_explicit_function() {
        let parsed = ScriptRef::parse("a/b.js:prepare", ScriptHook::Setup).unwrap();
        assert_eq!(parsed.path, PathBuf::from("a/b.js"));
        assert_eq!(parsed.function, "prepare");
    }

    #[test]
    fn script_ref_keeps_colons_that_do_not_precede_an_export_identifier() {
        let parsed = ScriptRef::parse(r"C:\scripts\hook.js", ScriptHook::OnRequest).unwrap();
        assert_eq!(parsed.path, PathBuf::from(r"C:\scripts\hook.js"));
        assert_eq!(parsed.function, "on_request");

        let parsed = ScriptRef::parse("scripts:archive/hook.js", ScriptHook::Setup).unwrap();
        assert_eq!(parsed.path, PathBuf::from("scripts:archive/hook.js"));
        assert_eq!(parsed.function, "setup");
    }

    #[test]
    fn script_ref_does_not_split_invalid_export_identifiers() {
        for raw in ["scripts/hook.js:123handler", "scripts/hook.js:not-valid"] {
            let parsed = ScriptRef::parse(raw, ScriptHook::Setup).unwrap();
            assert_eq!(parsed.path, PathBuf::from(raw));
            assert_eq!(parsed.function, "setup");
        }
    }

    #[test]
    fn script_ref_splits_windows_path_from_valid_export_identifier() {
        let parsed = ScriptRef::parse(r"C:\scripts\hook.js:on_request", ScriptHook::Setup).unwrap();
        assert_eq!(parsed.path, PathBuf::from(r"C:\scripts\hook.js"));
        assert_eq!(parsed.function, "on_request");
    }

    #[test]
    fn script_ref_accepts_javascript_identifier_punctuation() {
        let parsed = ScriptRef::parse("scripts/hook.js:$handler", ScriptHook::Setup).unwrap();
        assert_eq!(parsed.path, PathBuf::from("scripts/hook.js"));
        assert_eq!(parsed.function, "$handler");
    }

    #[test]
    fn body_content_types_are_distinct() {
        assert_eq!(
            BodyContent::FORM_CONTENT_TYPE,
            "application/x-www-form-urlencoded"
        );
        assert_eq!(BodyContent::MULTIPART_CONTENT_TYPE, "multipart/form-data");
    }

    #[test]
    fn blank_script_ref_is_none() {
        assert!(ScriptRef::parse("   ", ScriptHook::Setup).is_none());
    }

    #[test]
    fn method_sort_rank_orders_get_first() {
        let mut methods = vec![HttpMethod::Delete, HttpMethod::Get, HttpMethod::Post];
        methods.sort_by_key(|m| m.sort_rank());
        assert_eq!(
            methods,
            vec![HttpMethod::Get, HttpMethod::Post, HttpMethod::Delete]
        );
    }

    #[test]
    fn mnemonic_index_points_at_the_mnemonic_letter() {
        for method in HttpMethod::ALL {
            let name = method.as_str();
            let letter = name.as_bytes()[method.mnemonic_index()].to_ascii_lowercase();
            assert_eq!(letter as char, method.mnemonic(), "{name}");
        }
    }
}
