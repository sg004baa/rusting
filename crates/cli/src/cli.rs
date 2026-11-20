use std::{
    io::Write as _,
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
    /// Export a collection for another API client.
    Export {
        /// Export format.
        #[arg(short = 't', long = "type", value_enum, default_value_t = ExportType::Postman)]
        kind: ExportType,
        /// Source collection directory.
        #[arg(value_name = "DIR", conflicts_with = "collection")]
        directory: Option<PathBuf>,
        /// Destination JSON file. Must not already exist.
        #[arg(short, long, value_name = "FILE")]
        output: PathBuf,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ExportType {
    Postman,
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
        Some(Command::Export {
            kind: ExportType::Postman,
            directory,
            output,
        }) => {
            if directory.is_some() && cli.collection.is_some() {
                bail!("export cannot combine a source directory with --collection");
            }
            if !cli.env.is_empty() {
                bail!(
                    "export does not accept --env; variable references are exported instead of environment values"
                );
            }
            let source = match directory.as_deref().or(cli.collection.as_deref()) {
                Some(path) => path.to_path_buf(),
                None => locations::default_collection_directory()?,
            };
            export_postman(&source, &output)?;
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

fn export_postman(source: &Path, output: &Path) -> anyhow::Result<()> {
    let source = absolute_existing_directory(source)?;
    let loaded = Collection::from_directory(&source)?;
    if !loaded.failures.is_empty() {
        let mut message = String::from("cannot export an incomplete collection:");
        for failure in loaded.failures {
            use std::fmt::Write as _;
            write!(
                message,
                "\n{}: invalid request YAML",
                failure.path.display()
            )?;
        }
        bail!("{message}");
    }

    let exported = crate::postman::export(&loaded.collection)?;
    let mut bytes = serde_json::to_vec_pretty(&exported.document)
        .context("could not serialize Postman export")?;
    bytes.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .with_context(|| {
            format!(
                "could not create export file {} (the file must not already exist)",
                output.display()
            )
        })?;
    file.write_all(&bytes)
        .with_context(|| format!("could not write export file {}", output.display()))?;
    for warning in exported.warnings {
        eprintln!("Warning: {warning}");
    }
    println!("Exported Postman collection to {}", output.display());
    Ok(())
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

    #[tokio::test]
    async fn parses_postman_export_and_rejects_ambiguous_sources() {
        let export = Cli::try_parse_from(["rusting", "export", "api", "-o", "api.json"]).unwrap();
        assert!(matches!(
            export.command,
            Some(Command::Export {
                kind: ExportType::Postman,
                directory: Some(_),
                ..
            })
        ));
        assert!(Cli::try_parse_from(["rusting", "-c", "api", "export", "-o", "api.json"]).is_ok());
        assert!(Cli::try_parse_from(["rusting", "export", "-o", "api.json"]).is_ok());
        assert!(Cli::try_parse_from(["rusting", "export", "api"]).is_err());
        let temporary = tempfile::tempdir().unwrap();
        let one = temporary.path().join("one");
        let two = temporary.path().join("two");
        let output = temporary.path().join("api.json");
        std::fs::create_dir(&one).unwrap();
        std::fs::create_dir(&two).unwrap();
        use std::ffi::OsStr;
        for arguments in [
            [
                OsStr::new("rusting"),
                OsStr::new("-c"),
                one.as_os_str(),
                OsStr::new("export"),
                two.as_os_str(),
                OsStr::new("-o"),
                output.as_os_str(),
            ],
            [
                OsStr::new("rusting"),
                OsStr::new("export"),
                two.as_os_str(),
                OsStr::new("-c"),
                one.as_os_str(),
                OsStr::new("-o"),
                output.as_os_str(),
            ],
        ] {
            let error = match Cli::try_parse_from(arguments) {
                Ok(cli) => run(cli).await.unwrap_err().to_string(),
                Err(error) => error.to_string(),
            };
            assert!(error.contains("--collection"));
            assert!(!output.exists());
        }
        assert!(
            Cli::try_parse_from(["rusting", "export", "--type", "openapi", "-o", "api.json"])
                .is_err()
        );
    }

    #[test]
    fn export_requires_complete_existing_source_without_creating_output() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("out.json");
        let missing = temporary.path().join("missing");
        assert!(export_postman(&missing, &output).is_err());
        assert!(!output.exists());

        let bad = temporary.path().join("bad.posting.yaml");
        std::fs::write(&bad, "method: NOT_A_METHOD\n").unwrap();
        std::fs::write(
            temporary.path().join("good.posting.yaml"),
            "url: https://example.test\n",
        )
        .unwrap();
        std::fs::write(temporary.path().join("also-bad.posting.yaml"), "body: [\n").unwrap();
        let error = export_postman(temporary.path(), &output).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("bad.posting.yaml"));
        assert!(message.contains("incomplete collection"));
        assert!(message.contains("also-bad.posting.yaml"));
        assert!(!output.exists());
    }

    #[test]
    fn malformed_authentication_diagnostics_do_not_expose_secret_scalars() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("out.json");
        std::fs::write(
            temporary.path().join("bad-auth.posting.yaml"),
            "auth:\n  type: basic\n  basic: top-secret-credential-scalar\n",
        )
        .unwrap();
        let error = export_postman(temporary.path(), &output).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("bad-auth.posting.yaml"));
        assert!(message.contains("invalid request YAML"));
        assert!(!message.contains("top-secret-credential-scalar"));
        assert!(!output.exists());
    }

    #[test]
    fn export_refuses_to_overwrite_outputs_or_source_yaml() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("one.posting.yaml");
        let yaml = "name: One\nurl: https://example.test\n";
        std::fs::write(&source, yaml).unwrap();
        let output = temporary.path().join("out.json");
        std::fs::write(&output, "keep this file").unwrap();

        assert!(export_postman(temporary.path(), &output).is_err());
        assert_eq!(std::fs::read_to_string(&output).unwrap(), "keep this file");
        assert!(export_postman(temporary.path(), &source).is_err());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), yaml);
    }

    #[test]
    fn export_writes_postman_json_and_does_not_create_parent_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("out.json");
        export_postman(temporary.path(), &output).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        assert!(bytes.ends_with(b"\n"));
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(document["item"], serde_json::json!([]));
        assert_eq!(
            document["info"]["schema"],
            "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
        );

        let nested = temporary.path().join("missing").join("out.json");
        assert!(export_postman(temporary.path(), &nested).is_err());
        assert!(!nested.parent().unwrap().exists());
    }

    #[test]
    fn invalid_conversion_does_not_create_an_output_file() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("out.json");
        std::fs::write(
            temporary.path().join("invalid.posting.yaml"),
            "name: Invalid form\nbody:\n  form_data: []\n  content_type: text/plain\n",
        )
        .unwrap();
        let error = export_postman(temporary.path(), &output).unwrap_err();
        assert!(format!("{error:#}").contains("Invalid form"));
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn export_uses_global_collection_and_keeps_environment_references() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("out.json");
        std::fs::write(
            temporary.path().join("one.posting.yaml"),
            "url: ${BASE_URL}/items\nheaders:\n- name: X-Path\n  value: $PATH\n",
        )
        .unwrap();
        std::fs::write(
            temporary.path().join("rusting.env"),
            "BASE_URL=do-not-export\n",
        )
        .unwrap();
        let cli = Cli {
            collection: Some(temporary.path().to_path_buf()),
            env: Vec::new(),
            command: Some(Command::Export {
                kind: ExportType::Postman,
                directory: None,
                output: output.clone(),
            }),
        };
        assert_eq!(run(cli).await.unwrap(), ExitCode::SUCCESS);
        let contents = std::fs::read_to_string(output).unwrap();
        let document: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(
            document["item"][0]["request"]["url"]["raw"],
            "{{BASE_URL}}/items"
        );
        assert_eq!(
            document["item"][0]["request"]["header"][0]["value"],
            "{{PATH}}"
        );
        assert!(!contents.contains("do-not-export"));
    }

    #[tokio::test]
    async fn export_rejects_environment_files_without_creating_output() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("out.json");
        let cli = Cli {
            collection: Some(temporary.path().to_path_buf()),
            env: vec![temporary.path().join("secret.env")],
            command: Some(Command::Export {
                kind: ExportType::Postman,
                directory: None,
                output: output.clone(),
            }),
        };
        let error = run(cli).await.unwrap_err();
        assert!(error.to_string().contains("variable references"));
        assert!(!output.exists());
    }
}
