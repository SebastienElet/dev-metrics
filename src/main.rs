mod cli;
mod config;

use clap::Parser;
use cli::{Cli, Command};
use config::{Config, config_path};
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
        Command::Report => "report",
        Command::Schedule => "schedule",
    };
    println!(
        "{command} : non implémenté (répertoire de sortie configuré : {}).",
        config.output_dir.display()
    );
    Ok(())
}
