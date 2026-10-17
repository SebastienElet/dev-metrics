use super::error::{CommandError, ErrorCode};
use clap::{Subcommand, ValueEnum};
use dev_metrics::config::{Config, configured_value, environment_value, validate_setting};
use serde_json::{Value, json};
use std::io::{self, IsTerminal, Write};
use std::path::Path;

pub mod check;

pub const GITHUB_TOKEN_HELP: &str = "GitHub : fine-grained PAT en lecture seule : Metadata (inclus), Contents, Pull requests.\nCréer : https://github.com/settings/personal-access-tokens/new\nAlternative classic : repo (privé) ou public_repo (public), read:org si organisations.\n";
pub const BITBUCKET_TOKEN_HELP: &str = "Bitbucket Cloud : API token avec read:user:bitbucket, read:workspace:bitbucket, read:repository:bitbucket et read:pullrequest:bitbucket.\nCréer : https://id.atlassian.com/manage-profile/security/api-tokens\nbitbucket.email : email Atlassian pour l'authentification Basic ; facultatif avec Bearer.\nbitbucket.username : identifiant du compte dont les PR sont suivies, jamais l'email d'authentification.\n";

const KEYS: [&str; 7] = [
    "github.token",
    "bitbucket.email",
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

#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum CheckFormat {
    Table,
    Json,
}

#[derive(Debug, PartialEq, Subcommand)]
pub enum ConfigAction {
    #[command(about = "Vérifier les droits des tokens configurés auprès des API")]
    Check {
        #[arg(long, value_enum, default_value = "table")]
        format: CheckFormat,
    },
    #[command(about = "Créer le fichier de configuration", after_help = format!("{GITHUB_TOKEN_HELP}{BITBUCKET_TOKEN_HELP}"))]
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
        #[arg(
            long,
            help = "Email Atlassian pour l'authentification Basic ; facultatif avec Bearer"
        )]
        bitbucket_email: Option<String>,
        #[arg(
            long,
            help = "Compte Bitbucket suivi (slug, Account ID ou UUID) ; un email est enregistré dans bitbucket.email"
        )]
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

fn setting_hint(key: &str) -> (&'static str, &'static str) {
    match key {
        "github.token" | "bitbucket.token" => ("token non vide sans espace", "fake-token"),
        "bitbucket.email" => ("email Atlassian", "prenom.nom@domaine.com"),
        "bitbucket.username" => (
            "slug, Account ID ou UUID Bitbucket ; utilisez bitbucket.email pour l'authentification",
            "alice",
        ),
        "bitbucket.workspace" => (
            "slug de workspace sans espace ni barre oblique",
            "mon-equipe",
        ),
        "metrics.excluded_files" => (
            "motifs non vides séparés par des virgules",
            "*.snap,generated/*",
        ),
        "output_dir" => ("chemin non vide", "./reports"),
        _ => ("clé connue et valeur valide", "bitbucket.email"),
    }
}

fn setting_error(key: &str) -> CommandError {
    let (expected, example) = setting_hint(key);
    let mut error = config_error(format!(
        "Valeur invalide pour {key} : attendu {expected} (exemple : {example})"
    ));
    error.details = json!({ "key": key, "expected": expected, "example": example });
    error
}

fn validate_cli_setting(key: &str, value: &str) -> Result<(), CommandError> {
    validate_setting(key, value).map_err(|_| setting_error(key))
}

fn report_interactive_error(error: &CommandError) -> Result<(), CommandError> {
    let message = serde_json::to_string(error)
        .map_err(|_| config_error("Impossible d'afficher l'erreur de saisie"))?;
    writeln!(io::stderr().lock(), "{message}")
        .map_err(|_| config_error("Impossible d'afficher l'erreur de saisie"))
}

pub fn run(action: ConfigAction, path: &Path) -> Result<String, CommandError> {
    match action {
        ConfigAction::Check { .. } => unreachable!(),
        ConfigAction::Path { format } => match format {
            ConfigFormat::Text => Ok(path.display().to_string()),
            ConfigFormat::Json => Ok(json!({ "path": path.display().to_string() }).to_string()),
        },
        ConfigAction::Init {
            non_interactive,
            force,
            github_token,
            bitbucket_email,
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
                bitbucket_email,
                bitbucket_username,
                bitbucket_token,
                bitbucket_workspace,
                metrics_excluded_files,
                output_dir,
            ];
            for index in 0..KEYS.len() {
                let key = KEYS[index];
                loop {
                    let candidate = if values[index].is_some() || non_interactive {
                        values[index].take()
                    } else {
                        prompt(key)?
                    };
                    if key == "bitbucket.username"
                        && candidate
                            .as_deref()
                            .is_some_and(|value| value.contains('@'))
                        && values[1].is_none()
                    {
                        let email = candidate.as_deref().unwrap();
                        if let Err(error) = validate_cli_setting("bitbucket.email", email) {
                            if non_interactive {
                                return Err(error);
                            }
                            report_interactive_error(&error)?;
                            continue;
                        }
                        values[1] = candidate;
                        if non_interactive {
                            break;
                        }
                        show_prompt(
                            "Email enregistré dans bitbucket.email ; indiquez le compte à suivre ou laissez vide pour le compte authentifié.\n",
                        )?;
                        continue;
                    }
                    if let Some(ref candidate) = candidate
                        && let Err(error) = validate_cli_setting(key, candidate)
                    {
                        if non_interactive {
                            return Err(error);
                        }
                        report_interactive_error(&error)?;
                        continue;
                    }
                    values[index] = candidate;
                    break;
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
            validate_cli_setting(&key, &value)?;
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
            validate_cli_setting(key, &value)?;
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
                validate_cli_setting(key, &text)?;
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
    validate_cli_setting(key, value)?;
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
    let destination = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            std::fs::canonicalize(path).map_err(|_| {
                config_error(format!(
                    "Lien de configuration invalide : {}",
                    path.display()
                ))
            })?
        }
        Ok(_) => path.to_path_buf(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(_) => {
            return Err(config_error(format!(
                "Impossible d'inspecter {}",
                path.display()
            )));
        }
    };
    let parent = destination
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
        SaveMode::CreateNew => temporary.persist_noclobber(&destination).map_err(|_| {
            config_error("Configuration existante ; utilisez --force pour la remplacer")
        })?,
        SaveMode::Replace => temporary
            .persist(&destination)
            .map_err(|_| config_error("Impossible de remplacer la configuration"))?,
    };
    Ok(())
}

fn prompt(key: &str) -> Result<Option<String>, CommandError> {
    match key {
        "github.token" => show_prompt(GITHUB_TOKEN_HELP)?,
        "bitbucket.token" => show_prompt(BITBUCKET_TOKEN_HELP)?,
        _ => {}
    }
    #[cfg(unix)]
    if key.ends_with(".token") && io::stdin().is_terminal() {
        let echo = TerminalEcho::hide()
            .map_err(|_| config_error("Impossible de protéger la saisie du secret"))?;
        show_prompt(&format!("{key} (vide pour ignorer) : "))?;
        let mut value = String::new();
        let length = io::stdin()
            .read_line(&mut value)
            .map_err(|_| config_error("Impossible de lire le secret"))?;
        drop(echo);
        show_prompt("\n")?;
        if length == 0 {
            return Err(config_error("Entrée interactive indisponible"));
        }
        let value = value.trim_end_matches(['\r', '\n']);
        return Ok((!value.is_empty()).then(|| value.to_owned()));
    }
    #[cfg(windows)]
    if key.ends_with(".token") && io::stdin().is_terminal() {
        let value = rpassword::prompt_password(format!("{key} (vide pour ignorer) : "))
            .map_err(|_| config_error("Impossible de lire le secret"))?;
        return Ok((!value.is_empty()).then_some(value));
    }
    #[cfg(not(any(unix, windows)))]
    if key.ends_with(".token") && io::stdin().is_terminal() {
        return Err(config_error("Saisie de secret sans écho indisponible"));
    }
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

#[cfg(unix)]
struct TerminalEcho {
    descriptor: std::os::fd::RawFd,
    original: libc::termios,
}

#[cfg(unix)]
impl TerminalEcho {
    fn hide() -> io::Result<Self> {
        use std::os::fd::AsRawFd;

        let descriptor = io::stdin().as_raw_fd();
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        if unsafe { libc::tcgetattr(descriptor, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        let mut hidden = original;
        hidden.c_lflag &= !(libc::ECHO | libc::ECHONL);
        if unsafe { libc::tcsetattr(descriptor, libc::TCSANOW, &hidden) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            descriptor,
            original,
        })
    }
}

#[cfg(unix)]
impl Drop for TerminalEcho {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(self.descriptor, libc::TCSANOW, &self.original) };
    }
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
