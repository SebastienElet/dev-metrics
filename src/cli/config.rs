use super::error::{CommandError, ErrorCode};
use clap::{Subcommand, ValueEnum};
use dev_metrics::config::{Config, configured_value, environment_value, validate_setting};
use serde_json::{Value, json};
use std::io::{self, IsTerminal, Write};
use std::path::Path;

const KEYS: [&str; 6] = [
    "github.token",
    "bitbucket.username",
    "bitbucket.token",
    "bitbucket.workspace",
    "metrics.excluded_files",
    "output_dir",
];

#[derive(Clone, Copy)]
enum SaveMode {
    CreateNew,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum ConfigFormat {
    Text,
    Json,
}

#[derive(Debug, PartialEq, Subcommand)]
pub enum ConfigAction {
    #[command(about = "Créer le fichier de configuration")]
    Init {
        #[arg(
            long,
            help = "Aucun prompt ; les valeurs viennent uniquement des flags"
        )]
        non_interactive: bool,
        #[arg(long, help = "Remplacer un fichier existant")]
        force: bool,
        #[arg(long)]
        github_token: Option<String>,
        #[arg(long)]
        bitbucket_username: Option<String>,
        #[arg(long)]
        bitbucket_token: Option<String>,
        #[arg(long)]
        bitbucket_workspace: Option<String>,
        #[arg(long, help = "Motifs séparés par des virgules")]
        metrics_excluded_files: Option<String>,
        #[arg(long)]
        output_dir: Option<String>,
    },
    #[command(about = "Définir une clé")]
    Set { key: String, value: String },
    #[command(about = "Lire une clé et sa source")]
    Get {
        key: String,
        #[arg(long)]
        show_secrets: bool,
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },
    #[command(about = "Supprimer une clé du fichier")]
    Unset { key: String },
    #[command(about = "Lister les clés, valeurs et sources")]
    List {
        #[arg(long)]
        show_secrets: bool,
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },
    #[command(about = "Afficher le chemin effectif du fichier")]
    Path {
        #[arg(long, value_enum, default_value = "text")]
        format: ConfigFormat,
    },
}

fn config_error(message: impl ToString) -> CommandError {
    CommandError::new(ErrorCode::ConfigurationError, message)
}

pub fn run(action: ConfigAction, path: &Path) -> Result<String, CommandError> {
    match action {
        ConfigAction::Path { format } => match format {
            ConfigFormat::Text => Ok(path.display().to_string()),
            ConfigFormat::Json => Ok(json!({ "path": path.display().to_string() }).to_string()),
        },
        ConfigAction::Init {
            non_interactive,
            force,
            github_token,
            bitbucket_username,
            bitbucket_token,
            bitbucket_workspace,
            metrics_excluded_files,
            output_dir,
        } => {
            let mode = if force {
                SaveMode::Replace
            } else if path.exists() {
                if non_interactive || !confirm_overwrite()? {
                    return Err(config_error(
                        "Configuration existante ; utilisez --force pour la remplacer",
                    ));
                }
                SaveMode::Replace
            } else {
                SaveMode::CreateNew
            };
            let mut values = [
                github_token,
                bitbucket_username,
                bitbucket_token,
                bitbucket_workspace,
                metrics_excluded_files,
                output_dir,
            ];
            if !non_interactive {
                for (key, value) in KEYS.iter().zip(&mut values) {
                    if value.is_none() {
                        *value = prompt(key)?;
                    }
                }
            }
            let mut table = toml::Table::new();
            for (key, value) in KEYS.iter().zip(values) {
                if let Some(value) = value {
                    set_value(&mut table, key, &value)?;
                }
            }
            save(path, &table, mode)?;
            Ok(path.display().to_string())
        }
        ConfigAction::Set { key, value } => {
            validate_setting(&key, &value).map_err(config_error)?;
            let mut table = read(path)?;
            set_value(&mut table, &key, &value)?;
            save(path, &table, SaveMode::Replace)?;
            Ok(format!("{key} enregistré"))
        }
        ConfigAction::Unset { key } => {
            validate_key(&key)?;
            let mut table = read(path)?;
            remove_value(&mut table, &key);
            save(path, &table, SaveMode::Replace)?;
            Ok(format!("{key} supprimé"))
        }
        ConfigAction::Get {
            key,
            show_secrets,
            format,
        } => {
            validate_key(&key)?;
            let table = read(path)?;
            let entry = entry(&table, &key, show_secrets)?;
            match format {
                ConfigFormat::Json => Ok(entry.to_string()),
                ConfigFormat::Text => Ok(format!(
                    "{} ({})",
                    display_value(&entry["value"]),
                    entry["source"].as_str().unwrap()
                )),
            }
        }
        ConfigAction::List {
            show_secrets,
            format,
        } => {
            let table = read(path)?;
            let values = KEYS
                .iter()
                .map(|key| Ok(((*key).to_owned(), entry(&table, key, show_secrets)?)))
                .collect::<Result<serde_json::Map<String, Value>, CommandError>>()?;
            match format {
                ConfigFormat::Json => Ok(Value::Object(values).to_string()),
                ConfigFormat::Text => Ok(values
                    .iter()
                    .map(|(key, entry)| {
                        format!(
                            "{key} = {} ({})",
                            display_value(&entry["value"]),
                            entry["source"].as_str().unwrap()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")),
            }
        }
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "<absent>".into(),
        _ => value.to_string(),
    }
}

fn validate_key(key: &str) -> Result<(), CommandError> {
    if KEYS.contains(&key) {
        Ok(())
    } else {
        Err(config_error("Clé de configuration inconnue"))
    }
}

fn entry(table: &toml::Table, key: &str, show_secrets: bool) -> Result<Value, CommandError> {
    let file_value = configured_value(table, key);
    let environment = environment_value(key).map_err(config_error)?;
    let source = if environment.is_some() {
        "env"
    } else if file_value.is_some() {
        "file"
    } else {
        "unset"
    };
    let value = match environment {
        Some(value) => {
            validate_setting(key, &value).map_err(config_error)?;
            Some(setting_value(key, &value))
        }
        None => {
            if let Some(value) = file_value {
                let text = if key == "metrics.excluded_files" {
                    value
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|item| item.as_str().unwrap())
                        .collect::<Vec<_>>()
                        .join(",")
                } else {
                    value.as_str().unwrap().to_owned()
                };
                validate_setting(key, &text).map_err(config_error)?;
            }
            file_value.cloned()
        }
    };
    let value = if key.ends_with(".token") && !show_secrets && value.is_some() {
        json!("[masqué]")
    } else {
        value
            .map(|value| serde_json::to_value(value).unwrap())
            .unwrap_or(Value::Null)
    };
    Ok(json!({ "key": key, "value": value, "source": source }))
}

fn read(path: &Path) -> Result<toml::Table, CommandError> {
    match std::fs::read_to_string(path) {
        Ok(source) => {
            Config::parse(&source).map_err(|_| {
                config_error(format!("Configuration invalide dans {}", path.display()))
            })?;
            toml::from_str(&source).map_err(|_| {
                config_error(format!("Configuration invalide dans {}", path.display()))
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(_) => Err(config_error(format!(
            "Impossible de lire {}",
            path.display()
        ))),
    }
}

fn setting_value(key: &str, value: &str) -> toml::Value {
    if key == "metrics.excluded_files" {
        toml::Value::Array(if value.is_empty() {
            Vec::new()
        } else {
            value
                .split(',')
                .map(|part| toml::Value::String(part.trim().to_owned()))
                .collect()
        })
    } else {
        toml::Value::String(value.to_owned())
    }
}

fn set_value(table: &mut toml::Table, key: &str, value: &str) -> Result<(), CommandError> {
    validate_setting(key, value).map_err(config_error)?;
    if let Some((section, field)) = key.split_once('.') {
        let section = table
            .entry(section)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        section
            .as_table_mut()
            .unwrap()
            .insert(field.to_owned(), setting_value(key, value));
    } else {
        table.insert(key.to_owned(), setting_value(key, value));
    }
    Ok(())
}

fn remove_value(table: &mut toml::Table, key: &str) {
    if let Some((section, field)) = key.split_once('.') {
        if let Some(values) = table.get_mut(section).and_then(toml::Value::as_table_mut) {
            values.remove(field);
        }
    } else {
        table.remove(key);
    }
}

fn save(path: &Path, table: &toml::Table, mode: SaveMode) -> Result<(), CommandError> {
    let source = toml::to_string_pretty(table)
        .map_err(|_| config_error("Configuration impossible à sérialiser"))?;
    Config::parse(&source).map_err(|_| config_error("Configuration produite invalide"))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|_| config_error(format!("Impossible de créer {}", parent.display())))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| config_error("Impossible de créer le fichier temporaire"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| config_error("Impossible de protéger le fichier"))?;
    }
    temporary
        .write_all(source.as_bytes())
        .map_err(|_| config_error("Impossible d'écrire la configuration"))?;
    match mode {
        SaveMode::CreateNew => temporary.persist_noclobber(path).map_err(|_| {
            config_error("Configuration existante ; utilisez --force pour la remplacer")
        })?,
        SaveMode::Replace => temporary
            .persist(path)
            .map_err(|_| config_error("Impossible de remplacer la configuration"))?,
    };
    Ok(())
}

fn prompt(key: &str) -> Result<Option<String>, CommandError> {
    show_prompt(&format!("{key} (vide pour ignorer) : "))?;
    let mut value = String::new();
    let length = io::stdin()
        .read_line(&mut value)
        .map_err(|_| config_error("Impossible de lire la réponse"))?;
    if length == 0 {
        return Err(config_error("Entrée interactive indisponible"));
    }
    let value = value.trim_end_matches(['\r', '\n']);
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn confirm_overwrite() -> Result<bool, CommandError> {
    show_prompt("Remplacer la configuration existante ? [y/N] ")?;
    let mut response = String::new();
    io::stdin()
        .read_line(&mut response)
        .map_err(|_| config_error("Impossible de lire la réponse"))?;
    Ok(matches!(response.trim(), "y" | "Y" | "yes" | "oui"))
}

fn show_prompt(message: &str) -> Result<(), CommandError> {
    if !io::stdin().is_terminal() {
        return Ok(());
    }
    #[cfg(unix)]
    let terminal = "/dev/tty";
    #[cfg(windows)]
    let terminal = "CONOUT$";
    #[cfg(not(any(unix, windows)))]
    let terminal = "";
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .open(terminal)
        .map_err(|_| config_error("Impossible d'afficher le prompt"))?;
    output
        .write_all(message.as_bytes())
        .map_err(|_| config_error("Impossible d'afficher le prompt"))?;
    output
        .flush()
        .map_err(|_| config_error("Impossible d'afficher le prompt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_new_does_not_replace_a_file_created_after_the_initial_check() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "output_dir='first'").unwrap();
        let mut table = toml::Table::new();
        set_value(&mut table, "output_dir", "second").unwrap();
        assert!(save(&path, &table, SaveMode::CreateNew).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "output_dir='first'");
    }
}
