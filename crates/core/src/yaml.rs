//! YAML load/save for request files.
//!
//! Parsing goes through `serde_norway`, which is lenient and handles anything a
//! user is likely to hand-write. Emitting is done by the writer in this module
//! rather than by `serde_norway::to_string`, for two reasons:
//!
//! * multi-line strings must come out as block literals (`content: |-`), which
//!   is what makes a JSON request body readable on disk;
//! * key order must follow the struct's field declaration order, not be sorted.

use std::fmt::Write as _;

use serde::Serialize;
use serde_norway::Value;

/// Serializes a value to the request-file YAML dialect.
///
/// Errors only if the value cannot be represented as YAML at all.
pub fn to_string<T: Serialize>(value: &T) -> Result<String, serde_norway::Error> {
    let value = serde_norway::to_value(value)?;
    let mut out = String::new();
    match &value {
        Value::Mapping(map) if map.is_empty() => out.push_str("{}\n"),
        Value::Mapping(_) | Value::Sequence(_) => {
            write_node(&mut out, &value, 0, Position::TopLevel);
        }
        _ => write_scalar_inline(&mut out, &value, 0),
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

pub fn from_str<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, serde_norway::Error> {
    serde_norway::from_str(text)
}

/// Where the node being written sits, which decides whether it needs a leading
/// newline and how deeply it indents.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    TopLevel,
    /// Directly after a `key:` on the same line.
    AfterKey,
    /// Directly after a `- ` sequence dash on the same line.
    AfterDash,
}

fn write_node(out: &mut String, value: &Value, indent: usize, position: Position) {
    match value {
        Value::Mapping(map) => {
            if map.is_empty() {
                finish_inline(out, "{}", position);
                return;
            }
            // A mapping that follows a `- ` puts its first key on the dash
            // line and indents the rest to line up under it.
            let inline_first = position == Position::AfterDash;
            for (index, (key, child)) in map.iter().enumerate() {
                if index > 0 || !inline_first {
                    if index > 0 || position != Position::TopLevel {
                        out.push('\n');
                    }
                    push_indent(out, indent);
                }
                write_key(out, key);
                out.push(':');
                write_child(out, child, indent);
            }
        }
        Value::Sequence(items) => {
            if items.is_empty() {
                finish_inline(out, "[]", position);
                return;
            }
            for (index, item) in items.iter().enumerate() {
                if index > 0 || position != Position::TopLevel {
                    out.push('\n');
                }
                push_indent(out, indent);
                out.push_str("- ");
                write_node(out, item, indent + 2, Position::AfterDash);
            }
        }
        scalar => {
            if position == Position::AfterKey {
                out.push(' ');
            }
            write_scalar_inline(out, scalar, indent);
        }
    }
}

/// Writes the value that follows a `key:` we just emitted.
fn write_child(out: &mut String, child: &Value, indent: usize) {
    match child {
        Value::Mapping(map) if !map.is_empty() => {
            out.push('\n');
            push_indent(out, indent + 2);
            write_mapping_body(out, map, indent + 2);
        }
        // Sequences sit at the parent's indent level, PyYAML style:
        //   headers:
        //   - name: a
        Value::Sequence(items) if !items.is_empty() => {
            for item in items {
                out.push('\n');
                push_indent(out, indent);
                out.push_str("- ");
                write_node(out, item, indent + 2, Position::AfterDash);
            }
        }
        other => {
            out.push(' ');
            write_scalar_inline(out, other, indent);
        }
    }
}

fn write_mapping_body(out: &mut String, map: &serde_norway::Mapping, indent: usize) {
    for (index, (key, child)) in map.iter().enumerate() {
        if index > 0 {
            out.push('\n');
            push_indent(out, indent);
        }
        write_key(out, key);
        out.push(':');
        write_child(out, child, indent);
    }
}

fn finish_inline(out: &mut String, literal: &str, position: Position) {
    if position == Position::AfterKey {
        out.push(' ');
    }
    out.push_str(literal);
}

fn write_key(out: &mut String, key: &Value) {
    match key {
        Value::String(text) => out.push_str(&format_plain_or_quoted(text)),
        other => write_scalar_inline(out, other, 0),
    }
}

fn write_scalar_inline(out: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&format_number(number)),
        Value::String(text) => write_string(out, text, indent),
        // Tagged and nested values only reach here through hand-built Values.
        other => {
            let _ = write!(out, "{}", format_plain_or_quoted(&format!("{other:?}")));
        }
    }
}

/// Numbers must round-trip: a float that lost its fractional part would come
/// back as an integer and change `timeout: 5.0` into `timeout: 5`.
fn format_number(number: &serde_norway::Number) -> String {
    if let Some(int) = number.as_i64() {
        return int.to_string();
    }
    if let Some(uint) = number.as_u64() {
        return uint.to_string();
    }
    match number.as_f64() {
        Some(float) if float.is_nan() => ".nan".to_owned(),
        Some(float) if float.is_infinite() => {
            if float.is_sign_negative() {
                "-.inf".to_owned()
            } else {
                ".inf".to_owned()
            }
        }
        Some(float) => {
            let rendered = float.to_string();
            if rendered.contains(['.', 'e', 'E']) {
                rendered
            } else {
                format!("{rendered}.0")
            }
        }
        None => "null".to_owned(),
    }
}

fn write_string(out: &mut String, text: &str, indent: usize) {
    if let Some(block) = format_block_literal(text, indent) {
        out.push_str(&block);
    } else {
        out.push_str(&format_plain_or_quoted(text));
    }
}

/// Renders a multi-line string as a block literal, or returns `None` when the
/// string is single-line or contains something a block literal cannot carry.
fn format_block_literal(text: &str, indent: usize) -> Option<String> {
    if !text.contains('\n') {
        return None;
    }
    // A leading space or tab, or a tab anywhere at the start of a line, makes
    // the block's indentation ambiguous.
    if text.starts_with([' ', '\t']) {
        return None;
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return None;
    }

    let trailing_newlines = text.len() - text.trim_end_matches('\n').len();
    let body = text.trim_end_matches('\n');
    // Trailing whitespace on a line is silently eaten by any YAML reader, so
    // strip it here and keep the file honest about what will load back.
    let lines: Vec<&str> = body.split('\n').map(|line| line.trim_end()).collect();
    if lines.iter().any(|line| line.starts_with('\t')) {
        return None;
    }

    let chomping = match trailing_newlines {
        0 => "-",
        1 => "",
        _ => return None, // `|+` round-trips poorly; fall back to a quoted scalar.
    };

    let body_indent = indent + 2;
    let mut out = format!("|{chomping}");
    for line in lines {
        out.push('\n');
        if !line.is_empty() {
            for _ in 0..body_indent {
                out.push(' ');
            }
            out.push_str(line);
        }
    }
    Some(out)
}

/// Decides between a plain scalar and a quoted one, and quotes if needed.
fn format_plain_or_quoted(text: &str) -> String {
    if needs_quoting(text) {
        quote(text)
    } else {
        text.to_owned()
    }
}

fn needs_quoting(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if text != text.trim() {
        return true;
    }
    if text.contains('\n') || text.chars().any(|c| c.is_control()) {
        return true;
    }
    if text.contains(": ") || text.ends_with(':') || text.contains(" #") {
        return true;
    }
    // A leading indicator character makes the scalar something other than a
    // string, or is outright invalid.
    let first = text.as_bytes()[0];
    if b"-?:,[]{}#&*!|>'\"%@`".contains(&first) {
        // `-` and `?` are only indicators when followed by a space, but a
        // string like `-1` would parse as a number anyway, so quoting here is
        // both correct and simpler.
        return true;
    }
    // Anything YAML would read back as a non-string must be quoted.
    parses_as_non_string(text)
}

fn parses_as_non_string(text: &str) -> bool {
    matches!(
        serde_norway::from_str::<Value>(text),
        Ok(Value::Bool(_) | Value::Number(_) | Value::Null)
    )
}

fn quote(text: &str) -> String {
    // Single quotes keep backslashes literal, which matters for regex-ish and
    // Windows-path values. Fall back to double quotes only when the string
    // holds something a single-quoted scalar cannot express.
    if text.chars().any(|c| c.is_control()) {
        let mut out = String::with_capacity(text.len() + 2);
        out.push('"');
        for c in text.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => {
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out.push('"');
        return out;
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        if c == '\'' {
            out.push_str("''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push(' ');
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{
        Auth, BodyContent, HttpMethod, KeyValue, Options, PathParam, RequestModel, Scripts,
    };

    fn round_trip(model: &RequestModel) -> RequestModel {
        let text = super::to_string(model).expect("serialize");
        super::from_str::<RequestModel>(&text).expect("deserialize")
    }

    #[test]
    fn minimal_request_omits_every_default() {
        let model = RequestModel {
            name: "get random user".into(),
            url: "https://api.randomuser.me".into(),
            ..Default::default()
        };
        let text = super::to_string(&model).unwrap();
        assert_eq!(
            text,
            "name: get random user\nurl: https://api.randomuser.me\n"
        );
    }

    #[test]
    fn multiline_body_becomes_a_block_literal() {
        let model = RequestModel {
            name: "echo post".into(),
            method: HttpMethod::Post,
            body: Some(BodyContent::Raw {
                content: "{\n  \"a\": 1\n}".into(),
                content_type: Some("application/json".into()),
            }),
            ..Default::default()
        };
        let text = super::to_string(&model).unwrap();
        assert_eq!(
            text,
            "name: echo post\n\
             method: POST\n\
             body:\n  \
             content: |-\n    \
             {\n      \
             \"a\": 1\n    \
             }\n  \
             content_type: application/json\n"
        );
        assert_eq!(round_trip(&model), model);
    }

    #[test]
    fn sequences_sit_at_the_parent_indent() {
        let model = RequestModel {
            headers: vec![
                KeyValue::new("Content-Type", "application/json"),
                KeyValue {
                    name: "Accept".into(),
                    value: "*".into(),
                    enabled: false,
                },
            ],
            ..Default::default()
        };
        let text = super::to_string(&model).unwrap();
        assert_eq!(
            text,
            "headers:\n\
             - name: Content-Type\n  \
             value: application/json\n\
             - name: Accept\n  \
             value: '*'\n  \
             enabled: false\n"
        );
        assert_eq!(round_trip(&model), model);
    }

    #[test]
    fn ambiguous_scalars_are_quoted() {
        let model = RequestModel {
            params: vec![
                KeyValue::new("n", "123"),
                KeyValue::new("b", "true"),
                KeyValue::new("e", ""),
                KeyValue::new("y", "yes"),
                KeyValue::new("c", "a: b"),
            ],
            ..Default::default()
        };
        let text = super::to_string(&model).unwrap();
        assert!(text.contains("value: '123'"), "{text}");
        assert!(text.contains("value: 'true'"), "{text}");
        assert!(text.contains("value: ''"), "{text}");
        assert!(text.contains("value: 'a: b'"), "{text}");
        assert_eq!(round_trip(&model), model);
    }

    #[test]
    fn float_timeout_keeps_its_fraction() {
        let model = RequestModel {
            options: Options {
                timeout: 0.2,
                ..Default::default()
            },
            ..Default::default()
        };
        let text = super::to_string(&model).unwrap();
        assert_eq!(text, "options:\n  timeout: 0.2\n");
        assert_eq!(round_trip(&model), model);
    }

    #[test]
    fn whole_default_options_block_is_omitted_but_one_change_is_not() {
        let model = RequestModel {
            options: Options {
                follow_redirects: false,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            super::to_string(&model).unwrap(),
            "options:\n  follow_redirects: false\n"
        );
    }

    #[test]
    fn full_shape_round_trips() {
        let model = RequestModel {
            name: "echo".into(),
            description: "line one\nline two".into(),
            method: HttpMethod::Put,
            url: "https://example.com/:id".into(),
            body: Some(BodyContent::Form {
                form_data: vec![KeyValue::new("something", "123")],
                content_type: Some(BodyContent::FORM_CONTENT_TYPE.into()),
            }),
            auth: Some(Auth::digest("darren", "")),
            headers: vec![KeyValue::new("X-Setup-Var", "$setup_var")],
            params: vec![KeyValue::new("q", "1")],
            path_params: vec![PathParam {
                name: "id".into(),
                value: "42".into(),
            }],
            scripts: Scripts {
                setup: Some("scripts/hooks.js".into()),
                on_request: Some("scripts/hooks.js:prepare".into()),
                on_response: None,
            },
            options: Options {
                verify_ssl: false,
                timeout: 12.5,
                proxy_url: "http://localhost:8080".into(),
                ..Default::default()
            },
            path: None,
            cookies: Vec::new(),
        };
        assert_eq!(round_trip(&model), model);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let text = "name: legacy\nposting_version: 2.10.0\nsomething_else: 1\n";
        let model: RequestModel = super::from_str(text).unwrap();
        assert_eq!(model.name, "legacy");
    }

    #[test]
    fn legacy_posting_files_load() {
        let text = "\
name: echo
description: An echo server.
url: https://postman-echo.com/get
body:
  form_data:
  - name: something
    value: '123'
headers:
- name: X-Setup-Var
  value: $setup_var
options:
  follow_redirects: false
";
        let model: RequestModel = super::from_str(text).unwrap();
        assert_eq!(model.method, HttpMethod::Get);
        assert_eq!(model.headers.len(), 1);
        assert!(model.headers[0].enabled, "enabled defaults to true");
        assert!(!model.options.follow_redirects);
        assert!(model.options.verify_ssl, "unset options keep defaults");
        match model.body {
            Some(BodyContent::Form { ref form_data, .. }) => {
                assert_eq!(form_data[0].value, "123");
            }
            other => panic!("expected form body, got {other:?}"),
        }
    }
}
