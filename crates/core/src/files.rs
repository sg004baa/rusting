//! Filename validation and derivation for request files.

use std::path::Path;

use crate::model::REQUEST_SUFFIX;

/// Names Windows refuses regardless of extension.
const RESERVED_STEMS: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Why a filename was rejected. The message is shown verbatim in the modal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FileNameError {
    #[error("Name cannot be empty.")]
    Empty,
    #[error("Name cannot contain a path separator.")]
    Separator,
    #[error("Name is too long.")]
    TooLong,
    #[error("'{0}' is a reserved name.")]
    Reserved(String),
    #[error("Name cannot contain consecutive dots.")]
    ConsecutiveDots,
    #[error("Name cannot start with a dot.")]
    LeadingDot,
    #[error("Name cannot contain '{0}'.")]
    InvalidCharacter(char),
}

/// Validates a single path component destined to become a request filename.
pub fn validate_file_name(name: &str) -> Result<(), FileNameError> {
    if name.trim().is_empty() {
        return Err(FileNameError::Empty);
    }
    if name.contains('/') || name.contains('\\') {
        return Err(FileNameError::Separator);
    }
    if name.len() > 255 {
        return Err(FileNameError::TooLong);
    }
    if name.starts_with('.') {
        return Err(FileNameError::LeadingDot);
    }
    if name.contains("..") {
        return Err(FileNameError::ConsecutiveDots);
    }
    if let Some(c) = name
        .chars()
        .find(|c| matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
    {
        return Err(FileNameError::InvalidCharacter(c));
    }
    let stem = name.split('.').next().unwrap_or(name);
    if RESERVED_STEMS
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
    {
        return Err(FileNameError::Reserved(stem.to_owned()));
    }
    Ok(())
}

/// Validates the "path in collection" field of the new-request modal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DirectoryError {
    #[error("Path cannot escape the collection root.")]
    Escapes,
    #[error("Path must be relative to the collection root.")]
    Absolute,
}

pub fn validate_directory(path: &str) -> Result<(), DirectoryError> {
    let trimmed = path.trim();
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(DirectoryError::Absolute);
    }
    if Path::new(trimmed)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(DirectoryError::Escapes);
    }
    Ok(())
}

/// Normalizes the "path in collection" field: an empty path means the collection root.
pub fn normalize_directory(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        ".".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Derives a filename stem from a request title: lowercased, non-alphanumeric
/// runs collapsed to a single `-`.
pub fn generate_file_stem(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut pending_dash = false;
    for c in title.chars() {
        if c.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(c.to_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The full filename for a title, including the request suffix.
pub fn generate_file_name(title: &str) -> String {
    format!("{}{REQUEST_SUFFIX}", generate_file_stem(title))
}

/// Appends `-2`, `-3`, … to the stem until nothing exists at that path.
pub fn unique_file_name(directory: &Path, file_name: &str) -> String {
    if !directory.join(file_name).exists() {
        return file_name.to_owned();
    }
    let stem = file_name
        .strip_suffix(REQUEST_SUFFIX)
        .unwrap_or(file_name)
        .to_owned();
    for counter in 2u32.. {
        let candidate = format!("{stem}-{counter}{REQUEST_SUFFIX}");
        if !directory.join(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!("the counter range is unbounded")
}

/// Strips the request suffix for display in the collection tree.
pub fn display_name(file_name: &str) -> &str {
    file_name.strip_suffix(REQUEST_SUFFIX).unwrap_or(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_the_obvious() {
        assert_eq!(validate_file_name(""), Err(FileNameError::Empty));
        assert_eq!(validate_file_name("   "), Err(FileNameError::Empty));
        assert_eq!(validate_file_name("a/b"), Err(FileNameError::Separator));
        assert_eq!(
            validate_file_name(&"a".repeat(256)),
            Err(FileNameError::TooLong)
        );
        assert_eq!(
            validate_file_name(".hidden.yaml"),
            Err(FileNameError::LeadingDot)
        );
        assert_eq!(
            validate_file_name("a..b.yaml"),
            Err(FileNameError::ConsecutiveDots)
        );
        assert_eq!(
            validate_file_name("C:x"),
            Err(FileNameError::InvalidCharacter(':'))
        );
    }

    #[test]
    fn rejects_windows_reserved_stems_but_not_lookalikes() {
        for name in [
            "CON", "PRN.txt", "aux.log", "NUL.dat", "COM1.bin", "lpt1.tmp",
        ] {
            assert!(validate_file_name(name).is_err(), "{name}");
        }
        for name in ["COM0.bin", "LPT0.tmp", "CONSOLE", "conf.yaml"] {
            assert!(validate_file_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn accepts_a_normal_request_file_name() {
        assert!(validate_file_name("get-one.posting.yaml").is_ok());
    }

    #[test]
    fn directory_validation() {
        assert_eq!(validate_directory("/abs"), Err(DirectoryError::Absolute));
        assert_eq!(validate_directory("a/../b"), Err(DirectoryError::Escapes));
        assert!(validate_directory("").is_ok());
        assert!(validate_directory(".").is_ok());
        assert!(validate_directory("a/b").is_ok());
    }

    #[test]
    fn empty_directory_normalizes_to_collection_root() {
        assert_eq!(normalize_directory(""), ".");
        assert_eq!(normalize_directory("  "), ".");
        assert_eq!(normalize_directory(" api "), "api");
    }

    #[test]
    fn stem_generation_collapses_punctuation() {
        assert_eq!(generate_file_stem("Get One Post"), "get-one-post");
        assert_eq!(generate_file_stem("  Hello,  World!! "), "hello-world");
        assert_eq!(generate_file_stem("v2 / users"), "v2-users");
        assert_eq!(generate_file_stem("---"), "");
        assert_eq!(
            generate_file_name("Create User"),
            "create-user.posting.yaml"
        );
    }

    #[test]
    fn unique_file_name_counts_up() {
        let dir = std::env::temp_dir().join(format!("rusting-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = "dupe.posting.yaml";
        assert_eq!(unique_file_name(&dir, first), first);
        std::fs::write(dir.join(first), "").unwrap();
        assert_eq!(unique_file_name(&dir, first), "dupe-2.posting.yaml");
        std::fs::write(dir.join("dupe-2.posting.yaml"), "").unwrap();
        assert_eq!(unique_file_name(&dir, first), "dupe-3.posting.yaml");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn display_name_strips_the_suffix() {
        assert_eq!(display_name("get-one.posting.yaml"), "get-one");
        assert_eq!(display_name("other.yaml"), "other.yaml");
    }
}
