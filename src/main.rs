mod cli;

use chrono::{DateTime, Datelike, Days, NaiveDate, Utc};
use clap::{Parser, error::ErrorKind};
use cli::error::{CommandError, ErrorCode};
use cli::{ActivityFormat, Cli, Command, ReportFormat, SourceKind};
use dev_metrics::activity::{ActivitySource, Period};
use dev_metrics::config::{Config, config_path};
use dev_metrics::metrics::REPORT_SCHEMA;
use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let result = match Cli::try_parse() {
        Ok(cli) => run(
            cli,
            DateTime::<Utc>::from(std::time::SystemTime::now()).date_naive(),
            load_source,
        ),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            Ok(error.to_string())
        }
        Err(error) => Err(CommandError::new(ErrorCode::InvalidArguments, error)),
    };
    let result = result.and_then(|output| write_output(&mut std::io::stdout().lock(), &output));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if let Ok(json) = serde_json::to_string(&error) {
                let _ = writeln!(std::io::stderr().lock(), "{json}");
            }
            ExitCode::from(error.exit_code())
        }
    }
}

fn write_output(writer: &mut impl Write, output: &str) -> Result<(), CommandError> {
    writeln!(writer, "{}", output.trim_end())
        .map_err(|_| CommandError::new(ErrorCode::OutputError, "Impossible d'écrire sur stdout"))
}

fn run(
    cli: Cli,
    today: NaiveDate,
    mut source_factory: impl FnMut(
        SourceKind,
        &Config,
        Option<&str>,
    ) -> Result<Box<dyn ActivitySource>, CommandError>,
) -> Result<String, CommandError> {
    if cli.command == Command::Schema {
        return Ok(REPORT_SCHEMA.into());
    }
    if let Command::Config { action } = cli.command {
        let path = config_path(cli.config, std::env::var_os("HOME"))
            .map_err(|error| CommandError::new(ErrorCode::ConfigurationError, error))?;
        return cli::config::run(action, &path);
    }
    let invalid_period = |error| CommandError::new(ErrorCode::InvalidPeriod, error);
    let period = match &cli.command {
        Command::Github { from, to, .. } | Command::Bitbucket { from, to, .. } => {
            Period::parse(from, to).map_err(invalid_period)?
        }
        Command::Report {
            since,
            until,
            source,
            github_user,
            bitbucket_user,
            workspace,
            ..
        } => {
            if (!source.contains(&SourceKind::Github) && github_user.is_some())
                || (!source.contains(&SourceKind::Bitbucket)
                    && (bitbucket_user.is_some() || workspace.is_some()))
            {
                return Err(CommandError::new(
                    ErrorCode::InvalidArguments,
                    "Les options d'identité/workspace nécessitent la source correspondante dans --source",
                ));
            }
            let end = until.clone().unwrap_or_else(|| today.to_string());
            let period =
                Period::parse(since.as_deref().unwrap_or(&end), &end).map_err(invalid_period)?;
            period.previous().map_err(invalid_period)?;
            if since.is_none()
                && period
                    .end()
                    .checked_sub_days(Days::new(59))
                    .is_none_or(|date| date.year() < 0)
            {
                return Err(CommandError::new(
                    ErrorCode::InvalidPeriod,
                    "La période précédente de 30 jours est hors limites",
                ));
            }
            period
        }
        Command::Schema | Command::Config { .. } => unreachable!(),
    };
    let path = config_path(cli.config, std::env::var_os("HOME"))
        .map_err(|error| CommandError::new(ErrorCode::ConfigurationError, error))?;
    let mut config = Config::load(&path)
        .map_err(|error| CommandError::new(ErrorCode::ConfigurationError, error))?;
    let config_error = |error| CommandError::new(ErrorCode::ConfigurationError, error);
    match &cli.command {
        Command::Github { .. } => config.apply_github_environment().map_err(config_error)?,
        Command::Bitbucket { .. } => config.apply_bitbucket_environment().map_err(config_error)?,
        Command::Report { source, .. } => {
            if source.contains(&SourceKind::Github) {
                config.apply_github_environment().map_err(config_error)?;
            }
            if source.contains(&SourceKind::Bitbucket) {
                config.apply_bitbucket_environment().map_err(config_error)?;
            }
        }
        Command::Config { .. } | Command::Schema => unreachable!(),
    }
    let (kind, user, workspace, format) = match cli.command {
        Command::Github { user, format, .. } => (SourceKind::Github, user, None, format),
        Command::Bitbucket {
            user,
            workspace,
            format,
            ..
        } => (SourceKind::Bitbucket, user, workspace, format),
        Command::Report {
            source,
            since,
            github_user,
            bitbucket_user,
            workspace,
            format,
            ..
        } => {
            let selected = [SourceKind::Github, SourceKind::Bitbucket]
                .into_iter()
                .filter(|kind| source.contains(kind))
                .collect::<Vec<_>>();
            let sources = selected
                .iter()
                .map(|kind| source_factory(*kind, &config, workspace.as_deref()))
                .collect::<Result<Vec<_>, _>>()?;
            let requests = sources
                .iter()
                .zip(&selected)
                .map(|(source, kind)| dev_metrics::metrics::SourceRequest {
                    source: source.as_ref(),
                    user: match kind {
                        SourceKind::Github => github_user.as_deref(),
                        SourceKind::Bitbucket => bitbucket_user.as_deref(),
                    },
                })
                .collect::<Vec<_>>();
            let report = if since.is_some() {
                dev_metrics::metrics::collect_period(&requests, period, &config.metrics)
            } else {
                dev_metrics::metrics::collect(&requests, period.end(), &config.metrics)
            }.map_err(|error| CommandError {
                code: ErrorCode::ReportError,
                message: error.to_string(),
                details: serde_json::json!({"sources": selected.iter().map(|source| source.as_str()).collect::<Vec<_>>()}),
            })?;
            return match format {
                ReportFormat::Json => report
                    .json()
                    .map_err(|error| CommandError::new(ErrorCode::OutputError, error)),
                ReportFormat::Table => Ok(report.table()),
                ReportFormat::Markdown => Ok(report.markdown()),
            };
        }
        Command::Schema | Command::Config { .. } => unreachable!(),
    };
    let source = source_factory(kind, &config, workspace.as_deref())?;
    let activity = source
        .collect_activity(user.as_deref(), period)
        .map_err(|error| {
            CommandError::new(ErrorCode::CollectionError, error).source(kind.as_str())
        })?;
    match format {
        ActivityFormat::Json => serde_json::to_string(&activity)
            .map_err(|error| CommandError::new(ErrorCode::OutputError, error)),
        ActivityFormat::Table => Ok(activity.table()),
    }
}

fn load_source(
    kind: SourceKind,
    config: &Config,
    workspace: Option<&str>,
) -> Result<Box<dyn ActivitySource>, CommandError> {
    let auth_error = |error: Box<dyn std::error::Error>| {
        CommandError::new(ErrorCode::AuthenticationError, error).source(kind.as_str())
    };
    match kind {
        SourceKind::Github => {
            let configured = config
                .github
                .as_ref()
                .and_then(|github| github.token.as_deref());
            let token = dev_metrics::github::resolve_token(std::ffi::OsStr::new("gh"), configured)
                .map_err(auth_error)?;
            dev_metrics::github::GitHubSource::new(
                "https://api.github.com/".parse().unwrap(),
                &token,
            )
            .map(|source| Box::new(source) as Box<dyn ActivitySource>)
            .map_err(auth_error)
        }
        SourceKind::Bitbucket => {
            let configured = config.bitbucket.as_ref();
            let workspace = dev_metrics::bitbucket::resolve_workspace(
                workspace,
                configured.and_then(|config| config.workspace.as_deref()),
            )
            .map_err(|error| {
                CommandError::new(ErrorCode::ConfigurationError, error).source("bitbucket")
            })?;
            let token = dev_metrics::bitbucket::resolve_token(
                std::env::var_os("DEV_METRICS_BITBUCKET_TOKEN")
                    .or_else(|| std::env::var_os("BITBUCKET_TOKEN")),
                configured.and_then(|config| config.token.as_deref()),
            )
            .map_err(auth_error)?;
            dev_metrics::bitbucket::BitbucketSource::new(
                "https://api.bitbucket.org/2.0/".parse().unwrap(),
                &token,
                workspace,
            )
            .map(|source| Box::new(source) as Box<dyn ActivitySource>)
            .map_err(auth_error)
        }
    }
}

#[cfg(test)]
mod command_tests;
