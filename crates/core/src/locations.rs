//! XDG locations. Every accessor creates the directory it names, so callers
//! can write immediately.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Overrides the config file path entirely.
pub const CONFIG_FILE_ENV: &str = "RUSTING_CONFIG_FILE";

/// Prefix for settings supplied through the environment.
pub const ENV_PREFIX: &str = "RUSTING_";

/// Separator for nested settings keys in environment variables, e.g.
/// `RUSTING_SSL__CA_BUNDLE`.
pub const ENV_NESTED_SEPARATOR: &str = "__";

/// `.env` file loaded from the working directory when `--env` is not given.
pub const IMPLICIT_ENV_FILE: &str = "rusting.env";

fn project_dirs() -> Result<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", "rusting")
        .context("could not determine the user's config and data directories")
}

/// `$XDG_CONFIG_HOME/rusting`
pub fn config_directory() -> Result<PathBuf> {
    let path = project_dirs()?.config_dir().to_path_buf();
    ensure_directory(&path)?;
    Ok(path)
}

/// `$XDG_DATA_HOME/rusting`
pub fn data_directory() -> Result<PathBuf> {
    let path = project_dirs()?.data_dir().to_path_buf();
    ensure_directory(&path)?;
    Ok(path)
}

/// The config file, honouring [`CONFIG_FILE_ENV`].
pub fn config_file() -> Result<PathBuf> {
    if let Some(override_path) = std::env::var_os(CONFIG_FILE_ENV) {
        let path = PathBuf::from(override_path);
        if let Some(parent) = path.parent() {
            ensure_directory(parent)?;
        }
        return Ok(path);
    }
    Ok(config_directory()?.join("config.yaml"))
}

/// The collection used when `--collection` is not given.
pub fn default_collection_directory() -> Result<PathBuf> {
    let path = data_directory()?.join("default");
    ensure_directory(&path)?;
    Ok(path)
}

fn ensure_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("could not create directory {}", path.display()))
}
