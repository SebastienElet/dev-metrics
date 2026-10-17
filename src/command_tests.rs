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
                additions: 5,
                deletions: 1,
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
    let output = run(cli, today(), |source, _, workspace| {
        assert_eq!(source, SourceKind::Github);
        assert_eq!(workspace, None);
        Ok(Box::new(fixed(source)))
    })
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
    let output = run(cli, today(), |source, _, workspace| {
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
    })
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
fn report_selects_human_formats_and_honors_an_explicit_end_date() {
    for (format, expected) in [
        ("table", "Ensemble\n7 j :"),
        ("markdown", "## Métriques de développement"),
    ] {
        let (_directory, cli) = cli(
            &["report", "--until", "2026-10-30", "--format", format],
            "output_dir='unused'",
        );
        let output = run(cli, "2027-01-01".parse().unwrap(), |source, _, _| {
            Ok(Box::new(fixed(source)))
        })
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
        let error = run(cli, today(), |source, _, _| {
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
    let output = run(cli, today(), |source, _, _| {
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
    assert_eq!(value["commits"][0]["additions"], 5);
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
    let error = run(cli, today(), |source, _, _| {
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
    let output = run(cli, today(), |source, _, _| {
        Ok(Box::new(FixedSource {
            collection_from: "2026-10-25",
            ..fixed(source)
        }))
    })
    .unwrap();
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["windows"][0]["current_period"]["to"], "2026-10-30");
    let schema = run(
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
