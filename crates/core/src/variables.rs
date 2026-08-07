//! `$VAR` / `${VAR}` substitution and the cursor helpers the editors need.
//!
//! Two related but distinct grammars live here:
//!
//! * [`substitute`] resolves variables and **fails** on an undefined name or a
//!   malformed `$`. Requests must not be sent with a half-resolved URL.
//! * [`find_variables`] locates variable tokens for highlighting and
//!   autocompletion, and never fails.
//!
//! `$$` is the escape for a literal `$` in both.

use std::collections::BTreeMap;

/// The variable store: environment files, optionally the host environment, and
/// session variables set by scripts, already merged.
pub type Variables = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubstitutionError {
    #[error("Variable not defined: ${0}")]
    Undefined(String),
    #[error("Invalid variable reference at position {0}")]
    Malformed(usize),
}

/// A located variable token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableToken {
    pub name: String,
    /// Byte offset of the `$`.
    pub start: usize,
    /// Byte offset one past the end of the token.
    pub end: usize,
    pub braced: bool,
}

/// Resolves every `$NAME` / `${NAME}` in `text`.
pub fn substitute(text: &str, variables: &Variables) -> Result<String, SubstitutionError> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] != b'$' {
            let next = next_char_boundary(text, index);
            out.push_str(&text[index..next]);
            index = next;
            continue;
        }
        match parse_reference(text, index) {
            Reference::Escape => {
                out.push('$');
                index += 2;
            }
            Reference::Named { name, end, .. } => {
                let value = variables
                    .get(name)
                    .ok_or_else(|| SubstitutionError::Undefined(name.to_owned()))?;
                out.push_str(value);
                index = end;
            }
            Reference::Malformed => return Err(SubstitutionError::Malformed(index)),
        }
    }
    Ok(out)
}

/// True when `text` contains no variable reference at all, so substitution can
/// be skipped.
pub fn is_literal(text: &str) -> bool {
    !text.contains('$')
}

/// Locates every variable token, skipping `$$` escapes. Never fails: a trailing
/// bare `$` or a malformed `${` simply yields no token.
pub fn find_variables(text: &str) -> Vec<VariableToken> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index = next_char_boundary(text, index);
            continue;
        }
        match parse_reference(text, index) {
            Reference::Escape => index += 2,
            Reference::Named {
                name,
                end,
                braced,
                start,
            } => {
                found.push(VariableToken {
                    name: name.to_owned(),
                    start,
                    end,
                    braced,
                });
                index = end;
            }
            Reference::Malformed => index += 1,
        }
    }
    found
}

/// The variable token the caret sits inside, if any.
///
/// The caret must be strictly past the `$` and before the end of the token.
/// The one exception is an unbraced variable that runs to the end of the text:
/// there the caret sits at `end` while the name is still being typed, and the
/// autocompletion has to keep offering candidates.
pub fn variable_at_cursor(text: &str, cursor: usize) -> Option<VariableToken> {
    find_variables(text).into_iter().find(|token| {
        cursor > token.start
            && (cursor < token.end
                || (!token.braced && cursor == token.end && token.end == text.len()))
    })
}

enum Reference<'a> {
    Escape,
    Named {
        name: &'a str,
        start: usize,
        end: usize,
        braced: bool,
    },
    Malformed,
}

/// Parses the reference beginning at `start`, which must point at a `$`.
fn parse_reference(text: &str, start: usize) -> Reference<'_> {
    let bytes = text.as_bytes();
    debug_assert_eq!(bytes[start], b'$');
    let after = start + 1;
    match bytes.get(after) {
        Some(b'$') => Reference::Escape,
        Some(b'{') => {
            let name_start = after + 1;
            let mut cursor = name_start;
            while cursor < bytes.len() && is_name_byte(bytes[cursor], cursor == name_start) {
                cursor += 1;
            }
            if cursor == name_start || bytes.get(cursor) != Some(&b'}') {
                return Reference::Malformed;
            }
            Reference::Named {
                name: &text[name_start..cursor],
                start,
                end: cursor + 1,
                braced: true,
            }
        }
        Some(&byte) if is_name_byte(byte, true) => {
            let mut cursor = after;
            while cursor < bytes.len() && is_name_byte(bytes[cursor], false) {
                cursor += 1;
            }
            Reference::Named {
                name: &text[after..cursor],
                start,
                end: cursor,
                braced: false,
            }
        }
        _ => Reference::Malformed,
    }
}

/// Variable names are ASCII identifiers. Deliberately not Unicode-aware: a
/// name has to be typable in a `.env` file and in a shell.
fn is_name_byte(byte: u8, first: bool) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic() || (!first && byte.is_ascii_digit())
}

fn next_char_boundary(text: &str, index: usize) -> usize {
    let mut next = index + 1;
    while next < text.len() && !text.is_char_boundary(next) {
        next += 1;
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Variables {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn substitutes_both_forms() {
        let v = vars(&[("NAME", "world"), ("N", "1")]);
        assert_eq!(substitute("hi $NAME", &v).unwrap(), "hi world");
        assert_eq!(substitute("hi ${NAME}!", &v).unwrap(), "hi world!");
        assert_eq!(substitute("/posts/${N}/x", &v).unwrap(), "/posts/1/x");
    }

    #[test]
    fn double_dollar_escapes() {
        let v = vars(&[]);
        assert_eq!(substitute("$$NAME", &v).unwrap(), "$NAME");
        assert_eq!(substitute("$${NAME}", &v).unwrap(), "${NAME}");
        assert_eq!(substitute("100$$", &v).unwrap(), "100$");
    }

    #[test]
    fn undefined_variable_is_an_error() {
        assert_eq!(
            substitute("$MISSING", &vars(&[])),
            Err(SubstitutionError::Undefined("MISSING".into()))
        );
    }

    #[test]
    fn malformed_dollar_is_an_error() {
        assert_eq!(
            substitute("cost: 5$", &vars(&[])),
            Err(SubstitutionError::Malformed(7))
        );
        assert_eq!(
            substitute("${}", &vars(&[])),
            Err(SubstitutionError::Malformed(0))
        );
        assert_eq!(
            substitute("${UNCLOSED", &vars(&[])),
            Err(SubstitutionError::Malformed(0))
        );
    }

    #[test]
    fn find_variables_skips_escapes_and_junk() {
        let found = find_variables("a $FOO b ${BAR} c $$NOPE d $");
        let names: Vec<&str> = found.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["FOO", "BAR"]);
        assert_eq!((found[0].start, found[0].end), (2, 6));
        assert_eq!((found[1].start, found[1].end), (9, 15));
        assert!(found[1].braced);
    }

    #[test]
    fn cursor_hit_test_excludes_the_dollar_and_the_closing_brace() {
        let text = "Hello, $name!";
        assert!(variable_at_cursor(text, 7).is_none(), "on the $");
        for cursor in 8..=11 {
            assert!(variable_at_cursor(text, cursor).is_some(), "at {cursor}");
        }
        assert!(variable_at_cursor(text, 12).is_none(), "past the name");
    }

    #[test]
    fn cursor_at_end_of_trailing_unbraced_variable_hits() {
        let text = "url/$ID";
        assert_eq!(variable_at_cursor(text, 7).unwrap().name, "ID");
    }

    #[test]
    fn cursor_inside_braces_hits_but_after_them_does_not() {
        let text = "${ID}";
        assert!(variable_at_cursor(text, 3).is_some());
        assert!(variable_at_cursor(text, 5).is_none());
    }

    #[test]
    fn names_are_ascii_identifiers() {
        assert!(find_variables("$1BAD").is_empty());
        assert_eq!(find_variables("$A_1B")[0].name, "A_1B");
        // A non-ASCII byte simply ends the name.
        assert_eq!(find_variables("$café")[0].name, "caf");
    }

    #[test]
    fn multibyte_text_does_not_split_characters() {
        let v = vars(&[("X", "値")]);
        assert_eq!(substitute("日本語 $X です", &v).unwrap(), "日本語 値 です");
    }
}
