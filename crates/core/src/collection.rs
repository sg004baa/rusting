//! The collection tree: a directory of `*.posting.yaml` files, loaded into a
//! nested [`Collection`] and written back one file at a time.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::files;
use crate::model::{REQUEST_SUFFIX, RequestModel};
use crate::yaml;

/// One directory of the collection tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Collection {
    /// Absolute path of this directory.
    pub path: PathBuf,
    /// Directory basename, shown in the tree.
    pub name: String,
    pub requests: Vec<RequestModel>,
    pub children: Vec<Collection>,
}

/// A request that could not be parsed. Loading never fails wholesale: one bad
/// file must not hide the rest of the collection.
#[derive(Debug, Clone)]
pub struct LoadFailure {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug)]
pub struct LoadedCollection {
    pub collection: Collection,
    pub failures: Vec<LoadFailure>,
}

impl Collection {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            path,
            name,
            requests: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Recursively loads every request file under `root`.
    pub fn from_directory(root: impl AsRef<Path>) -> Result<LoadedCollection> {
        let root = root.as_ref();
        let mut failures = Vec::new();
        let collection = load_directory(root, &mut failures)
            .with_context(|| format!("could not read collection {}", root.display()))?;
        Ok(LoadedCollection {
            collection,
            failures,
        })
    }

    /// Depth-first walk over every request in the tree.
    pub fn requests_recursive(&self) -> Vec<&RequestModel> {
        let mut out = Vec::new();
        self.collect_requests(&mut out);
        out
    }

    fn collect_requests<'a>(&'a self, out: &mut Vec<&'a RequestModel>) {
        out.extend(self.requests.iter());
        for child in &self.children {
            child.collect_requests(out);
        }
    }

    /// The index at which `request` should be inserted to keep `requests`
    /// sorted by `(method rank, name)`.
    pub fn insertion_index(&self, request: &RequestModel) -> usize {
        self.requests
            .partition_point(|existing| existing.sort_key() <= request.sort_key())
    }

    /// Finds the node owning `directory`, which must be inside this collection.
    pub fn find_mut(&mut self, directory: &Path) -> Option<&mut Collection> {
        if self.path == directory {
            return Some(self);
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_mut(directory))
    }

    /// Creates any missing intermediate nodes down to `directory` and returns
    /// the leaf. Also creates the directories on disk.
    pub fn ensure_path(&mut self, directory: &Path) -> Result<&mut Collection> {
        let relative = directory.strip_prefix(&self.path).with_context(|| {
            format!(
                "{} is outside the collection {}",
                directory.display(),
                self.path.display()
            )
        })?;
        std::fs::create_dir_all(directory)
            .with_context(|| format!("could not create {}", directory.display()))?;

        let mut node = self;
        for component in relative.components() {
            let segment = component.as_os_str().to_string_lossy().into_owned();
            if segment == "." {
                continue;
            }
            let child_path = node.path.join(&segment);
            let index = match node
                .children
                .iter()
                .position(|child| child.path == child_path)
            {
                Some(index) => index,
                None => {
                    let insert_at = node.children.partition_point(|child| child.name <= segment);
                    node.children
                        .insert(insert_at, Collection::new(&child_path));
                    insert_at
                }
            };
            node = &mut node.children[index];
        }
        Ok(node)
    }
}

fn load_directory(directory: &Path, failures: &mut Vec<LoadFailure>) -> Result<Collection> {
    let mut collection = Collection::new(directory);

    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        // A collection directory that does not exist yet is an empty
        // collection, not an error: the CLI creates it lazily.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(collection),
        Err(error) => return Err(error.into()),
    };

    let mut child_directories = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            child_directories.push(path);
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.ends_with(REQUEST_SUFFIX) {
            continue;
        }
        match load_request(&path) {
            Ok(request) => collection.requests.push(request),
            Err(error) => failures.push(LoadFailure {
                path: path.clone(),
                message: format!("{error:#}"),
            }),
        }
    }

    child_directories.sort();
    for child_directory in child_directories {
        collection
            .children
            .push(load_directory(&child_directory, failures)?);
    }

    collection
        .requests
        .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    Ok(collection)
}

/// Reads one request file. A file with no `name` takes its name from the
/// filename, so a hand-created file shows up sensibly in the tree.
pub fn load_request(path: &Path) -> Result<RequestModel> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let mut request: RequestModel =
        yaml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))?;
    if request.name.is_empty() {
        request.name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(files::display_name)
            .unwrap_or_default()
            .to_owned();
    }
    request.path = Some(path.to_path_buf());
    Ok(request)
}

/// Writes a request to `request.path`, creating parent directories.
pub fn save_request(request: &RequestModel) -> Result<()> {
    let path = request
        .path
        .as_ref()
        .context("request has no path on disk")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let text = yaml::to_string(request).context("could not serialize request")?;
    std::fs::write(path, text).with_context(|| format!("could not write {}", path.display()))
}

pub fn delete_request(request: &RequestModel) -> Result<()> {
    let path = request
        .path
        .as_ref()
        .context("request has no path on disk")?;
    std::fs::remove_file(path).with_context(|| format!("could not delete {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HttpMethod;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("rusting-collection-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, relative: &str, contents: &str) {
            let path = self.0.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn loads_nested_directories_and_sorts_by_method_then_name() {
        let dir = TempDir::new("nested");
        dir.write("b.posting.yaml", "name: b\nmethod: POST\nurl: http://x\n");
        dir.write("a.posting.yaml", "name: a\nmethod: POST\nurl: http://x\n");
        dir.write("z.posting.yaml", "name: z\nurl: http://x\n");
        dir.write("sub/one.posting.yaml", "name: one\nurl: http://x\n");
        dir.write("notes.txt", "ignored");

        let loaded = Collection::from_directory(&dir.0).unwrap();
        assert!(loaded.failures.is_empty());
        let names: Vec<&str> = loaded
            .collection
            .requests
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, vec!["z", "a", "b"], "GET sorts before POST");
        assert_eq!(loaded.collection.children.len(), 1);
        assert_eq!(loaded.collection.children[0].name, "sub");
        assert_eq!(loaded.collection.children[0].requests[0].name, "one");
    }

    #[test]
    fn a_broken_file_is_reported_and_the_rest_still_loads() {
        let dir = TempDir::new("broken");
        dir.write("good.posting.yaml", "name: good\nurl: http://x\n");
        dir.write("bad.posting.yaml", "name: [unclosed\n");

        let loaded = Collection::from_directory(&dir.0).unwrap();
        assert_eq!(loaded.collection.requests.len(), 1);
        assert_eq!(loaded.failures.len(), 1);
        assert!(loaded.failures[0].path.ends_with("bad.posting.yaml"));
    }

    #[test]
    fn a_missing_collection_directory_loads_as_empty() {
        let loaded = Collection::from_directory("/nonexistent/rusting/collection").unwrap();
        assert!(loaded.collection.requests.is_empty());
        assert!(loaded.collection.children.is_empty());
    }

    #[test]
    fn name_falls_back_to_the_filename() {
        let dir = TempDir::new("noname");
        dir.write("get-one.posting.yaml", "url: http://x\n");
        let loaded = Collection::from_directory(&dir.0).unwrap();
        assert_eq!(loaded.collection.requests[0].name, "get-one");
    }

    #[test]
    fn insertion_index_keeps_the_list_sorted() {
        let mut collection = Collection::new("/tmp/x");
        collection.requests = vec![
            RequestModel {
                name: "a".into(),
                ..Default::default()
            },
            RequestModel {
                name: "c".into(),
                ..Default::default()
            },
            RequestModel {
                name: "b".into(),
                method: HttpMethod::Post,
                ..Default::default()
            },
        ];
        let new = RequestModel {
            name: "b".into(),
            ..Default::default()
        };
        assert_eq!(collection.insertion_index(&new), 1);
    }

    #[test]
    fn ensure_path_creates_intermediate_nodes_and_directories() {
        let dir = TempDir::new("ensure");
        let mut collection = Collection::new(&dir.0);
        let target = dir.0.join("a").join("b");
        let leaf = collection.ensure_path(&target).unwrap();
        assert_eq!(leaf.path, target);
        assert!(target.is_dir());
        assert_eq!(collection.children.len(), 1);
        assert_eq!(collection.children[0].children[0].name, "b");
        // Idempotent.
        collection.ensure_path(&target).unwrap();
        assert_eq!(collection.children.len(), 1);
        assert_eq!(collection.children[0].children.len(), 1);
    }

    #[test]
    fn save_then_load_round_trips_through_disk() {
        let dir = TempDir::new("save");
        let path = dir.0.join("saved.posting.yaml");
        let request = RequestModel {
            name: "saved".into(),
            method: HttpMethod::Post,
            url: "https://example.com/:id".into(),
            path: Some(path.clone()),
            ..Default::default()
        };
        save_request(&request).unwrap();
        let reloaded = load_request(&path).unwrap();
        assert_eq!(reloaded.name, "saved");
        assert_eq!(reloaded.method, HttpMethod::Post);
        assert_eq!(reloaded.path, Some(path));
    }
}
