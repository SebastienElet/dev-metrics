mod cli;

use chrono::{DateTime, Datelike, Days, NaiveDate, Utc};
use clap::{Parser, error::ErrorKind};
use cli::config::{CheckFormat, ConfigAction};
use cli::error::{CommandError, ErrorCode};
use cli::{ActivityFormat, Cli, Command, ReportFormat, SourceKind};
use dev_metrics::activity::{ActivitySource, Period};
use dev_metrics::config::{Config, config_path, validate_setting};
use dev_metrics::metrics::REPORT_SCHEMA;
use dev_metrics::progress::{Event, Progress};
use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let started = Instant::now();
    let mut progress = Progress::default();
    let result = match Cli::try_parse() {
        Ok(cli) => {
            if matches!(
                cli.command,
                Command::Report { .. } | Command::Github { .. } | Command::Bitbucket { .. }
            ) {
                let stderr = std::io::stderr();
                let terminal = stderr.is_terminal();
                progress = Progress::new(stderr, terminal, cli.quiet, cli.verbose);
            }
            match &cli.command {
                Command::Config {
                    action: ConfigAction::Check { format },
                } => run_config_check(&cli, *format),
                _ => {
                    let cache = (!cli.no_cache)
                        .then(|| {
                            std::env::var_os("HOME").map(|home| {
                                std::path::PathBuf::from(home).join(".cache/dev-metrics")
                            })
                        })
                        .flatten();
                    run(
                        &progress,
                        cli,
                        DateTime::<Utc>::from(std::time::SystemTime::now()).date_naive(),
                        |kind, config, workspace| {
                            load_source(kind, config, workspace, &progress, cache.as_deref())
                        },
                    )
                    .map(|output| (output, 0))
                }
            }
        }
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            Ok((error.to_string(), 0))
        }
        Err(error) => Err(CommandError::new(ErrorCode::InvalidArguments, error)),
    };
    let result = result.and_then(|(output, code)| {
        write_output(&mut std::io::stdout().lock(), &output).map(|()| code)
    });
    progress.emit(Event::Finished {
        elapsed: started.elapsed(),
        success: result.as_ref().is_ok_and(|code| *code == 0),
    });
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            if let Ok(json) = serde_json::to_string(&error) {
                let _ = writeln!(std::io::stderr().lock(), "{json}");
            }
            ExitCode::from(error.exit_code())
        }
    }
}

fn run_config_check(cli: &Cli, format: CheckFormat) -> Result<(String, u8), CommandError> {
    let path = config_path(cli.config.clone(), std::env::var_os("HOME"))
        .map_err(|error| CommandError::new(ErrorCode::ConfigurationError, error))?;
    let report = cli::config::check::run(&path)?;
    let code = if report.success() { 0 } else { 4 };
    Ok((report.render(format)?, code))
}

fn write_output(writer: &mut impl Write, output: &str) -> Result<(), CommandError> {
    writeln!(writer, "{}", output.trim_end())
        .map_err(|_| CommandError::new(ErrorCode::OutputError, "Impossible d'écrire sur stdout"))
}

fn run(
    progress: &Progress,
    cli: Cli,
    today: NaiveDate,
    source_factory: impl FnMut(
        SourceKind,
        &Config,
        Option<&str>,
    ) -> Result<Box<dyn ActivitySource>, CommandError>,
) -> Result<String, CommandError> {
    run_with_warnings(
        progress,
        cli,
        today,
        source_factory,
        &mut std::io::stderr().lock(),
    )
}

fn run_with_warnings(
    progress: &Progress,
    cli: Cli,
    today: NaiveDate,
    mut source_factory: impl FnMut(
        SourceKind,
        &Config,
        Option<&str>,
    ) -> Result<Box<dyn ActivitySource>, CommandError>,
    warnings_writer: &mut impl Write,
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
        Command::Bitbucket { user, .. } => config
            .apply_bitbucket_environment(user.as_deref())
            .map_err(config_error)?,
        Command::Report {
            source,
            bitbucket_user,
            ..
        } => {
            if source.contains(&SourceKind::Github) {
                config.apply_github_environment().map_err(config_error)?;
            }
            if source.contains(&SourceKind::Bitbucket) {
                config
                    .apply_bitbucket_environment(bitbucket_user.as_deref())
                    .map_err(config_error)?;
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
            let configured_bitbucket_user =
                if selected.contains(&SourceKind::Bitbucket) && bitbucket_user.is_none() {
                    configured_bitbucket_username(&config)?
                } else {
                    None
                };
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
                        SourceKind::Bitbucket => {
                            bitbucket_user.as_deref().or(configured_bitbucket_user)
                        }
                    },
                })
                .collect::<Vec<_>>();
            let report = if since.is_some() {
                dev_metrics::metrics::collect_period_with_progress(&requests, period, &config.metrics, progress)
            } else {
                dev_metrics::metrics::collect_with_progress(&requests, period.end(), &config.metrics, progress)
            }.map_err(|error| CommandError {
                code: ErrorCode::ReportError,
                message: error.to_string(),
                details: serde_json::json!({"sources": selected.iter().map(|source| source.as_str()).collect::<Vec<_>>()}),
            })?;
            if !matches!(format, ReportFormat::Json) {
                write_warnings(warnings_writer, &report.warnings)?;
            }
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
    let configured_user = if kind == SourceKind::Bitbucket && user.is_none() {
        configured_bitbucket_username(&config)?
    } else {
        None
    };
    let source = source_factory(kind, &config, workspace.as_deref())?;
    let activity = source
        .collect_activity(user.as_deref().or(configured_user), period)
        .map_err(|error| {
            CommandError::new(ErrorCode::CollectionError, error).source(kind.as_str())
        })?;
    match format {
        ActivityFormat::Json => serde_json::to_string(&activity)
            .map_err(|error| CommandError::new(ErrorCode::OutputError, error)),
        ActivityFormat::Table => {
            write_warnings(warnings_writer, &activity.warnings)?;
            Ok(activity.table())
        }
    }
}

fn write_warnings(
    writer: &mut impl Write,
    warnings: &[dev_metrics::activity::ActivityWarning],
) -> Result<(), CommandError> {
    for warning in warnings {
        serde_json::to_writer(&mut *writer, warning).map_err(|_| {
            CommandError::new(
                ErrorCode::OutputError,
                "Impossible d'écrire les avertissements sur stderr",
            )
        })?;
        writeln!(writer).map_err(|_| {
            CommandError::new(
                ErrorCode::OutputError,
                "Impossible d'écrire les avertissements sur stderr",
            )
        })?;
    }
    Ok(())
}

fn configured_bitbucket_username(config: &Config) -> Result<Option<&str>, CommandError> {
    let username = config
        .bitbucket
        .as_ref()
        .and_then(|bitbucket| bitbucket.username.as_deref());
    if let Some(username) = username {
        validate_setting("bitbucket.username", username)
            .map_err(|error| CommandError::new(ErrorCode::ConfigurationError, error))?;
    }
    Ok(username)
}

fn load_source(
    kind: SourceKind,
    config: &Config,
    workspace: Option<&str>,
    progress: &Progress,
    cache: Option<&std::path::Path>,
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
            .map(|source| {
                let source = match cache {
                    Some(directory) => source.with_cache(directory.into()),
                    None => source,
                };
                Box::new(source.with_progress(progress.clone())) as Box<dyn ActivitySource>
            })
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
            let api_base = "https://api.bitbucket.org/2.0/".parse().unwrap();
            let source = match configured.and_then(|config| config.email.as_deref()) {
                Some(email) => dev_metrics::bitbucket::BitbucketSource::new_with_email(
                    api_base, &token, email, workspace,
                ),
                None => dev_metrics::bitbucket::BitbucketSource::new(api_base, &token, workspace),
            };
            source
                .map(|source| {
                    Box::new(source.with_progress(progress.clone())) as Box<dyn ActivitySource>
                })
                .map_err(auth_error)
        }
    }
}

#[cfg(test)]
mod command_tests;
