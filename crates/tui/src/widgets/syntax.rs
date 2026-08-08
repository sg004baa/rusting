//! Tree-sitter syntax highlighting for multiline editors.

use std::ops::Range;

use tree_sitter_highlight::{
    HighlightConfiguration, HighlightEvent, Highlighter as TreeSitterHighlighter,
};

use crate::theme;
use crate::widgets::Highlight;

const HIGHLIGHT_NAMES: &[&str] = &[
    "string.special",
    "string",
    "number",
    "boolean",
    "constant.builtin",
    "constant",
    "property",
    "tag",
    "attribute",
    "comment",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation",
];

const JSON_HIGHLIGHTS_QUERY: &str = r#"
(string) @string
(pair key: (string) @property)
(number) @number
[(true) (false)] @boolean
(null) @constant
(escape_sequence) @string.special
(comment) @comment
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Json,
    Html,
    Css,
}

impl Language {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "json" => Some(Self::Json),
            "html" => Some(Self::Html),
            "css" => Some(Self::Css),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html => "html",
            Self::Css => "css",
        }
    }

    pub const fn extension(self) -> &'static str {
        self.name()
    }
}

pub struct Highlighter {
    highlighter: TreeSitterHighlighter,
    json: HighlightConfiguration,
    html: HighlightConfiguration,
}

impl Highlighter {
    pub fn new() -> Self {
        let mut json = HighlightConfiguration::new(
            tree_sitter_json::LANGUAGE.into(),
            "json",
            JSON_HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .expect("the bundled JSON highlight query must match its grammar");
        json.configure(HIGHLIGHT_NAMES);

        let mut html = HighlightConfiguration::new(
            tree_sitter_html::LANGUAGE.into(),
            "html",
            tree_sitter_html::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .expect("the bundled HTML highlight query must match its grammar");
        html.configure(HIGHLIGHT_NAMES);

        Self {
            highlighter: TreeSitterHighlighter::new(),
            json,
            html,
        }
    }

    /// Highlights each source line independently. Ranges are byte offsets into
    /// the corresponding line, rather than offsets into the complete document.
    pub fn highlight(&mut self, text: &str, language: Option<Language>) -> Vec<Vec<Highlight>> {
        let line_starts = line_starts(text);
        let mut lines = vec![Vec::new(); line_starts.len()];
        let Some(language) = language else {
            return lines;
        };

        // No CSS grammar is shipped. Deliberately leave CSS plain rather than
        // claiming that HTML captures describe CSS syntax.
        let config = match language {
            Language::Json => &self.json,
            Language::Html => &self.html,
            Language::Css => return lines,
        };

        let Ok(events) = self
            .highlighter
            .highlight(config, text.as_bytes(), None, |_| None)
        else {
            return lines;
        };

        let mut active = Vec::new();
        for event in events {
            match event {
                Ok(HighlightEvent::HighlightStart(highlight)) => active.push(highlight.0),
                Ok(HighlightEvent::HighlightEnd) => {
                    active.pop();
                }
                Ok(HighlightEvent::Source { start, end }) => {
                    let Some(index) = active.last().copied() else {
                        continue;
                    };
                    let Some(capture) = HIGHLIGHT_NAMES.get(index) else {
                        continue;
                    };
                    append_range(
                        &mut lines,
                        &line_starts,
                        text,
                        start..end,
                        theme::syntax::for_capture(capture),
                    );
                }
                Err(_) => {
                    // Tree-sitter can still produce useful captures around a
                    // malformed region; retain those instead of failing the
                    // entire document.
                }
            }
        }
        lines
    }
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        text.bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );
    starts
}

fn append_range(
    lines: &mut [Vec<Highlight>],
    starts: &[usize],
    text: &str,
    range: Range<usize>,
    style: ratatui::style::Style,
) {
    if range.start >= range.end || range.start >= text.len() {
        return;
    }

    let mut start = range.start;
    let end = range.end.min(text.len());
    while start < end {
        let line = starts.partition_point(|offset| *offset <= start) - 1;
        let next_line = starts.get(line + 1).copied().unwrap_or(text.len());
        let content_end =
            if next_line > starts[line] && text.as_bytes().get(next_line - 1) == Some(&b'\n') {
                next_line - 1
            } else {
                next_line
            };
        let piece_end = end.min(content_end);
        if start < piece_end {
            lines[line].push(Highlight {
                range: start - starts[line]..piece_end - starts[line],
                style,
            });
        }
        start = if piece_end == start {
            next_line.max(start + 1)
        } else {
            piece_end
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_names_and_extensions_are_strict() {
        assert_eq!(Language::from_name("json"), Some(Language::Json));
        assert_eq!(Language::from_name("JSON"), None);
        assert_eq!(Language::Html.name(), "html");
        assert_eq!(Language::Css.extension(), "css");
    }

    #[test]
    fn no_language_preserves_source_line_count() {
        let mut highlighter = Highlighter::new();
        let lines = highlighter.highlight("one\ntwo\n", None);
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(Vec::is_empty));
    }

    #[test]
    fn json_captures_are_line_relative_and_use_theme_styles() {
        let mut highlighter = Highlighter::new();
        let lines = highlighter.highlight(
            "{\n  \"ok\": true,\n  \"count\": 2,\n  \"none\": null\n}",
            Some(Language::Json),
        );
        assert!(lines[1].iter().any(|item| {
            item.range == (2..6) && item.style == theme::syntax::for_capture("property")
        }));
        assert!(
            lines[1]
                .iter()
                .any(|item| item.style == theme::syntax::for_capture("boolean"))
        );
        assert!(
            lines[2]
                .iter()
                .any(|item| item.style == theme::syntax::for_capture("number"))
        );
        assert!(
            lines[3]
                .iter()
                .any(|item| item.style == theme::syntax::for_capture("constant"))
        );
    }

    #[test]
    fn html_tags_attributes_and_punctuation_are_highlighted() {
        let mut highlighter = Highlighter::new();
        let lines = highlighter.highlight("<p class=\"lead\">Hi</p>", Some(Language::Html));
        for capture in ["tag", "attribute", "punctuation.bracket"] {
            assert!(
                lines[0]
                    .iter()
                    .any(|item| item.style == theme::syntax::for_capture(capture)),
                "missing {capture} capture"
            );
        }
    }

    #[test]
    fn malformed_documents_keep_the_highlights_that_can_be_parsed() {
        let mut highlighter = Highlighter::new();
        let lines = highlighter.highlight("{\"ok\": true,\n\"bad\":", Some(Language::Json));
        assert_eq!(lines.len(), 2);
        assert!(!lines[0].is_empty());
    }

    #[test]
    fn css_is_deliberately_unhighlighted_without_a_css_parser() {
        let mut highlighter = Highlighter::new();
        let lines = highlighter.highlight("body { color: red; }", Some(Language::Css));
        assert_eq!(lines, vec![Vec::new()]);
    }
}
