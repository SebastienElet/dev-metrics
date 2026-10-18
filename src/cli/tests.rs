use super::*;

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
                format,
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
            "2026-10-07",
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
                format,
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
        assert!(matches!(cli.command, Command::Report { .. }));
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

#[test]
fn accepts_no_cache_before_or_after_report() {
    for args in [
        ["dev-metrics", "--no-cache", "report"],
        ["dev-metrics", "report", "--no-cache"],
    ] {
        assert!(Cli::try_parse_from(args).is_ok());
    }
}
