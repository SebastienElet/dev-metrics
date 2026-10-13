mod cli;

use clap::Parser;
use cli::{Cli, Command, Format};
use dev_metrics::config::{Config, config_path};
use dev_metrics::github::{GitHubSource, Period, resolve_token};
use std::error::Error;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Erreur : {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let path = config_path(cli.config, std::env::var_os("HOME"))?;
    let config = Config::load(&path)?;
    let command = match cli.command {
        Command::Github {
            from,
            to,
            user,
            format,
        } => {
            let period = Period::parse(&from, &to)?;
            let configured = config
                .github
                .as_ref()
                .and_then(|github| github.token.as_deref());
            let token = resolve_token(std::ffi::OsStr::new("gh"), configured)?;
            let source = GitHubSource::new("https://api.github.com/".parse()?, &token)?;
            let user = match user {
                Some(user) => user,
                None => source.authenticated_user()?,
            };
            let activity = source.activity(&user, period)?;
            let output = match format {
                Format::Json => serde_json::to_string_pretty(&activity)?,
                Format::Table => activity.table(),
            };
            use std::io::Write;
            writeln!(std::io::stdout().lock(), "{output}")?;
            return Ok(());
        }
        Command::Report => "report",
        Command::Schedule => "schedule",
    };
    println!(
        "{command} : non implémenté (répertoire de sortie configuré : {}).",
        config.output_dir.display()
    );
    Ok(())
}
