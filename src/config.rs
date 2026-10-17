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
    pub workspace: Option<String>,
    pub token: Option<String>,
}

impl std::fmt::Debug for BitbucketConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BitbucketConfig")
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
