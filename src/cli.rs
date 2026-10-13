use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "dev-metrics", version, about)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Fichier de configuration TOML"
    )]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, PartialEq, Subcommand)]
pub enum Command {
    #[command(about = "Collecter l'activité GitHub sur une période UTC")]
    Github {
        #[arg(long, value_name = "YYYY-MM-DD")]
        from: String,
        #[arg(long, value_name = "YYYY-MM-DD")]
        to: String,
        #[arg(long, help = "Utilisateur GitHub (par défaut : compte authentifié)")]
        user: Option<String>,
        #[arg(long, value_enum, default_value = "json")]
        format: Format,
    },
    #[command(about = "Collecter l'activité Bitbucket Cloud sur une période UTC")]
    Bitbucket {
        #[arg(long, value_name = "YYYY-MM-DD")]
        from: String,
        #[arg(long, value_name = "YYYY-MM-DD")]
        to: String,
        #[arg(long, help = "Workspace Bitbucket (par défaut : configuration)")]
        workspace: Option<String>,
        #[arg(
            long,
            help = "UUID Bitbucket ou Account ID Atlassian (par défaut : compte authentifié)"
        )]
        user: Option<String>,
        #[arg(long, value_enum, default_value = "json")]
        format: Format,
    },
    #[command(about = "Rapport de métriques (non implémenté)")]
    Report,
    #[command(about = "Planification des rapports (non implémentée)")]
    Schedule,
}

#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum Format {
    Json,
    Table,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_schedule() {
        assert!(Cli::try_parse_from(["dev-metrics", "schedule"]).is_ok());
    }

    #[test]
    fn parses_bitbucket_workspace_user_and_format() {
        for format in ["json", "table"] {
            assert!(
                Cli::try_parse_from([
                    "dev-metrics",
                    "bitbucket",
                    "--from",
                    "2026-10-01",
                    "--to",
                    "2026-10-07",
                    "--workspace",
                    "team",
                    "--user",
                    "alice",
                    "--format",
                    format
                ])
                .is_ok()
            );
        }
        assert!(
            Cli::try_parse_from([
                "dev-metrics",
                "bitbucket",
                "--from",
                "2026-10-01",
                "--to",
                "2026-10-07"
            ])
            .is_ok()
        );
    }

    #[test]
    fn parses_github_period_user_and_output_format() {
        for format in ["json", "table"] {
            assert!(
                Cli::try_parse_from([
                    "dev-metrics",
                    "github",
                    "--from",
                    "2026-10-01",
                    "--to",
                    "2026-10-07",
                    "--user",
                    "octocat",
                    "--format",
                    format
                ])
                .is_ok()
            );
        }
    }

    #[test]
    fn parses_report_with_config_before_or_after_subcommand() {
        for arguments in [
            ["dev-metrics", "--config", "custom.toml", "report"],
            ["dev-metrics", "report", "--config", "custom.toml"],
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert_eq!(cli.command, Command::Report);
            assert_eq!(cli.config, Some(PathBuf::from("custom.toml")));
        }
    }

    #[test]
    fn rejects_missing_or_unknown_subcommand_and_config_without_path() {
        for arguments in [
            vec!["dev-metrics"],
            vec!["dev-metrics", "unknown"],
            vec!["dev-metrics", "report", "--config"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }
}
