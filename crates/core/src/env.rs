//! Loading request variables from `.env` files, the host environment, and
//! script-set session variables.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use crate::variables::Variables;

/// The variable store, kept as three layers so a `.env` reload does not lose
/// what a script set.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    /// `.env` files, in the order they were given.
    pub files: Vec<PathBuf>,
    /// Merged result of the files.
    from_files: Variables,
    /// Set by scripts; survives a file reload.
    session: Variables,
    /// Whether the host environment participates.
    use_host_environment: bool,
    /// Cached merge of all three layers.
    merged: Variables,
}

impl Environment {
    /// Reads every file in `files`, later files overriding earlier ones.
    ///
    /// A file that cannot be read is an error: the user asked for it
    /// explicitly, so silently sending requests with unresolved variables
    /// would be worse than refusing.
    pub fn load(files: Vec<PathBuf>, use_host_environment: bool) -> Result<Self> {
        let mut environment = Self {
            files,
            use_host_environment,
            ..Self::default()
        };
        environment.reload()?;
        Ok(environment)
    }

    /// Re-reads the `.env` files, preserving session variables.
    pub fn reload(&mut self) -> Result<()> {
        let mut from_files = Variables::new();
        for file in &self.files {
            for (key, value) in read_dotenv(file)? {
                from_files.insert(key, value);
            }
        }
        self.from_files = from_files;
        self.remerge();
        Ok(())
    }

    /// Replaces the file list and reloads.
    pub fn set_files(&mut self, files: Vec<PathBuf>) -> Result<()> {
        self.files = files;
        self.reload()
    }

    /// The `.env` contents alone, which also feed the settings loader.
    pub fn file_values(&self) -> &Variables {
        &self.from_files
    }

    /// Everything a request can reference.
    pub fn variables(&self) -> &Variables {
        &self.merged
    }

    pub fn session(&self) -> &Variables {
        &self.session
    }

    pub fn set_session_variable(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.session.insert(name.into(), value.into());
        self.remerge();
    }

    pub fn clear_session_variable(&mut self, name: &str) {
        self.session.remove(name);
        self.remerge();
    }

    pub fn clear_session(&mut self) {
        self.session.clear();
        self.remerge();
    }

    /// Session variables win, then the host environment, then the files.
    fn remerge(&mut self) {
        let mut merged = self.from_files.clone();
        if self.use_host_environment {
            merged.extend(std::env::vars());
        }
        merged.extend(self.session.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.merged = merged;
    }
}

/// Parses one `.env` file. Quotes are stripped, `export ` prefixes accepted,
/// `#` comments ignored — `dotenvy`'s rules.
pub fn read_dotenv(path: &Path) -> Result<BTreeMap<String, String>> {
    let iter = dotenvy::from_path_iter(path)
        .with_context(|| format!("could not read environment file {}", path.display()))?;
    let mut values = BTreeMap::new();
    for entry in iter {
        let (key, value) = entry
            .with_context(|| format!("could not parse environment file {}", path.display()))?;
        values.insert(key, value);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(tag: &str, contents: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("rusting-env-{tag}-{}.env", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn later_files_override_earlier_ones() {
        let base = write(
            "base",
            "POST_ID=1\nUSER_ID=2\nFILE=\"base\"\nONLY_BASE=true\n",
        );
        let extra = write("extra", "FILE=\"extra\"\nPOST_ID=2\nONLY_EXTRA=true\n");

        let environment = Environment::load(vec![base.clone(), extra.clone()], false).unwrap();
        let vars = environment.variables();
        assert_eq!(vars.get("POST_ID").map(String::as_str), Some("2"));
        assert_eq!(vars.get("USER_ID").map(String::as_str), Some("2"));
        assert_eq!(vars.get("FILE").map(String::as_str), Some("extra"));
        assert_eq!(vars.get("ONLY_BASE").map(String::as_str), Some("true"));
        assert_eq!(vars.get("ONLY_EXTRA").map(String::as_str), Some("true"));

        std::fs::remove_file(base).unwrap();
        std::fs::remove_file(extra).unwrap();
    }

    #[test]
    fn session_variables_outrank_files_and_survive_reload() {
        let path = write("session", "TOKEN=from-file\n");
        let mut environment = Environment::load(vec![path.clone()], false).unwrap();
        environment.set_session_variable("TOKEN", "from-script");
        assert_eq!(
            environment.variables().get("TOKEN").map(String::as_str),
            Some("from-script")
        );

        environment.reload().unwrap();
        assert_eq!(
            environment.variables().get("TOKEN").map(String::as_str),
            Some("from-script"),
            "reload must not lose session variables"
        );

        environment.clear_session_variable("TOKEN");
        assert_eq!(
            environment.variables().get("TOKEN").map(String::as_str),
            Some("from-file"),
            "clearing reveals the file value again"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn host_environment_is_opt_in() {
        // PATH is set in every environment this runs in, and mutating the
        // process environment is unsafe in edition 2024, so read rather than write.
        let expected = std::env::var("PATH").expect("PATH must be set");

        let without = Environment::load(Vec::new(), false).unwrap();
        assert!(without.variables().get("PATH").is_none());

        let with = Environment::load(Vec::new(), true).unwrap();
        assert_eq!(with.variables().get("PATH"), Some(&expected));
    }

    #[test]
    fn a_missing_env_file_is_an_error() {
        let error =
            Environment::load(vec![PathBuf::from("/nonexistent/x.env")], false).unwrap_err();
        assert!(format!("{error:#}").contains("x.env"));
    }
}
