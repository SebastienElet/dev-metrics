use dev_metrics::activity::{Activity, Commit, FileChange, Period, PullRequest, Source};
use dev_metrics::metrics::{MetricsConfig, MetricsReport, calculate, calculate_period};
use serde_json::Value;

fn activity() -> Activity {
    let file = |lines, deletions| FileChange {
        path: "src/lib.rs".into(),
        previous_path: None,
        additions: lines,
        deletions,
    };
    let pr = |date: &str, lines| PullRequest {
        source: Source::GitHub,
        repository: "team/project".into(),
        number: lines,
        author: Some("alice".into()),
        title: String::new(),
        url: String::new(),
        created_at: date.parse().unwrap(),
        merged_at: None,
        first_review_received_at: None,
        additions: lines,
        deletions: 0,
        files: vec![file(lines, 0)],
    };
    let commit = |date: &str, lines, deletions| Commit {
        source: Source::GitHub,
        repository: "team/project".into(),
        sha: date.into(),
        message: String::new(),
        url: String::new(),
        authored_at: date.parse().unwrap(),
        additions: lines + 999,
        deletions,
        files: vec![
            file(lines, deletions),
            FileChange {
                path: "Cargo.lock".into(),
                ..file(999, 0)
            },
        ],
    };
    let mut previous = pr("2026-10-23T00:00:00Z", 20);
    previous.merged_at = Some("2026-10-24T00:00:00Z".parse().unwrap());
    previous.first_review_received_at = Some("2026-10-23T01:00:00Z".parse().unwrap());
    Activity {
        schema_version: 1,
        source: Source::GitHub,
        user: "alice".into(),
        period: Period::parse("2026-09-01", "2026-10-30").unwrap(),
        pull_requests: vec![pr("2026-10-30T23:59:59Z", 500), previous],
        commits: vec![
            commit("2026-10-30T23:59:59Z", 12, 2),
            commit("2026-10-23T00:00:00Z", 4, 4),
        ],
        reviews_given: vec![],
        reviews_received: vec![],
        comments: vec![],
    }
}

fn report() -> MetricsReport {
    calculate(
        &[activity()],
        "2026-10-30".parse().unwrap(),
        &MetricsConfig::default(),
    )
    .unwrap()
}

#[test]
fn compact_json_matches_versioned_snapshot() {
    let output = report().json().unwrap();
    assert_eq!(output, include_str!("fixtures/report-v1.json").trim_end());
    assert_eq!(output.lines().count(), 1);
}

#[test]
fn table_and_markdown_match_snapshots() {
    let report = report();
    assert_eq!(
        report.table(),
        include_str!("fixtures/report.table").trim_end()
    );
    assert_eq!(
        report.markdown(),
        include_str!("fixtures/report.md").trim_end()
    );
}

#[test]
fn empty_custom_report_matches_snapshots_and_schema() {
    let report = calculate_period(
        &[],
        Period::parse("2024-03-01", "2024-03-01").unwrap(),
        &MetricsConfig::default(),
    )
    .unwrap();
    assert_eq!(
        report.json().unwrap(),
        include_str!("fixtures/report-empty-v1.json").trim_end()
    );
    assert_eq!(
        report.table(),
        include_str!("fixtures/report-empty.table").trim_end()
    );
    assert_eq!(
        report.markdown(),
        include_str!("fixtures/report-empty.md").trim_end()
    );
    validator()
        .validate(&serde_json::from_str::<Value>(&report.json().unwrap()).unwrap())
        .unwrap();
}

fn validator() -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(dev_metrics::metrics::REPORT_SCHEMA).unwrap();
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema)
        .unwrap()
}

#[test]
fn schema_validates_real_output_and_rejects_broken_contracts() {
    let validator = validator();
    let output: Value = serde_json::from_str(&report().json().unwrap()).unwrap();
    validator.validate(&output).unwrap();
    for (pointer, invalid) in [
        ("/schema_version", serde_json::json!(2)),
        (
            "/windows/0/current_period/from",
            serde_json::json!("2026-02-30"),
        ),
        ("/windows/0/metrics/commits/current", Value::Null),
        (
            "/windows/0/metrics/lines_added/current",
            serde_json::json!(-1),
        ),
        (
            "/windows/0/metrics/large_pull_request_percent/current",
            serde_json::json!(101),
        ),
        ("/by_source/0/source", serde_json::json!("unknown")),
    ] {
        let mut broken = output.clone();
        *broken.pointer_mut(pointer).unwrap() = invalid;
        assert!(!validator.is_valid(&broken), "{pointer}");
    }
    let mut missing = output.clone();
    missing["windows"][0]["metrics"]
        .as_object_mut()
        .unwrap()
        .remove("reviews_received");
    assert!(!validator.is_valid(&missing));
    let mut extra = output;
    extra["unexpected"] = serde_json::json!(true);
    assert!(!validator.is_valid(&extra));
}
