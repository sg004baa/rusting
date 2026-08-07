//! URL utilities: scheme inference and `:name` path placeholders.
//!
//! Deliberately string-based rather than built on the `url` crate. The URL in
//! the editor is routinely not a valid URL — it is being typed, it contains
//! `$VARS` and `:placeholders`, and it may have no scheme yet. A strict parser
//! would reject or normalise all of that.

/// Splits a URL into `(prefix, path, suffix)` where `path` is the portion
/// placeholders may appear in, and `suffix` starts at the first `?` or `#`.
fn split_path(url: &str) -> (&str, &str, &str) {
    let after_scheme = match url.find("://") {
        Some(index) => index + 3,
        None => 0,
    };
    // The authority ends at the first `/` after the scheme.
    let path_start = url[after_scheme..]
        .find('/')
        .map_or(url.len(), |index| after_scheme + index);
    let path_end = url[path_start..]
        .find(['?', '#'])
        .map_or(url.len(), |index| path_start + index);
    (
        &url[..path_start],
        &url[path_start..path_end],
        &url[path_end..],
    )
}

/// Prepends `http://` unless the URL already carries a scheme.
///
/// `localhost:8000` must not be mistaken for a scheme, so a scheme is only
/// recognised when followed by `//`.
pub fn ensure_protocol(url: &str) -> String {
    if has_scheme(url) {
        url.to_owned()
    } else {
        format!("http://{url}")
    }
}

pub fn has_scheme(url: &str) -> bool {
    let Some(index) = url.find("://") else {
        return false;
    };
    let scheme = &url[..index];
    !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// A `:name` placeholder located in the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathParamToken {
    pub name: String,
    /// Byte offset of the `:` within the whole URL.
    pub start: usize,
    /// Byte offset one past the end of the name, within the whole URL.
    pub end: usize,
}

/// Locates every unescaped `:name` in the path. `::name` is an escape for a
/// literal `:name` and yields nothing.
pub fn find_path_params(url: &str) -> Vec<PathParamToken> {
    let (prefix, path, _) = split_path(url);
    let base = prefix.len();
    let bytes = path.as_bytes();
    let mut found = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b':' {
            index += 1;
            continue;
        }
        // `::` is the escape; skip both colons so the second is not a marker.
        if bytes.get(index + 1) == Some(&b':') {
            index += 2;
            continue;
        }
        let name_start = index + 1;
        let mut cursor = name_start;
        while cursor < bytes.len() && is_name_byte(bytes[cursor], cursor == name_start) {
            cursor += 1;
        }
        if cursor == name_start {
            index += 1;
            continue;
        }
        found.push(PathParamToken {
            name: path[name_start..cursor].to_owned(),
            start: base + index,
            end: base + cursor,
        });
        index = cursor;
    }
    found
}

/// Placeholder names in first-appearance order, deduplicated.
pub fn path_param_names(url: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for token in find_path_params(url) {
        if !names.contains(&token.name) {
            names.push(token.name);
        }
    }
    names
}

/// Replaces `:name` placeholders in the path with their values and unescapes
/// `::` to `:`. Placeholders with no supplied value are left as they are.
pub fn substitute_path_params(url: &str, values: &(impl PathParamLookup + ?Sized)) -> String {
    let (prefix, path, suffix) = split_path(url);
    let tokens = find_path_params(url);
    let base = prefix.len();

    let mut rebuilt = String::with_capacity(path.len());
    let mut cursor = 0usize;
    for token in &tokens {
        let local_start = token.start - base;
        let local_end = token.end - base;
        rebuilt.push_str(&path[cursor..local_start]);
        match values.value_for(&token.name) {
            Some(value) => rebuilt.push_str(value),
            None => rebuilt.push_str(&path[local_start..local_end]),
        }
        cursor = local_end;
    }
    rebuilt.push_str(&path[cursor..]);

    format!("{prefix}{}{suffix}", rebuilt.replace("::", ":"))
}

/// Lets both a map and a slice of [`crate::model::PathParam`] drive
/// [`substitute_path_params`] without an intermediate allocation.
pub trait PathParamLookup {
    fn value_for(&self, name: &str) -> Option<&str>;
}

impl PathParamLookup for [crate::model::PathParam] {
    fn value_for(&self, name: &str) -> Option<&str> {
        self.iter()
            .find(|param| param.name == name)
            .map(|param| param.value.as_str())
    }
}

impl PathParamLookup for std::collections::BTreeMap<String, String> {
    fn value_for(&self, name: &str) -> Option<&str> {
        self.get(name).map(String::as_str)
    }
}

/// The `scheme://authority` prefix, used to build the URL bar's autocomplete
/// candidates from the collection.
pub fn base_url(url: &str) -> Option<String> {
    let index = url.find("://")?;
    if !has_scheme(url) {
        return None;
    }
    let after = index + 3;
    let end = url[after..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |offset| after + offset);
    if end == after {
        return None;
    }
    Some(url[..end].to_owned())
}

fn is_name_byte(byte: u8, first: bool) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic() || (!first && byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn ensure_protocol_leaves_existing_schemes_alone() {
        assert_eq!(ensure_protocol("https://a.com"), "https://a.com");
        assert_eq!(ensure_protocol("ws://a.com"), "ws://a.com");
        assert_eq!(ensure_protocol("a.com"), "http://a.com");
    }

    #[test]
    fn a_port_is_not_a_scheme() {
        assert_eq!(ensure_protocol("localhost:8000"), "http://localhost:8000");
        assert_eq!(
            ensure_protocol("localhost:8000/x"),
            "http://localhost:8000/x"
        );
    }

    #[test]
    fn finds_placeholders_only_in_the_path() {
        let names = path_param_names("https://a.com/posts/:id/c?x=:notme#:norme");
        assert_eq!(names, vec!["id"]);
    }

    #[test]
    fn double_colon_escapes() {
        assert_eq!(path_param_names("https://a.com/::id"), Vec::<String>::new());
        assert_eq!(path_param_names("https://a.com/::id/:id"), vec!["id"]);
    }

    #[test]
    fn substitutes_and_unescapes() {
        let values = map(&[("id", "123")]);
        assert_eq!(
            substitute_path_params("https://a.com/::id/:id", &values),
            "https://a.com/:id/123"
        );
    }

    #[test]
    fn unfilled_placeholders_survive() {
        let values = map(&[]);
        assert_eq!(
            substitute_path_params("https://a.com/:id", &values),
            "https://a.com/:id"
        );
    }

    #[test]
    fn query_and_fragment_are_untouched() {
        let values = map(&[("id", "1")]);
        assert_eq!(
            substitute_path_params("https://a.com/:id?a=::b#:c", &values),
            "https://a.com/1?a=::b#:c"
        );
    }

    #[test]
    fn names_are_deduplicated_in_first_appearance_order() {
        assert_eq!(path_param_names("https://a.com/:b/:a/:b"), vec!["b", "a"]);
    }

    #[test]
    fn token_offsets_index_the_whole_url() {
        let url = "https://a.com/posts/:id";
        let token = &find_path_params(url)[0];
        assert_eq!(&url[token.start..token.end], ":id");
    }

    #[test]
    fn base_url_extracts_scheme_and_authority() {
        assert_eq!(
            base_url("https://a.com/x/y?z=1").as_deref(),
            Some("https://a.com")
        );
        assert_eq!(base_url("https://a.com").as_deref(), Some("https://a.com"));
        assert_eq!(base_url("a.com/x"), None);
        assert_eq!(base_url("https://"), None);
    }
}
