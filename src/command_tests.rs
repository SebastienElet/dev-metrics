use super::*;
use cli::ReportFormat;
use dev_metrics::activity::{Activity, Commit, FileChange, Source};
use serde_json::{Value, json};
use std::error::Error;

struct FixedSource {
    source: Source,
    user: Option<&'static str>,
    collection_from: &'static str,
    failure: bool,
    overflow: bool,
}

impl ActivitySource for FixedSource {
    fn collect_activity(
        &self,
        user: Option<&str>,
        period: Period,
    ) -> Result<Activity, Box<dyn Error>> {
        assert_eq!(user, self.user);
        assert_eq!(
            serde_json::to_value(&period).unwrap()["from"],
            self.collection_from
        );
        if self.failure {
            return Err("collecte interrompue".into());
        }
        Ok(Activity {
            schema_version: 1,
            warnings: vec![],
            source: self.source,
            user: "alice".into(),
            period,
            pull_requests: vec![],
            reviews_given: vec![],
            reviews_received: vec![],
            comments: vec![],
            commits: vec![Commit {
                source: self.source,
                repository: "team/project".into(),
                sha: "sha".into(),
                message: String::new(),
                url: String::new(),
                authored_at: "2026-10-30T12:00:00Z".parse().unwrap(),
                additions: if self.overflow { u64::MAX } else { 7 },
                deletions: 2,
                files: vec![
                    FileChange {
                        path: "src/lib.rs".into(),
                        previous_path: None,
                        additions: if self.overflow { u64::MAX } else { 5 },
                        deletions: 1,
                    },
                    FileChange {
                        path: "other.rs".into(),
                        previous_path: None,
                        additions: 2,
                        deletions: 1,
                    },
                ],
            }],
        })
    }
}

fn cli(arguments: &[&str], config: &str) -> (tempfile::TempDir, Cli) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, config).unwrap();
    let mut cli =
        Cli::try_parse_from(std::iter::once("dev-metrics").chain(arguments.iter().copied()))
            .unwrap();
    cli.config = Some(path);
    (directory, cli)
}

fn today() -> NaiveDate {
    "2026-10-30".parse().unwrap()
}

fn fixed(source: SourceKind) -> FixedSource {
    FixedSource {
        source: match source {
            SourceKind::Github => Source::GitHub,
            SourceKind::Bitbucket => Source::Bitbucket,
        },
        user: None,
        collection_from: "2026-09-01",
        failure: false,
        overflow: false,
    }
}

#[test]
fn report_defaults_to_compact_json_and_rolling_windows_using_configured_exclusions() {
    let (_directory, cli) = cli(
        &["report"],
        "output_dir='unused'\n[metrics]\nexcluded_files=['other.rs']",
    );
    assert!(matches!(
        cli.command,
        Command::Report {
            format: ReportFormat::Json,
            ..
        }
    ));
    let output = run(
        &Progress::default(),
        cli,
        today(),
        |source, _, workspace| {
            assert_eq!(source, SourceKind::Github);
            assert_eq!(workspace, None);
            Ok(Box::new(fixed(source)))
        },
    )
    .unwrap();
    let mut stdout = Vec::new();
    write_output(&mut stdout, &output).unwrap();
    assert_eq!(stdout.iter().filter(|byte| **byte == b'\n').count(), 1);
    let value: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["windows"][0]["days"], 7);
    assert_eq!(value["windows"][1]["days"], 30);
    assert_eq!(
        value["windows"][0]["metrics"]["lines_added"]["current"],
        5.0
    );
    let schema: Value = serde_json::from_str(REPORT_SCHEMA).unwrap();
    jsonschema::validator_for(&schema)
        .unwrap()
        .validate(&value)
        .unwrap();
}

#[test]
fn report_collects_each_source_once_and_compares_the_custom_period() {
    let (_directory, cli) = cli(
        &[
            "report",
            "--source",
            "bitbucket,github,github",
            "--github-user",
            "alice",
            "--bitbucket-user",
            "{alice}",
            "--workspace",
            "team",
            "--since",
            "2026-10-28",
            "--until",
            "2026-10-30",
        ],
        "output_dir='unused'",
    );
    let mut requested = vec![];
    let output = run(
        &Progress::default(),
        cli,
        today(),
        |source, _, workspace| {
            requested.push(source);
            assert_eq!(workspace, Some("team"));
            Ok(Box::new(FixedSource {
                user: Some(if source == SourceKind::Github {
                    "alice"
                } else {
                    "{alice}"
                }),
                collection_from: "2026-10-25",
                ..fixed(source)
            }))
        },
    )
    .unwrap();
    assert_eq!(requested, [SourceKind::Github, SourceKind::Bitbucket]);
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["windows"].as_array().unwrap().len(), 1);
    assert_eq!(value["windows"][0]["days"], 3);
    assert_eq!(value["windows"][0]["metrics"]["commits"]["current"], 2.0);
    assert_eq!(value["by_source"][0]["source"], "github");
    assert_eq!(value["by_source"][1]["source"], "bitbucket");
}

#[test]
fn bitbucket_collection_uses_configured_username_unless_cli_user_is_explicit() {
    for (arguments, expected) in [
        (
            vec!["bitbucket", "--from", "2026-10-01", "--to", "2026-10-30"],
            "configured-user",
        ),
        (
            vec![
                "bitbucket",
                "--from",
                "2026-10-01",
                "--to",
                "2026-10-30",
                "--user",
                "explicit-user",
            ],
            "explicit-user",
        ),
    ] {
        let (_directory, cli) = cli(&arguments, "[bitbucket]\nusername='configured-user'\n");
        let output = run(&Progress::default(), cli, today(), |source, _, _| {
            Ok(Box::new(FixedSource {
                user: Some(if expected == "configured-user" {
                    "configured-user"
                } else {
                    "explicit-user"
                }),
                collection_from: "2026-10-01",
                ..fixed(source)
            }))
        });
        assert!(output.is_ok(), "{output:?}");
    }
    let (_directory, cli) = cli(
        &[
            "report",
            "--source",
            "bitbucket",
            "--since",
            "2026-10-28",
            "--until",
            "2026-10-30",
        ],
        "[bitbucket]\nusername='configured-user'\n",
    );
    let output = run(&Progress::default(), cli, today(), |source, _, _| {
        Ok(Box::new(FixedSource {
            user: Some("configured-user"),
            collection_from: "2026-10-25",
            ..fixed(source)
        }))
    });
    assert!(output.is_ok(), "{output:?}");
}

#[test]
fn invalid_configured_bitbucket_username_fails_before_source_creation() {
    let (_directory, cli) = cli(
        &["bitbucket", "--from", "2026-10-01", "--to", "2026-10-30"],
        "[bitbucket]\nusername='bad/path'\n",
    );
    let error = run(&Progress::default(), cli, today(), |_, _, _| {
        panic!("source should not be created")
    })
    .unwrap_err();
    assert_eq!(error.exit_code(), 3);
    assert_eq!(
        serde_json::to_value(error).unwrap()["code"],
        "CONFIGURATION_ERROR"
    );
}

#[test]
fn bitbucket_environment_username_reaches_collection() {
    if std::env::var_os("DEV_METRICS_TEST_USERNAME_CHILD").is_some() {
        let (_directory, cli) = cli(
            &["bitbucket", "--from", "2026-10-01", "--to", "2026-10-30"],
            "[bitbucket]\nusername='file-user'\n",
        );
        let result = run(&Progress::default(), cli, today(), |source, config, _| {
            assert_eq!(
                config.bitbucket.as_ref().unwrap().username.as_deref(),
                Some("env-user")
            );
            Ok(Box::new(FixedSource {
                user: Some("env-user"),
                collection_from: "2026-10-01",
                ..fixed(source)
            }))
        });
        assert!(result.is_ok(), "{result:?}");
        return;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "command_tests::bitbucket_environment_username_reaches_collection",
        ])
        .env("DEV_METRICS_TEST_USERNAME_CHILD", "1")
        .env("DEV_METRICS_BITBUCKET_USERNAME", "env-user")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn explicit_bitbucket_user_overrides_invalid_environment_username() {
    if std::env::var_os("DEV_METRICS_TEST_EXPLICIT_USER_CHILD").is_some() {
        for (arguments, collection_from) in [
            (
                vec![
                    "bitbucket",
                    "--from",
                    "2026-10-01",
                    "--to",
                    "2026-10-30",
                    "--user",
                    "explicit-user",
                ],
                "2026-10-01",
            ),
            (
                vec![
                    "report",
                    "--source",
                    "bitbucket",
                    "--since",
                    "2026-10-28",
                    "--until",
                    "2026-10-30",
                    "--bitbucket-user",
                    "explicit-user",
                ],
                "2026-10-25",
            ),
        ] {
            let (_directory, cli) = cli(&arguments, "[bitbucket]\nusername='file-user'\n");
            let result = run(&Progress::default(), cli, today(), |source, _, _| {
                Ok(Box::new(FixedSource {
                    user: Some("explicit-user"),
                    collection_from,
                    ..fixed(source)
                }))
            });
            assert!(result.is_ok(), "{result:?}");
        }
        return;
    }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "command_tests::explicit_bitbucket_user_overrides_invalid_environment_username",
        ])
        .env("DEV_METRICS_TEST_EXPLICIT_USER_CHILD", "1")
        .env("DEV_METRICS_BITBUCKET_USERNAME", "bad/path")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn report_selects_human_formats_and_honors_an_explicit_end_date() {
    for (format, expected) in [
        ("table", "Ensemble\n7 j :"),
        ("markdown", "## Métriques de développement"),
    ] {
        let (_directory, cli) = cli(
            &["report", "--until", "2026-10-30", "--format", format],
            "output_dir='unused'",
        );
        let output = run(
            &Progress::default(),
            cli,
            "2027-01-01".parse().unwrap(),
            |source, _, _| Ok(Box::new(fixed(source))),
        )
        .unwrap();
        assert!(output.starts_with(expected), "{output}");
        assert!(output.contains("2026-10-24 → 2026-10-30"));
    }
}

#[test]
fn report_errors_propagate_without_a_partial_result() {
    for (failure, overflow) in [(true, false), (false, true)] {
        let (_directory, cli) = cli(
            &["report", "--source", "github,bitbucket"],
            "output_dir='unused'",
        );
        let error = run(&Progress::default(), cli, today(), |source, _, _| {
            Ok(Box::new(FixedSource {
                failure: source == SourceKind::Bitbucket && failure,
                overflow: source == SourceKind::Bitbucket && overflow,
                ..fixed(source)
            }))
        })
        .unwrap_err();
        assert_eq!(error.exit_code(), 5);
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["code"], "REPORT_ERROR");
        assert_eq!(value["details"]["sources"], json!(["github", "bitbucket"]));
    }
}

#[test]
fn raw_activity_json_is_compact_and_preserves_its_contract() {
    let (_directory, cli) = cli(
        &["github", "--since", "2026-10-01", "--until", "2026-10-30"],
        "output_dir='unused'",
    );
    let output = run(&Progress::default(), cli, today(), |source, _, _| {
        Ok(Box::new(FixedSource {
            collection_from: "2026-10-01",
            ..fixed(source)
        }))
    })
    .unwrap();
    assert_eq!(output.lines().count(), 1);
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["source"], "github");
    assert_eq!(value["commits"][0]["additions"], 7);
}

#[test]
fn stdout_failure_has_a_stable_structured_error() {
    struct BrokenOutput;
    impl Write for BrokenOutput {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let error = write_output(&mut BrokenOutput, "{}").unwrap_err();
    assert_eq!(error.exit_code(), 1);
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({"code":"OUTPUT_ERROR","message":"Impossible d'écrire sur stdout","details":{}})
    );
}

#[test]
fn raw_collection_failure_has_source_details_and_exit_five() {
    let (_directory, cli) = cli(
        &["github", "--from", "2026-09-01", "--to", "2026-10-30"],
        "",
    );
    let error = run(&Progress::default(), cli, today(), |source, _, _| {
        Ok(Box::new(FixedSource {
            failure: true,
            ..fixed(source)
        }))
    })
    .unwrap_err();
    assert_eq!(error.exit_code(), 5);
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({
            "code": "COLLECTION_ERROR", "message": "collecte interrompue", "details": {"source":"github"}
        })
    );
}

#[test]
fn custom_period_defaults_to_today_and_schema_command_validates_its_output() {
    let (_directory, cli) = cli(&["report", "--since", "2026-10-28"], "");
    let output = run(&Progress::default(), cli, today(), |source, _, _| {
        Ok(Box::new(FixedSource {
            collection_from: "2026-10-25",
            ..fixed(source)
        }))
    })
    .unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["windows"][0]["current_period"]["to"], "2026-10-30");
    let schema = run(
        &Progress::default(),
        Cli::try_parse_from(["dev-metrics", "schema"]).unwrap(),
        today(),
        |_, _, _| panic!("schema ne collecte rien"),
    )
    .unwrap();
    let schema: Value = serde_json::from_str(&schema).unwrap();
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema)
        .unwrap()
        .validate(&value)
        .unwrap();
}

struct WarningSource;

impl ActivitySource for WarningSource {
    fn collect_activity(
        &self,
        user: Option<&str>,
        period: Period,
    ) -> Result<Activity, Box<dyn Error>> {
        let mut activity = fixed(SourceKind::Github).collect_activity(user, period)?;
        activity
            .warnings
            .push(dev_metrics::activity::ActivityWarning {
                source: Source::GitHub,
                repository: "team/project".into(),
                subject: dev_metrics::activity::WarningSubject::Commit {
                    sha: "abc123".into(),
                },
                code: dev_metrics::activity::WarningCode::FileStatisticsMismatch,
                message: "Totaux API +100/-20, détail disponible +7/-2".into(),
            });
        Ok(activity)
    }
}

#[test]
fn warnings_reach_json_or_stderr_without_polluting_table_and_markdown() {
    for format in ["table", "markdown", "json"] {
        let (_directory, cli) = cli(&["report", "--format", format], "");
        let mut stderr = Vec::new();
        let stdout = run_with_warnings(
            &Progress::default(),
            cli,
            today(),
            |_, _, _| Ok(Box::new(WarningSource)),
            &mut stderr,
        )
        .unwrap();
        let warning = if format == "json" {
            assert!(stderr.is_empty());
            let value: Value = serde_json::from_str(&stdout).unwrap();
            value["warnings"][0].clone()
        } else {
            assert!(!stdout.contains("FILE_STATISTICS_MISMATCH"));
            serde_json::from_slice::<Value>(&stderr).expect("structured stderr warning")
        };
        assert_eq!(warning["source"], "github");
        assert_eq!(warning["repository"], "team/project");
        assert_eq!(warning["subject"], json!({"kind":"commit","sha":"abc123"}));
        assert_eq!(warning["code"], "FILE_STATISTICS_MISMATCH");
    }
}

#[test]
fn warning_write_failures_remain_output_errors() {
    let (_directory, cli) = cli(&["report", "--format", "table"], "");
    let error = run_with_warnings(
        &Progress::default(),
        cli,
        today(),
        |_, _, _| Ok(Box::new(WarningSource)),
        &mut std::io::Cursor::new([]),
    )
    .unwrap_err();
    assert_eq!(serde_json::to_value(error).unwrap()["code"], "OUTPUT_ERROR");
}

#[test]
fn raw_activity_table_emits_warnings_on_stderr() {
    let (_directory, cli) = cli(
        &[
            "github",
            "--from",
            "2026-09-01",
            "--to",
            "2026-10-30",
            "--format",
            "table",
        ],
        "",
    );
    let mut stderr = Vec::new();
    let output = run_with_warnings(
        &Progress::default(),
        cli,
        today(),
        |_, _, _| Ok(Box::new(WarningSource)),
        &mut stderr,
    )
    .unwrap();
    assert!(output.starts_with("GitHub"));
    assert!(!output.contains("FILE_STATISTICS_MISMATCH"));
    let warning: Value = serde_json::from_slice(&stderr).unwrap();
    assert_eq!(warning["repository"], "team/project");
    assert_eq!(warning["subject"], json!({"kind":"commit","sha":"abc123"}));
}

#[test]
fn report_progress_preserves_stdout_for_every_output_mode() {
    for format in ["json", "table", "markdown"] {
        let mut baseline = None;
        for (terminal, quiet, verbose) in [
            (false, false, false),
            (true, false, false),
            (true, false, true),
            (false, false, true),
            (true, true, true),
        ] {
            let (_directory, arguments) = cli(&["report", "--format", format], "");
            let log = tempfile::NamedTempFile::new().unwrap();
            let progress = Progress::new(log.reopen().unwrap(), terminal, quiet, verbose);
            let output = run(&progress, arguments, today(), |kind, _, _| {
                Ok(Box::new(fixed(kind)))
            })
            .unwrap();
            let mut stdout = Vec::new();
            write_output(&mut stdout, &output).unwrap();
            if let Some(expected) = &baseline {
                assert_eq!(&stdout, expected);
            } else {
                baseline = Some(stdout.clone());
            }
            let stderr = std::fs::read_to_string(log.path()).unwrap();
            if terminal && !quiet {
                assert!(stderr.contains("Calcul des métriques"), "{stderr}");
            } else {
                assert!(stderr.is_empty());
            }
            if format == "json" {
                serde_json::from_slice::<Value>(&stdout).unwrap();
                assert!(!stdout.contains(&0x1b));
            }
        }
    }
}

#[test]
fn failed_collection_never_announces_metric_calculation() {
    let (_directory, arguments) = cli(&["report"], "");
    let log = tempfile::NamedTempFile::new().unwrap();
    let progress = Progress::new(log.reopen().unwrap(), true, false, true);
    assert!(
        run(&progress, arguments, today(), |kind, _, _| Ok(Box::new(
            FixedSource {
                failure: true,
                ..fixed(kind)
            }
        )))
        .is_err()
    );
    assert!(std::fs::read_to_string(log.path()).unwrap().is_empty());
}

#[test]
fn partial_statistics_warnings_survive_quiet_and_non_tty_progress() {
    for (command, format) in [
        ("report", "json"),
        ("report", "table"),
        ("report", "markdown"),
        ("github", "json"),
        ("github", "table"),
    ] {
        let mut baseline = None;
        for (terminal, options) in [
            (true, vec![]),
            (true, vec!["--verbose"]),
            (true, vec!["--quiet", "--verbose"]),
            (false, vec!["--verbose"]),
        ] {
            let mut arguments = vec![command, "--format", format];
            if command == "github" {
                arguments.extend(["--from", "2026-09-01", "--to", "2026-10-30"]);
            }
            arguments.extend(options);
            let (_directory, cli) = cli(&arguments, "");
            let quiet = cli.quiet;
            let log = tempfile::NamedTempFile::new().unwrap();
            let progress = Progress::new(
                log.as_file().try_clone().unwrap(),
                terminal,
                cli.quiet,
                cli.verbose,
            );
            let stdout = run_with_warnings(
                &progress,
                cli,
                today(),
                |_, _, _| Ok(Box::new(WarningSource)),
                &mut log.as_file().try_clone().unwrap(),
            )
            .unwrap();
            if let Some(expected) = &baseline {
                assert_eq!(&stdout, expected);
            } else {
                baseline = Some(stdout.clone());
            }
            let stderr = std::fs::read_to_string(log.path()).unwrap();
            let warning_lines = stderr
                .lines()
                .filter(|line| line.starts_with('{'))
                .collect::<Vec<_>>();
            let warning = if format == "json" {
                assert!(warning_lines.is_empty());
                serde_json::from_str::<Value>(&stdout).unwrap()["warnings"][0].clone()
            } else {
                assert_eq!(warning_lines.len(), 1, "{stderr}");
                assert!(!stdout.contains("FILE_STATISTICS_MISMATCH"));
                serde_json::from_str::<Value>(warning_lines[0]).unwrap()
            };
            assert_eq!(warning["code"], "FILE_STATISTICS_MISMATCH");
            if quiet || !terminal {
                assert!(stderr.lines().all(|line| line.starts_with('{')), "{stderr}");
            } else if command == "report" {
                assert!(stderr.contains("Calcul des métriques"), "{stderr}");
            }
        }
    }
}
