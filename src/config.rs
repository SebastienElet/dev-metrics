use serde::Deserialize;
use std::error::Error;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub output_dir: Option<PathBuf>,
    #[serde(default)]
    pub metrics: crate::metrics::MetricsConfig,
    pub github: Option<GitHubConfig>,
    pub bitbucket: Option<BitbucketConfig>,
}

#[derive(Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GitHubConfig {
    pub token: Option<String>,
}

impl std::fmt::Debug for GitHubConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubConfig")
            .field("token", &self.token.as_ref().map(|_| "[masqué]"))
            .finish()
    }
}

#[derive(Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BitbucketConfig {
    pub username: Option<String>,
    pub workspace: Option<String>,
    pub token: Option<String>,
}

impl std::fmt::Debug for BitbucketConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BitbucketConfig")
            .field("username", &self.username)
            .field("workspace", &self.workspace)
            .field("token", &self.token.as_ref().map(|_| "[masqué]"))
            .finish()
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("Impossible de lire {} : {error}", path.display()))?;
        Self::parse(&source)
            .map_err(|_| format!("Configuration invalide dans {}", path.display()).into())
    }

    pub fn parse(source: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(source)
    }

    pub fn apply_github_environment(&mut self) -> Result<(), String> {
        if let Some(value) = environment_value("github.token")? {
            validate_setting("github.token", &value)?;
            self.github
                .get_or_insert(GitHubConfig { token: None })
                .token = Some(value);
        }
        Ok(())
    }

    pub fn apply_bitbucket_environment(&mut self) -> Result<(), String> {
        for key in [
            "bitbucket.username",
            "bitbucket.token",
            "bitbucket.workspace",
        ] {
            if key == "bitbucket.token" && std::env::var_os("DEV_METRICS_BITBUCKET_TOKEN").is_none()
            {
                continue;
            }
            if let Some(value) = environment_value(key)? {
                validate_setting(key, &value)?;
                match key {
                    "bitbucket.username" => {
                        self.bitbucket
                            .get_or_insert(BitbucketConfig {
                                username: None,
                                token: None,
                                workspace: None,
                            })
                            .username = Some(value)
                    }
                    "bitbucket.token" => {
                        self.bitbucket
                            .get_or_insert(BitbucketConfig {
                                username: None,
                                token: None,
                                workspace: None,
                            })
                            .token = Some(value)
                    }
                    "bitbucket.workspace" => {
                        self.bitbucket
                            .get_or_insert(BitbucketConfig {
                                username: None,
                                token: None,
                                workspace: None,
                            })
                            .workspace = Some(value)
                    }
                    _ => unreachable!(),
                }
            }
        }
        Ok(())
    }
}

pub fn environment_value(key: &str) -> Result<Option<String>, String> {
    let name = match key {
        "github.token" => "DEV_METRICS_GITHUB_TOKEN",
        "bitbucket.username" => "DEV_METRICS_BITBUCKET_USERNAME",
        "bitbucket.token" => "DEV_METRICS_BITBUCKET_TOKEN",
        "bitbucket.workspace" => "DEV_METRICS_BITBUCKET_WORKSPACE",
        _ => return Ok(None),
    };
    let value = std::env::var_os(name).or_else(|| {
        (key == "bitbucket.token")
            .then(|| std::env::var_os("BITBUCKET_TOKEN"))
            .flatten()
    });
    value
        .map(|value| {
            value
                .into_string()
                .map_err(|_| format!("Variable d'environnement {name} invalide"))
        })
        .transpose()
}

pub fn configured_value<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Value> {
    match key.split_once('.') {
        Some((section, field)) => table.get(section)?.as_table()?.get(field),
        None => table.get(key),
    }
}

pub fn validate_setting(key: &str, value: &str) -> Result<(), String> {
    let valid = match key {
        "github.token" | "bitbucket.token" => {
            !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic())
        }
        "bitbucket.username" => !value.trim().is_empty() && !value.chars().any(char::is_control),
        "bitbucket.workspace" => {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        }
        "metrics.excluded_files" => {
            value.is_empty()
                || value
                    .split(',')
                    .all(|part| !part.trim().is_empty() && !part.chars().any(char::is_control))
        }
        "output_dir" => !value.trim().is_empty() && !value.chars().any(char::is_control),
        _ => return Err("Clé de configuration inconnue".into()),
    };
    if valid {
        Ok(())
    } else {
        Err(format!("Valeur invalide pour {key}"))
    }
}

pub fn config_path(explicit: Option<PathBuf>, home: Option<OsString>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let home = home
        .filter(|value| !value.is_empty())
        .ok_or("HOME absent ou vide ; indiquez un fichier avec --config")?;
    Ok(PathBuf::from(home).join(".config/dev-metrics/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config() {
        let config = Config::parse(include_str!("../config.example.toml")).unwrap();
        assert_eq!(config.output_dir, Some(PathBuf::from("./reports")));
    }

    #[test]
    fn accepts_configuration_without_an_output_directory() {
        for text in ["", "[github]\ntoken='configured-token'"] {
            assert!(Config::parse(text).is_ok());
        }
    }

    #[test]
    fn rejects_invalid_config() {
        for source in [
            "output_dir = [",
            "output_dir = 42",
            "output_dir = './reports'\nunknown = true",
        ] {
            assert!(Config::parse(source).is_err(), "accepted {source:?}");
        }
    }

    #[test]
    fn accepts_optional_github_token() {
        assert!(
            Config::parse("output_dir = './reports'\n[github]\ntoken = 'configured-token'").is_ok()
        );
    }

    #[test]
    fn accepts_metrics_exclusion_configuration() {
        let config =
            Config::parse("output_dir = './reports'\n[metrics]\nexcluded_files = ['*.snap']")
                .unwrap();
        assert_eq!(config.metrics.excluded_files, vec!["*.snap"]);
        for source in [
            "output_dir = './reports'",
            "output_dir = './reports'\n[metrics]",
        ] {
            assert_eq!(
                Config::parse(source).unwrap().metrics,
                crate::metrics::MetricsConfig::default()
            );
        }
        assert!(
            Config::parse("output_dir = './reports'\n[metrics]\nexcluded_files = []")
                .unwrap()
                .metrics
                .excluded_files
                .is_empty()
        );
        for source in [
            "output_dir = './reports'\n[metrics]\nexcluded_files = 42",
            "output_dir = './reports'\n[metrics]\nunknown = true",
        ] {
            assert!(Config::parse(source).is_err());
        }
    }

    #[test]
    fn accepts_optional_bitbucket_workspace_and_masks_token() {
        let config = Config::parse(
            "output_dir = './reports'\n[bitbucket]\nworkspace = 'team'\ntoken = 'secret-token'",
        )
        .unwrap();
        assert!(!format!("{config:?}").contains("secret-token"));
        assert!(Config::parse("output_dir = './reports'\n[bitbucket]").is_ok());
        assert!(Config::parse("output_dir = './reports'\n[bitbucket]\nunknown = true").is_err());
    }

    #[test]
    fn resolves_default_path() {
        let home = PathBuf::from("home");
        assert_eq!(
            config_path(None, Some(home.clone().into_os_string())).unwrap(),
            home.join(".config/dev-metrics/config.toml")
        );
    }

    #[test]
    fn explicit_path_overrides_home_and_works_without_it() {
        for home in [None, Some(OsString::from("other-home"))] {
            let path = PathBuf::from("custom.toml");
            assert_eq!(config_path(Some(path.clone()), home).unwrap(), path);
        }
    }

    #[test]
    fn rejects_missing_or_empty_home_without_explicit_path() {
        for home in [None, Some(OsString::new())] {
            assert!(config_path(None, home).is_err());
        }
    }

    #[test]
    fn loads_config_from_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "output_dir = 'weekly-notes'").unwrap();
        assert_eq!(
            Config::load(&path).unwrap().output_dir,
            Some(PathBuf::from("weekly-notes"))
        );
    }

    #[test]
    fn validates_configurable_values_without_exposing_input() {
        for (key, value) in [
            ("github.token", "bad token"),
            ("bitbucket.workspace", "bad/path"),
            ("bitbucket.username", ""),
            ("metrics.excluded_files", "*.snap,,generated/*"),
            ("output_dir", ""),
        ] {
            let error = validate_setting(key, value).unwrap_err();
            if !value.is_empty() {
                assert!(!error.contains(value));
            }
        }
        for (key, value) in [
            ("github.token", "fake-token"),
            ("bitbucket.workspace", "team_1"),
            ("metrics.excluded_files", ""),
        ] {
            assert!(validate_setting(key, value).is_ok());
        }
    }

    #[test]
    fn reports_path_for_missing_unreadable_invalid_and_non_utf8_files() {
        let directory = tempfile::tempdir().unwrap();
        let invalid = directory.path().join("invalid.toml");
        let non_utf8 = directory.path().join("non-utf8.toml");
        std::fs::write(&invalid, "output_dir = [").unwrap();
        std::fs::write(&non_utf8, [0xff]).unwrap();
        for path in [
            directory.path().join("missing.toml"),
            directory.path().to_path_buf(),
            invalid,
            non_utf8,
        ] {
            let message = Config::load(&path).unwrap_err().to_string();
            assert!(message.contains(&path.display().to_string()), "{message}");
        }
    }
}
