use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand, ValueEnum};
use rusting_core::{
    collection::{self, Collection},
    env::Environment,
    locations,
};

#[derive(Debug, Parser)]
#[command(name = "rusting", version, about = "A keyboard-first TUI HTTP client")]
pub struct Cli {
    /// Collection directory to open.
    #[arg(short, long, value_name = "DIR", global = true)]
    pub collection: Option<PathBuf>,

    /// Environment file(s), loaded in order.
    #[arg(short, long, value_name = "FILE", global = true)]
    pub env: Vec<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print a rusting filesystem location.
    Locate {
        #[arg(value_enum)]
        target: LocateTarget,
    },
    /// Import an API specification as a collection.
    Import {
        /// Specification type.
        #[arg(short = 't', long = "type", value_enum, default_value_t = ImportType::Openapi)]
        kind: ImportType,
        /// Destination collection directory.
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
        /// OpenAPI YAML or JSON file.
        #[arg(value_name = "SPEC")]
        spec: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LocateTarget {
    Config,
    Collection,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ImportType {
    Openapi,
}

pub async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    match cli.command {
        Some(Command::Locate { target }) => {
            let path = match target {
                LocateTarget::Config => locations::config_file()?,
                LocateTarget::Collection => locations::default_collection_directory()?,
            };
            println!("{}", path.display());
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Import {
            kind: ImportType::Openapi,
            output,
            spec,
        }) => {
            import_openapi(&spec, output.as_deref())?;
            Ok(ExitCode::SUCCESS)
        }
        None => {
            run_tui(cli.collection.as_deref(), &cli.env).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn run_tui(collection: Option<&Path>, requested_env: &[PathBuf]) -> anyhow::Result<()> {
    let env_paths = resolve_env_paths(requested_env)?;
    let dotenv_only = Environment::load(env_paths.clone(), false)?;
    let config_path = locations::config_file()?;
    let settings = rusting_core::config::load(Some(&config_path), dotenv_only.file_values())?;
    let environment = Environment::load(env_paths, settings.use_host_environment)?;

    let collection_path = match collection {
        Some(path) => absolute_existing_directory(path)?,
        None => locations::default_collection_directory()?,
    };
    let loaded = Collection::from_directory(&collection_path)?;
    let collection_state_file = locations::collection_browser_state_file().ok();
    let app = rusting_tui::app::App::new(
        settings,
        environment,
        loaded.collection,
        loaded.failures,
        collection_state_file,
    )?;
    app.run().await
}

fn resolve_env_paths(requested: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let candidates = if requested.is_empty() {
        let implicit = std::env::current_dir()
            .context("could not determine current directory")?
            .join(locations::IMPLICIT_ENV_FILE);
        if implicit.is_file() {
            vec![implicit]
        } else {
            Vec::new()
        }
    } else {
        requested.to_vec()
    };

    candidates
        .into_iter()
        .map(|path| {
            if !path.is_file() {
                bail!("environment file does not exist: {}", path.display());
            }
            path.canonicalize()
                .with_context(|| format!("could not resolve {}", path.display()))
        })
        .collect()
}

fn absolute_existing_directory(path: &Path) -> anyhow::Result<PathBuf> {
    if !path.is_dir() {
        bail!("collection directory does not exist: {}", path.display());
    }
    path.canonicalize()
        .with_context(|| format!("could not resolve collection directory {}", path.display()))
}

fn import_openapi(spec: &Path, requested_output: Option<&Path>) -> anyhow::Result<PathBuf> {
    if !spec.is_file() {
        bail!("specification does not exist: {}", spec.display());
    }
    let spec = spec
        .canonicalize()
        .with_context(|| format!("could not resolve specification {}", spec.display()))?;

    let default_root = locations::default_collection_directory()?;
    let provisional_name = spec
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("imported");
    let provisional_output = requested_output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_root.join(provisional_name));
    let mut imported = rusting_openapi::import(&spec, &provisional_output)?;

    let output = match requested_output {
        Some(path) => path.to_path_buf(),
        None => {
            let name = if imported.collection.name.is_empty() {
                provisional_name
            } else {
                imported.collection.name.as_str()
            };
            default_root.join(name)
        }
    };
    if output.exists() && !output.is_dir() {
        bail!("import output is not a directory: {}", output.display());
    }
    std::fs::create_dir_all(&output)
        .with_context(|| format!("could not create import output {}", output.display()))?;

    let old_root = imported.collection.path.clone();
    rebase_collection(&mut imported.collection, &old_root, &output)?;
    save_collection(&imported.collection)?;

    let name = if imported.collection.name.is_empty() {
        provisional_name
    } else {
        imported.collection.name.as_str()
    };
    let env_path = output.join(format!("{name}.env"));
    let mut contents = String::new();
    for (key, value) in &imported.env {
        contents.push_str(key);
        contents.push('=');
        contents.push_str(&dotenv_value(value));
        contents.push('\n');
    }
    std::fs::write(&env_path, contents)
        .with_context(|| format!("could not write {}", env_path.display()))?;

    println!("Imported OpenAPI collection to {}", output.display());
    Ok(output)
}

fn rebase_collection(
    collection: &mut Collection,
    old_root: &Path,
    new_root: &Path,
) -> anyhow::Result<()> {
    let relative = collection.path.strip_prefix(old_root).with_context(|| {
        format!(
            "importer returned collection path {} outside {}",
            collection.path.display(),
            old_root.display()
        )
    })?;
    collection.path = new_root.join(relative);
    for request in &mut collection.requests {
        let old_path = request
            .path
            .as_ref()
            .context("importer returned a request without a path")?;
        let relative = old_path.strip_prefix(old_root).with_context(|| {
            format!(
                "importer returned request path {} outside {}",
                old_path.display(),
                old_root.display()
            )
        })?;
        request.path = Some(new_root.join(relative));
    }
    for child in &mut collection.children {
        rebase_collection(child, old_root, new_root)?;
    }
    Ok(())
}

fn save_collection(collection: &Collection) -> anyhow::Result<()> {
    std::fs::create_dir_all(&collection.path)
        .with_context(|| format!("could not create {}", collection.path.display()))?;
    for request in &collection.requests {
        collection::save_request(request)?;
    }
    for child in &collection.children {
        save_collection(child)?;
    }
    Ok(())
}

fn dotenv_value(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "-._/:".contains(character))
    {
        value.to_owned()
    } else {
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        format!("\"{escaped}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_locate_and_openapi_import_forms() {
        let run = Cli::try_parse_from(["rusting", "-c", "requests", "-e", ".env"]).unwrap();
        assert!(run.command.is_none());
        assert_eq!(run.env, [PathBuf::from(".env")]);

        let locate = Cli::try_parse_from(["rusting", "locate", "config"]).unwrap();
        assert!(matches!(
            locate.command,
            Some(Command::Locate {
                target: LocateTarget::Config
            })
        ));

        let import = Cli::try_parse_from([
            "rusting", "import", "--type", "openapi", "--output", "out", "api.yaml",
        ])
        .unwrap();
        assert!(matches!(import.command, Some(Command::Import { .. })));
        assert!(Cli::try_parse_from(["rusting", "import", "-t", "postman", "x.json"]).is_err());
    }

    #[test]
    fn dotenv_values_preserve_spaces_and_special_characters() {
        assert_eq!(dotenv_value("https://example.test"), "https://example.test");
        assert_eq!(dotenv_value("a b\"c"), "\"a b\\\"c\"");
    }
}
