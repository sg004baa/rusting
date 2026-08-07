//! Inline span computation for single-line inputs.
//!
//! Pure functions: text in, styled byte ranges out. Nothing here touches the
//! terminal, so the rules are unit-testable without a backend.

use std::ops::Range;

use ratatui::style::{Modifier, Style};
use rusting_core::{PathParam, Variables, urls, variables};

use crate::theme;

/// A styled byte range within the input's value.
#[derive(Debug, Clone, PartialEq)]
pub struct Highlight {
    pub range: Range<usize>,
    pub style: Style,
}

/// Styles a URL: scheme, authority, every `/`, then variables and `:params` on
/// top, and finally underlines the token under the caret.
///
/// Later highlights win on overlap, which is why the ordering matters: a
/// variable inside the authority must beat the authority's own colour.
pub fn url(
    text: &str,
    vars: &Variables,
    path_params: &[PathParam],
    cursor: Option<usize>,
) -> Vec<Highlight> {
    let mut out = Vec::new();

    if let Some(scheme_end) = text.find("://")
        && urls::has_scheme(text)
    {
        out.push(Highlight {
            range: 0..scheme_end,
            style: theme::url::protocol(),
        });
        let authority_start = scheme_end + 3;
        let authority_end = text[authority_start..]
            .find(['/', '?', '#'])
            .map_or(text.len(), |index| authority_start + index);
        out.push(Highlight {
            range: authority_start..authority_end,
            style: theme::url::base(),
        });
    }

    // Every separator, wherever it appears.
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'/' {
            out.push(Highlight {
                range: index..index + 1,
                style: theme::url::separator(),
            });
        }
    }

    out.extend(variable_spans(text, vars));

    for token in urls::find_path_params(text) {
        let has_value = path_params
            .iter()
            .any(|param| param.name == token.name && !param.value.is_empty());
        out.push(Highlight {
            range: token.start..token.end,
            style: theme::path_param(has_value),
        });
    }

    if let Some(cursor) = cursor {
        out.extend(underline_token_at(text, cursor));
    }
    out
}

/// Styles just the variable tokens. Used by every plain input that accepts
/// variables: header values, query values, auth fields, proxy URL.
pub fn variables(text: &str, vars: &Variables, cursor: Option<usize>) -> Vec<Highlight> {
    let mut out = variable_spans(text, vars);
    if let Some(cursor) = cursor {
        out.extend(underline_token_at(text, cursor));
    }
    out
}

fn variable_spans(text: &str, vars: &Variables) -> Vec<Highlight> {
    variables::find_variables(text)
        .into_iter()
        .map(|token| Highlight {
            range: token.start..token.end,
            style: theme::variable(vars.contains_key(&token.name)),
        })
        .collect()
}

/// Underlines the variable or `:param` token the caret sits in, so it is clear
/// which token an autocompletion would replace.
fn underline_token_at(text: &str, cursor: usize) -> Option<Highlight> {
    let underline = Style::new().add_modifier(Modifier::UNDERLINED);
    if let Some(token) = variables::variable_at_cursor(text, cursor) {
        return Some(Highlight {
            range: token.start..token.end,
            style: underline,
        });
    }
    urls::find_path_params(text)
        .into_iter()
        .find(|token| cursor >= token.start && cursor <= token.end)
        .map(|token| Highlight {
            range: token.start..token.end,
            style: underline,
        })
}

/// Flattens overlapping highlights into a non-overlapping, ordered list where
/// later entries have been merged over earlier ones.
///
/// Renderers need disjoint spans; this is the only place that ordering rule is
/// applied.
pub fn flatten(text: &str, highlights: &[Highlight]) -> Vec<Highlight> {
    if highlights.is_empty() {
        return Vec::new();
    }
    // One style slot per byte is wasteful but trivially correct, and an input
    // line is at most a few hundred bytes.
    let mut per_byte: Vec<Option<Style>> = vec![None; text.len()];
    for highlight in highlights {
        let end = highlight.range.end.min(text.len());
        for slot in per_byte
            .get_mut(highlight.range.start.min(end)..end)
            .unwrap_or_default()
        {
            *slot = Some(match slot.take() {
                Some(existing) => existing.patch(highlight.style),
                None => highlight.style,
            });
        }
    }

    let mut out: Vec<Highlight> = Vec::new();
    let mut index = 0usize;
    while index < per_byte.len() {
        let Some(style) = per_byte[index] else {
            index += 1;
            continue;
        };
        let start = index;
        while index < per_byte.len() && per_byte[index] == Some(style) {
            index += 1;
        }
        // Never split a character in half.
        let mut end = index;
        while end < text.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        out.push(Highlight {
            range: start..end,
            style,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn vars(pairs: &[&str]) -> Variables {
        pairs
            .iter()
            .map(|k| ((*k).to_owned(), "v".to_owned()))
            .collect::<BTreeMap<_, _>>()
    }

    fn styles_at(text: &str, highlights: &[Highlight], needle: &str) -> Option<Style> {
        let start = text.find(needle)?;
        flatten(text, highlights)
            .into_iter()
            .find(|h| h.range.start <= start && start < h.range.end)
            .map(|h| h.style)
    }

    #[test]
    fn scheme_and_authority_are_coloured() {
        let text = "https://example.com/x";
        let highlights = url(text, &vars(&[]), &[], None);
        assert_eq!(
            styles_at(text, &highlights, "https"),
            Some(theme::url::protocol())
        );
        assert_eq!(
            styles_at(text, &highlights, "example"),
            Some(theme::url::base())
        );
    }

    #[test]
    fn a_schemeless_url_gets_no_authority_colour() {
        let text = "example.com/x";
        let highlights = url(text, &vars(&[]), &[], None);
        assert_eq!(styles_at(text, &highlights, "example"), None);
    }

    #[test]
    fn resolved_and_unresolved_variables_differ() {
        let text = "https://$HOST/$MISSING";
        let highlights = url(text, &vars(&["HOST"]), &[], None);
        assert_eq!(
            styles_at(text, &highlights, "$HOST").unwrap().fg,
            theme::variable(true).fg
        );
        assert_eq!(
            styles_at(text, &highlights, "$MISSING").unwrap().fg,
            theme::variable(false).fg
        );
    }

    #[test]
    fn a_variable_beats_the_authority_colour_it_sits_in() {
        let text = "https://$HOST/x";
        let highlights = url(text, &vars(&["HOST"]), &[], None);
        assert_eq!(
            styles_at(text, &highlights, "$HOST").unwrap().fg,
            theme::variable(true).fg
        );
    }

    #[test]
    fn path_params_reflect_whether_a_value_exists() {
        let text = "https://x.com/:filled/:empty";
        let params = vec![
            PathParam {
                name: "filled".into(),
                value: "1".into(),
            },
            PathParam {
                name: "empty".into(),
                value: String::new(),
            },
        ];
        let highlights = url(text, &vars(&[]), &params, None);
        assert_eq!(
            styles_at(text, &highlights, ":filled").unwrap().fg,
            theme::path_param(true).fg
        );
        assert_eq!(
            styles_at(text, &highlights, ":empty").unwrap().fg,
            theme::path_param(false).fg
        );
    }

    #[test]
    fn the_token_under_the_caret_is_underlined() {
        let text = "https://x.com/$ID/y";
        let cursor = text.find("$ID").unwrap() + 2;
        let highlights = url(text, &vars(&["ID"]), &[], Some(cursor));
        let style = styles_at(text, &highlights, "$ID").unwrap();
        assert!(style.add_modifier.contains(Modifier::UNDERLINED));
        // A neighbouring segment is not underlined.
        let other = styles_at(text, &highlights, "x.com").unwrap();
        assert!(!other.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn flatten_produces_disjoint_ordered_spans() {
        let text = "abcdef";
        let highlights = vec![
            Highlight {
                range: 0..4,
                style: Style::new().fg(theme::ACCENT),
            },
            Highlight {
                range: 2..6,
                style: Style::new().add_modifier(Modifier::BOLD),
            },
        ];
        let flat = flatten(text, &highlights);
        assert_eq!(
            flat.iter().map(|h| h.range.clone()).collect::<Vec<_>>(),
            vec![0..2, 2..4, 4..6]
        );
        assert_eq!(flat[1].style.fg, Some(theme::ACCENT));
        assert!(flat[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(flat[2].style.fg, None);
    }

    #[test]
    fn flatten_never_splits_a_multibyte_character() {
        let text = "a日本b";
        let highlights = vec![Highlight {
            range: 0..2,
            style: Style::new().fg(theme::ACCENT),
        }];
        for span in flatten(text, &highlights) {
            assert!(text.is_char_boundary(span.range.start));
            assert!(text.is_char_boundary(span.range.end));
        }
    }

    #[test]
    fn out_of_range_highlights_are_clamped() {
        let text = "ab";
        let flat = flatten(
            text,
            &[Highlight {
                range: 1..99,
                style: Style::new().fg(theme::ACCENT),
            }],
        );
        assert_eq!(
            flat,
            vec![Highlight {
                range: 1..2,
                style: Style::new().fg(theme::ACCENT)
            }]
        );
    }
}
