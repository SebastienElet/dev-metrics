use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(version, about)]
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
    #[command(about = "Rapport de métriques (non implémenté)")]
    Report,
    #[command(about = "Planification des rapports (non implémentée)")]
    Schedule,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_schedule() {
        assert!(Cli::try_parse_from(["devmetrics", "schedule"]).is_ok());
    }

    #[test]
    fn parses_report_with_config_before_or_after_subcommand() {
        for arguments in [
            ["devmetrics", "--config", "custom.toml", "report"],
            ["devmetrics", "report", "--config", "custom.toml"],
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert_eq!(cli.command, Command::Report);
            assert_eq!(cli.config, Some(PathBuf::from("custom.toml")));
        }
    }

    #[test]
    fn rejects_missing_or_unknown_subcommand_and_config_without_path() {
        for arguments in [
            vec!["devmetrics"],
            vec!["devmetrics", "unknown"],
            vec!["devmetrics", "report", "--config"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }
}
