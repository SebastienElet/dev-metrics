use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

pub mod config;
pub mod error;

pub const EXIT_HELP: &str = "Sorties : succès sur stdout ; erreurs JSON {code,message,details} sur stderr.\nCodes de sortie : 0 succès/aide ; 1 écriture ; 2 arguments/période ; 3 configuration ; 4 authentification/vérification des accès ; 5 collecte/calcul.\nAucun rapport partiel en cas d'échec de collecte ou de calcul.";

#[derive(Debug, Parser)]
#[command(name = "dev-metrics", version, about = "Métriques de développement pour un humain ou un bot LLM", after_help = format!("Exemples :\n  dev-metrics config init\n  dev-metrics report\n  dev-metrics report --format table\n  dev-metrics report --since 2026-10-01 --until 2026-10-15 --format markdown\n  dev-metrics schema\n  dev-metrics github --help\n  dev-metrics bitbucket --help\n  dev-metrics --config ./config.toml report\n\n{EXIT_HELP}"))]
pub struct Cli {
    #[arg(
        long,
        global = true,
        help = "Désactiver la progression et les logs, même avec --verbose"
    )]
    pub quiet: bool,
    #[arg(
        long,
        global = true,
        help = "Détailler les appels API, pages et limites sur stderr (TTY uniquement, sans secrets)"
    )]
    pub verbose: bool,
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Configuration TOML (défaut : $HOME/.config/dev-metrics/config.toml). Exemple : --config ./config.toml"
    )]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, PartialEq, Subcommand)]
pub enum Command {
    #[command(about = "Créer, lire et modifier la configuration", after_help = format!("Exemples :\n  dev-metrics config init\n  dev-metrics config init --non-interactive --github-token TOKEN --bitbucket-workspace team\n  dev-metrics config check --format json\n  dev-metrics config set bitbucket.workspace team\n  dev-metrics config get github.token --format json\n  dev-metrics config list --format json\n  dev-metrics config path\n\n{}{}\n{EXIT_HELP}", config::GITHUB_TOKEN_HELP, config::BITBUCKET_TOKEN_HELP))]
    Config {
        #[command(subcommand)]
        action: config::ConfigAction,
    },
    #[command(about = "Collecter l'activité brute GitHub, contrat activity v1", after_help = format!("Exemples :\n  dev-metrics github --from 2026-10-01 --to 2026-10-07\n  dev-metrics github --since 2026-10-01 --until 2026-10-07 --user octocat --format table\n  dev-metrics github --from 2026-10-01 --to 2026-10-07 --format json --config ./config.toml\n\n{EXIT_HELP}"))]
    Github {
        #[arg(
            long,
            visible_alias = "since",
            value_name = "YYYY-MM-DD",
            help = "Début inclusif UTC, date ISO 8601. Exemple : --from 2026-10-01"
        )]
        from: String,
        #[arg(
            long,
            visible_alias = "until",
            value_name = "YYYY-MM-DD",
            help = "Fin inclusive UTC, date ISO 8601. Exemple : --to 2026-10-07"
        )]
        to: String,
        #[arg(
            long,
            help = "Compte GitHub (défaut : compte authentifié). Exemple : --user octocat"
        )]
        user: Option<String>,
        #[arg(
            long,
            value_enum,
            default_value = "json",
            help = "Activité brute en JSON compact ou tableau. Exemple : --format table"
        )]
        format: ActivityFormat,
    },
    #[command(about = "Collecter l'activité brute Bitbucket Cloud, contrat activity v1", after_help = format!("Exemples :\n  dev-metrics bitbucket --workspace team --from 2026-10-01 --to 2026-10-07\n  dev-metrics bitbucket --since 2026-10-01 --until 2026-10-07 --user '{{uuid}}' --format table\n  dev-metrics bitbucket --from 2026-10-01 --to 2026-10-07 --format json --config ./config.toml\n\n{EXIT_HELP}"))]
    Bitbucket {
        #[arg(
            long,
            visible_alias = "since",
            value_name = "YYYY-MM-DD",
            help = "Début inclusif UTC. Exemple : --from 2026-10-01"
        )]
        from: String,
        #[arg(
            long,
            visible_alias = "until",
            value_name = "YYYY-MM-DD",
            help = "Fin inclusive UTC. Exemple : --to 2026-10-07"
        )]
        to: String,
        #[arg(
            long,
            help = "Workspace (défaut : configuration). Exemple : --workspace team"
        )]
        workspace: Option<String>,
        #[arg(
            long,
            help = "UUID Bitbucket ou Account ID Atlassian (défaut : identité configurée, puis compte authentifié). Exemple : --user '{uuid}'"
        )]
        user: Option<String>,
        #[arg(
            long,
            value_enum,
            default_value = "json",
            help = "Activité brute en JSON compact ou tableau. Exemple : --format table"
        )]
        format: ActivityFormat,
    },
    #[command(about = "Calculer les métriques et deltas : 7/30 jours ou période libre", after_help = format!("Exemples :\n  dev-metrics report\n  dev-metrics report --until 2026-10-30 --format json\n  dev-metrics report --source github,bitbucket --github-user octocat --bitbucket-user '{{uuid}}' --workspace team --format table\n  dev-metrics report --since 2026-10-01 --until 2026-10-15 --format markdown\n  dev-metrics report --config ./config.toml\n\nChaque période est comparée aux jours immédiatement précédents de même durée.\nSans --since : fenêtres de 7 et 30 jours (60 jours collectés).\nDates ISO 8601 YYYY-MM-DD, inclusives en UTC ; timestamps non acceptés.\nJSON/tableau : agrégats et répartitions par source/dépôt. Markdown : résumé global.\n\n{EXIT_HELP}"))]
    Report {
        #[arg(
            long,
            value_enum,
            value_delimiter = ',',
            default_value = "github",
            help = "Sources explicites, doublons ignorés. Exemple : --source github,bitbucket"
        )]
        source: Vec<SourceKind>,
        #[arg(
            long,
            value_name = "YYYY-MM-DD",
            help = "Début d'une période libre remplaçant 7/30 jours. Exemple : --since 2026-10-01"
        )]
        since: Option<String>,
        #[arg(
            long,
            value_name = "YYYY-MM-DD",
            help = "Fin inclusive (défaut : aujourd'hui UTC). Exemple : --until 2026-10-30"
        )]
        until: Option<String>,
        #[arg(
            long,
            help = "Compte GitHub (défaut : compte authentifié). Exemple : --github-user octocat"
        )]
        github_user: Option<String>,
        #[arg(
            long,
            help = "UUID/Account ID Bitbucket (défaut : identité configurée, puis compte authentifié). Exemple : --bitbucket-user '{uuid}'"
        )]
        bitbucket_user: Option<String>,
        #[arg(
            long,
            help = "Workspace Bitbucket (défaut : configuration). Exemple : --workspace team"
        )]
        workspace: Option<String>,
        #[arg(
            long,
            value_enum,
            default_value = "json",
            help = "JSON compact versionné, tableau complet ou résumé Markdown. Exemple : --format markdown"
        )]
        format: ReportFormat,
    },
    #[command(about = "Imprimer le JSON Schema du rapport, sans configuration ni réseau", after_help = format!("Exemple :\n  dev-metrics schema > report.schema.json\n\nLe schéma Draft 2020-12 décrit exclusivement le JSON de report (schema_version: 1).\nLes schémas de l'activité brute github/bitbucket sont disponibles dans schemas/ du dépôt.\n\n{EXIT_HELP}"))]
    Schema,
}

#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum ActivityFormat {
    Json,
    Table,
}

#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum ReportFormat {
    Json,
    Table,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum SourceKind {
    Github,
    Bitbucket,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Bitbucket => "bitbucket",
        }
    }
}

#[cfg(test)]
mod tests;
