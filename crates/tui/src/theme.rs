//! The single hardcoded colour scheme.
//!
//! There is deliberately no theme system. Two consequences follow, and both are
//! load-bearing:
//!
//! * **Nothing paints a background.** Every surface uses [`Color::Reset`] so the
//!   terminal's own background — including transparency and any image behind it
//!   — shows through. The only painted backgrounds are the block cursor and a
//!   couple of selection highlights, listed below.
//! * **Colours that were alpha-blended against the old dark background are
//!   pre-blended here**, because there is no background to blend against at
//!   runtime. The blend base is `#0F0F1F`; each constant records the source
//!   colour and alpha it came from.

use ratatui::style::{Color, Modifier, Style};

/// Selection, focus rings, active tab.
pub const ACCENT: Color = Color::Rgb(0xFF, 0x69, 0xB4);
/// `ACCENT` at 40% — unfocused pane borders.
pub const ACCENT_DIM: Color = Color::Rgb(0x6F, 0x33, 0x5B);
/// `ACCENT` at 50% — unfocused pane titles.
pub const ACCENT_HALF: Color = Color::Rgb(0x87, 0x3C, 0x6A);
/// `ACCENT` at 25% — the one painted highlight, used for the open tree node
/// and the matched portion of an autocompletion candidate.
pub const ACCENT_MUTED: Color = Color::Rgb(0x4B, 0x26, 0x44);

/// Cursor, JSON keys, the block cursor background.
pub const PRIMARY: Color = Color::Rgb(0xC4, 0x5A, 0xFF);
/// Numbers, URL base.
pub const SECONDARY: Color = Color::Rgb(0xA6, 0x84, 0xE8);
pub const WARNING: Color = Color::Rgb(0xFF, 0xD7, 0x00);
pub const ERROR: Color = Color::Rgb(0xFF, 0x45, 0x00);
pub const SUCCESS: Color = Color::Rgb(0x00, 0xFA, 0x9A);

/// The terminal's own foreground.
pub const FOREGROUND: Color = Color::Reset;
/// Secondary text. Rendered with a real colour rather than `DIM` where the
/// text must stay legible next to dimmed rows.
pub const MUTED: Color = Color::Rgb(0x82, 0x82, 0x89);

/// Foreground for text sitting on `PRIMARY`, e.g. under the block cursor.
pub const ON_PRIMARY: Color = Color::Rgb(0x00, 0x00, 0x00);

/// Per-method colours in the collection tree.
pub const METHOD_GET: Color = Color::Rgb(0x0E, 0xA5, 0xE9);
pub const METHOD_POST: Color = Color::Rgb(0x22, 0xC5, 0x5E);
pub const METHOD_PUT: Color = Color::Rgb(0xF5, 0x9E, 0x0B);
pub const METHOD_DELETE: Color = Color::Rgb(0xEF, 0x44, 0x44);
pub const METHOD_PATCH: Color = Color::Rgb(0x14, 0xB8, 0xA6);
pub const METHOD_OPTIONS: Color = Color::Rgb(0x8B, 0x5C, 0xF6);
pub const METHOD_HEAD: Color = Color::Rgb(0xD9, 0x46, 0xEF);

pub fn method_color(method: rusting_core::HttpMethod) -> Color {
    use rusting_core::HttpMethod as M;
    match method {
        M::Get => METHOD_GET,
        M::Post => METHOD_POST,
        M::Put => METHOD_PUT,
        M::Delete => METHOD_DELETE,
        M::Patch => METHOD_PATCH,
        M::Options => METHOD_OPTIONS,
        M::Head => METHOD_HEAD,
    }
}

/// Panel borders. `focused` gets the full accent and a bold title.
pub fn border(focused: bool) -> Style {
    Style::new().fg(if focused { ACCENT } else { ACCENT_DIM })
}

/// Border titles, right-aligned on the request and response panes.
pub fn border_title(focused: bool) -> Style {
    if focused {
        Style::new().fg(FOREGROUND).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(ACCENT_HALF)
    }
}

/// The block cursor inside a focused text input or editor.
pub fn cursor() -> Style {
    Style::new().bg(PRIMARY).fg(ON_PRIMARY)
}

/// The selected row of a focused list, table or tree.
pub fn selection() -> Style {
    Style::new().bg(ACCENT_MUTED).add_modifier(Modifier::BOLD)
}

/// A row whose `enabled` toggle is off.
pub fn disabled() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// Placeholder and empty-state text.
pub fn placeholder() -> Style {
    Style::new().fg(MUTED).add_modifier(Modifier::DIM)
}

/// Variable tokens in an input. A resolved variable is green, an unresolved one
/// red, so a typo is visible before the request is sent.
pub fn variable(resolved: bool) -> Style {
    Style::new().fg(if resolved { SUCCESS } else { ERROR })
}

/// `:name` path placeholders in the URL.
pub fn path_param(has_value: bool) -> Style {
    Style::new().fg(if has_value { SUCCESS } else { WARNING })
}

/// URL segment styling in the URL bar.
pub mod url {
    use super::*;

    pub fn protocol() -> Style {
        Style::new().fg(ACCENT)
    }
    pub fn base() -> Style {
        Style::new().fg(SECONDARY)
    }
    /// Every `/` in the URL.
    pub fn separator() -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }
}

/// Syntax highlighting. Only the buckets that JSON, HTML and CSS produce.
pub mod syntax {
    use super::*;

    pub fn key() -> Style {
        Style::new().fg(PRIMARY)
    }
    pub fn string() -> Style {
        Style::new().fg(ACCENT)
    }
    pub fn number() -> Style {
        Style::new().fg(SECONDARY)
    }
    pub fn boolean() -> Style {
        Style::new().fg(SUCCESS)
    }
    pub fn null() -> Style {
        Style::new().fg(WARNING)
    }
    pub fn punctuation() -> Style {
        Style::new().fg(MUTED)
    }
    pub fn tag() -> Style {
        Style::new().fg(PRIMARY)
    }
    pub fn attribute() -> Style {
        Style::new().fg(SECONDARY)
    }
    pub fn comment() -> Style {
        Style::new().fg(MUTED).add_modifier(Modifier::ITALIC)
    }

    /// Maps a tree-sitter highlight capture name to a style.
    ///
    /// The capture list is the one the highlight configurations are built with;
    /// see `HIGHLIGHT_NAMES` in the syntax module.
    pub fn for_capture(capture: &str) -> Style {
        match capture {
            "string" | "string.special" => string(),
            "number" => number(),
            "constant.builtin" | "boolean" => boolean(),
            "constant" => null(),
            "property" | "tag" => tag(),
            "attribute" => attribute(),
            "comment" => comment(),
            "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => punctuation(),
            _ => Style::new(),
        }
    }
}

/// Status code colouring for the response pane and URL bar pill.
pub fn status_color(status: u16) -> Color {
    match status {
        ..300 => SUCCESS,
        300..400 => WARNING,
        _ => ERROR,
    }
}

/// Timing phase markers next to the URL bar.
pub fn timing_marker(state: MarkerState) -> Style {
    match state {
        MarkerState::NotStarted => Style::new().fg(MUTED).add_modifier(Modifier::DIM),
        MarkerState::Started => Style::new().fg(WARNING),
        MarkerState::Complete => Style::new().fg(SUCCESS),
        MarkerState::Failed => Style::new().fg(ERROR),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerState {
    NotStarted,
    Started,
    Complete,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_here_paints_a_background_except_the_documented_highlights() {
        for style in [
            border(true),
            border(false),
            border_title(true),
            disabled(),
            placeholder(),
            variable(true),
            path_param(false),
            url::protocol(),
            syntax::string(),
            timing_marker(MarkerState::Complete),
        ] {
            assert_eq!(style.bg, None, "{style:?} paints a background");
        }
        assert_eq!(cursor().bg, Some(PRIMARY));
        assert_eq!(selection().bg, Some(ACCENT_MUTED));
    }

    #[test]
    fn status_colour_boundaries() {
        assert_eq!(status_color(200), SUCCESS);
        assert_eq!(status_color(299), SUCCESS);
        assert_eq!(status_color(301), WARNING);
        assert_eq!(status_color(399), WARNING);
        assert_eq!(status_color(404), ERROR);
        assert_eq!(status_color(500), ERROR);
    }

    #[test]
    fn every_method_has_a_distinct_colour() {
        let mut colors: Vec<Color> = rusting_core::HttpMethod::ALL
            .into_iter()
            .map(method_color)
            .collect();
        let count = colors.len();
        colors.sort_by_key(|c| format!("{c:?}"));
        colors.dedup();
        assert_eq!(colors.len(), count);
    }

    #[test]
    fn unknown_captures_fall_back_to_no_styling() {
        assert_eq!(syntax::for_capture("something.unknown"), Style::new());
    }
}
