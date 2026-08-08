//! Small-list fuzzy ranking used by completion popups and palettes.

use nucleo::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo::{Config, Matcher, Utf32Str};

/// One candidate accepted by the matcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Index in the caller's candidate slice.
    pub index: usize,
    /// Nucleo's match score. Higher is better.
    pub score: u32,
    /// Matched positions as character indices, not UTF-8 byte offsets.
    pub positions: Vec<u32>,
}

/// Fuzzy-ranks `candidates` using nucleo.
///
/// Results are ordered by descending score. Equal scores retain the input
/// order, which makes a popup stable while the user types. An empty needle is
/// deliberately handled without invoking the matcher so every candidate is
/// returned with score zero.
pub fn rank(needle: &str, candidates: &[&str]) -> Vec<Match> {
    if needle.is_empty() {
        return candidates
            .iter()
            .enumerate()
            .map(|(index, _)| Match {
                index,
                score: 0,
                positions: Vec::new(),
            })
            .collect();
    }

    let atom = Atom::new(
        needle,
        CaseMatching::Ignore,
        Normalization::Smart,
        AtomKind::Fuzzy,
        false,
    );
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut char_buffer = Vec::new();
    let mut matches = Vec::new();

    for (index, candidate) in candidates.iter().enumerate() {
        // `Utf32Str::new` intentionally collapses extended grapheme clusters.
        // The public contract here is Rust `char` indices, so retain every
        // codepoint in the haystack passed to nucleo.
        let haystack = if candidate.is_ascii() {
            Utf32Str::Ascii(candidate.as_bytes())
        } else {
            char_buffer.clear();
            char_buffer.extend(candidate.chars());
            Utf32Str::Unicode(&char_buffer)
        };
        let mut positions = Vec::new();
        if let Some(score) = atom.indices(haystack, &mut matcher, &mut positions) {
            matches.push(Match {
                index,
                score: u32::from(score),
                positions,
            });
        }
    }

    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.index.cmp(&right.index))
    });
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_needle_returns_every_candidate_in_input_order() {
        assert_eq!(
            rank("", &["second", "first"]),
            vec![
                Match {
                    index: 0,
                    score: 0,
                    positions: Vec::new(),
                },
                Match {
                    index: 1,
                    score: 0,
                    positions: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn unmatched_candidates_are_removed_and_scores_descend() {
        let matches = rank("abc", &["a---b---c", "abc", "no match"]);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].index, 1);
        assert_eq!(matches[0].positions, vec![0, 1, 2]);
        assert!(matches[0].score >= matches[1].score);
    }

    #[test]
    fn equal_scores_keep_the_original_candidate_order() {
        let matches = rank("same", &["same", "same", "same"]);
        assert_eq!(
            matches.iter().map(|item| item.index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn positions_are_unicode_character_indices() {
        let matches = rank("éb", &["aéb"]);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].positions, vec![1, 2]);
    }

    #[test]
    fn combining_codepoints_do_not_shift_later_character_indices() {
        let matches = rank("b", &["a\u{301}b"]);
        assert_eq!(matches[0].positions, vec![2]);
    }

    #[test]
    fn matching_is_case_insensitive() {
        let matches = rank("gb", &["GitBranch"]);
        assert_eq!(matches[0].positions, vec![0, 3]);
    }
}
