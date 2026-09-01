//! Durable, collection-scoped state for the collection browser.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result};

type StoredState = BTreeMap<String, Vec<String>>;

pub fn load(path: &Path, collection_root: &Path) -> Result<HashSet<PathBuf>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("could not read collection browser state {}", path.display()))?;
    let state: StoredState = serde_json::from_str(&contents).with_context(|| {
        format!(
            "could not parse collection browser state {}",
            path.display()
        )
    })?;
    let Some(key) = collection_key(collection_root) else {
        return Ok(HashSet::new());
    };
    let Some(collapsed) = state.get(&key) else {
        return Ok(HashSet::new());
    };
    Ok(collapsed
        .iter()
        .filter_map(|relative| safe_relative_path(relative))
        .map(|relative| collection_root.join(relative))
        .collect())
}

pub fn save<'a>(
    path: &Path,
    collection_root: &Path,
    collapsed: impl IntoIterator<Item = &'a Path>,
) -> Result<()> {
    let Some(key) = collection_key(collection_root) else {
        return Ok(());
    };
    let mut state = match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoredState::new(),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("could not read collection browser state {}", path.display())
            });
        }
    };
    let collapsed = collapsed
        .into_iter()
        .filter_map(|directory| directory.strip_prefix(collection_root).ok())
        .filter_map(|relative| relative.to_str())
        .filter(|relative| safe_relative_path(relative).is_some())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if collapsed.is_empty() {
        state.remove(&key);
    } else {
        state.insert(key, collapsed);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "could not create collection browser state directory {}",
                parent.display()
            )
        })?;
    }
    let contents = serde_json::to_string_pretty(&state)
        .context("could not serialize collection browser state")?;
    fs::write(path, contents).with_context(|| {
        format!(
            "could not write collection browser state {}",
            path.display()
        )
    })
}

fn collection_key(root: &Path) -> Option<String> {
    fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf())
        .to_str()
        .map(str::to_owned)
}

fn safe_relative_path(path: &str) -> Option<&Path> {
    let path = Path::new(path);
    (!path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_))))
    .then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_directories_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let state_file = directory.path().join("state.json");
        let root = directory.path().join("collection");
        let users = root.join("users");
        let teams = root.join("admin/teams");
        fs::create_dir_all(&users).unwrap();
        fs::create_dir_all(&teams).unwrap();

        save(&state_file, &root, [users.as_path(), teams.as_path()]).unwrap();

        assert_eq!(
            load(&state_file, &root).unwrap(),
            HashSet::from([users, teams])
        );
    }

    #[test]
    fn collections_with_the_same_relative_directories_are_isolated() {
        let directory = tempfile::tempdir().unwrap();
        let state_file = directory.path().join("state.json");
        let first_root = directory.path().join("first");
        let second_root = directory.path().join("second");
        let first_users = first_root.join("users");
        let second_users = second_root.join("users");
        fs::create_dir_all(&first_users).unwrap();
        fs::create_dir_all(&second_users).unwrap();

        save(&state_file, &first_root, [first_users.as_path()]).unwrap();
        save(&state_file, &second_root, [second_users.as_path()]).unwrap();

        assert_eq!(
            load(&state_file, &first_root).unwrap(),
            HashSet::from([first_users])
        );
        assert_eq!(
            load(&state_file, &second_root).unwrap(),
            HashSet::from([second_users])
        );
    }
}
