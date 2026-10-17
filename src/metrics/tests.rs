use super::*;
use crate::activity::{Commit, FileChange, PullRequest};
use chrono::{DateTime, Utc};

fn timestamp(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn through() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 30).unwrap()
}

#[test]
fn custom_period_compares_equal_inclusive_windows_without_recalculating_metrics() {
    let mut data = activity(Source::GitHub, "alice");
    data.commits = vec![
        commit("2026-10-24T23:59:59Z", 900, 900),
        commit("2026-10-25T00:00:00Z", 10, 4),
        commit("2026-10-27T23:59:59Z", 20, 2),
        commit("2026-10-28T00:00:00Z", 12, 1),
        commit("2026-10-30T23:59:59Z", 3, 2),
        commit("2026-10-31T00:00:00Z", 900, 900),
    ];
    let report = calculate_period(
        &[data],
        Period::parse("2026-10-28", "2026-10-30").unwrap(),
        &MetricsConfig::default(),
    )
    .unwrap();
    assert_eq!(report.windows.len(), 1);
    let window = &report.windows[0];
    assert_eq!(window.days, 3);
    assert_eq!(window.previous_period.from.to_string(), "2026-10-25");
    assert_eq!(window.previous_period.to.to_string(), "2026-10-27");
    assert_comparison(&window.metrics.lines_added, 15.0, 30.0, -15.0, Some(-50.0));
    assert_eq!(report.by_source[0].windows.len(), 1);
    assert_eq!(report.by_repository[0].windows.len(), 1);
}

fn activity(source: Source, user: &str) -> Activity {
    Activity::empty(
        source,
        user,
        Period::parse("2026-09-01", "2026-10-30").unwrap(),
    )
}

fn file(path: &str, additions: u64, deletions: u64) -> FileChange {
    FileChange {
        path: path.into(),
        previous_path: None,
        additions,
        deletions,
    }
}

fn pull_request(date: &str, lines: u64) -> PullRequest {
    PullRequest {
        source: Source::GitHub,
        repository: "team/project".into(),
        number: 1,
        author: Some("alice".into()),
        title: String::new(),
        url: String::new(),
        created_at: timestamp(date),
        merged_at: None,
        first_review_received_at: None,
        additions: lines,
        deletions: 0,
        files: vec![file("src/lib.rs", lines, 0)],
    }
}

fn commit(date: &str, additions: u64, deletions: u64) -> Commit {
    Commit {
        source: Source::GitHub,
        repository: "team/project".into(),
        sha: date.into(),
        message: String::new(),
        url: String::new(),
        authored_at: timestamp(date),
        additions,
        deletions,
        files: vec![file("src/lib.rs", additions, deletions)],
    }
}

fn assert_comparison(
    value: &Comparison,
    current: f64,
    previous: f64,
    delta: f64,
    percent: Option<f64>,
) {
    assert_eq!(
        value,
        &Comparison {
            current: Some(current),
            previous: Some(previous),
            delta: Some(delta),
            delta_percent: percent
        }
    );
}

#[test]
fn volume_and_deltas_use_event_dates_and_commit_lines_without_double_counting() {
    let mut data = activity(Source::GitHub, "ALICE");
    data.pull_requests = vec![
        pull_request("2026-10-24T00:00:00Z", 1000),
        pull_request("2026-10-30T23:59:59Z", 1000),
        pull_request("2026-10-23T23:59:59Z", 1000),
        pull_request("2026-09-01T00:00:00Z", 1000),
    ];
    data.pull_requests[0].merged_at = Some(timestamp("2026-10-25T00:00:00Z"));
    data.pull_requests[2].merged_at = Some(timestamp("2026-10-30T12:00:00Z"));
    data.pull_requests.push(PullRequest {
        author: Some("bob".into()),
        ..pull_request("2026-10-25T00:00:00Z", 5000)
    });
    data.commits = vec![
        commit("2026-10-24T00:00:00Z", 10, 4),
        commit("2026-10-30T23:59:59Z", 20, 2),
        commit("2026-10-23T23:59:59Z", 10, 12),
        commit("2026-09-30T23:59:59Z", 5, 9),
        commit("2026-10-31T00:00:00Z", 900, 900),
    ];
    let report = calculate(&[data], through(), &MetricsConfig::default()).unwrap();
    let week = &report.windows[0].metrics;
    assert_comparison(&week.pull_requests_opened, 2.0, 1.0, 1.0, Some(100.0));
    assert_comparison(&week.pull_requests_merged, 2.0, 0.0, 2.0, None);
    assert_comparison(&week.commits, 2.0, 1.0, 1.0, Some(100.0));
    assert_comparison(&week.lines_added, 30.0, 10.0, 20.0, Some(200.0));
    assert_comparison(&week.lines_deleted, 6.0, 12.0, -6.0, Some(-50.0));
    let month = &report.windows[1].metrics;
    assert_comparison(&month.pull_requests_opened, 3.0, 1.0, 2.0, Some(200.0));
    assert_comparison(&month.commits, 3.0, 1.0, 2.0, Some(200.0));
    assert_comparison(&month.lines_added, 40.0, 5.0, 35.0, Some(700.0));
}

#[test]
fn speed_uses_merge_and_first_review_cohorts_including_old_pull_requests() {
    let mut data = activity(Source::GitHub, "alice");
    let mut old = pull_request("2026-08-25T00:00:00Z", 10);
    old.merged_at = Some(timestamp("2026-10-24T00:00:00Z"));
    old.first_review_received_at = Some(timestamp("2026-10-24T00:00:00Z"));
    let mut recent = pull_request("2026-10-24T00:00:00Z", 20);
    recent.merged_at = Some(timestamp("2026-10-26T00:00:00Z"));
    recent.first_review_received_at = Some(timestamp("2026-10-24T12:00:00Z"));
    let mut previous = pull_request("2026-10-17T00:00:00Z", 30);
    previous.merged_at = Some(timestamp("2026-10-21T00:00:00Z"));
    previous.first_review_received_at = Some(timestamp("2026-10-18T00:00:00Z"));
    let mut future = pull_request("2026-10-28T00:00:00Z", 40);
    future.merged_at = Some(timestamp("2026-10-31T00:00:00Z"));
    future.first_review_received_at = Some(timestamp("2026-11-01T00:00:00Z"));
    data.pull_requests = vec![old, recent, previous, future];
    let report = calculate(&[data], through(), &MetricsConfig::default()).unwrap();
    let week = &report.windows[0].metrics;
    assert_comparison(
        &week.median_merge_seconds,
        2_678_400.0,
        345_600.0,
        2_332_800.0,
        Some(675.0),
    );
    assert_comparison(
        &week.median_first_review_seconds,
        2_613_600.0,
        86_400.0,
        2_527_200.0,
        Some(2925.0),
    );
    let month = &report.windows[1].metrics;
    assert_eq!(month.median_merge_seconds.current, Some(345_600.0));
    assert_eq!(month.median_first_review_seconds.current, Some(86_400.0));
    assert_eq!(month.median_merge_seconds.delta, None);
}

#[test]
fn size_uses_opened_owned_pull_requests_and_a_strict_400_line_threshold() {
    let mut data = activity(Source::GitHub, "alice");
    data.pull_requests = vec![
        pull_request("2026-10-24T00:00:00Z", 100),
        pull_request("2026-10-25T00:00:00Z", 400),
        pull_request("2026-10-26T00:00:00Z", 401),
        pull_request("2026-10-27T00:00:00Z", 499),
        pull_request("2026-10-17T00:00:00Z", 200),
        pull_request("2026-10-18T00:00:00Z", 0),
    ];
    data.pull_requests.push(PullRequest {
        author: None,
        ..pull_request("2026-10-25T00:00:00Z", 5000)
    });
    data.pull_requests.push(PullRequest {
        author: Some("bob".into()),
        ..pull_request("2026-10-25T00:00:00Z", 5000)
    });
    let report = calculate(&[data], through(), &MetricsConfig::default()).unwrap();
    let week = &report.windows[0].metrics;
    assert_comparison(
        &week.median_pull_request_lines,
        400.5,
        100.0,
        300.5,
        Some(300.5),
    );
    assert_comparison(&week.large_pull_request_percent, 50.0, 0.0, 50.0, None);
    assert_eq!(
        report.windows[1].metrics.median_pull_request_lines.current,
        Some(300.0)
    );
}

#[test]
fn empty_data_has_zero_counts_absent_medians_and_no_undefined_divisions() {
    let report = calculate(&[], through(), &MetricsConfig::default()).unwrap();
    for window in report.windows {
        assert_comparison(
            &window.metrics.pull_requests_opened,
            0.0,
            0.0,
            0.0,
            Some(0.0),
        );
        assert_comparison(&window.metrics.commits, 0.0, 0.0, 0.0, Some(0.0));
        for value in [
            window.metrics.median_merge_seconds,
            window.metrics.median_first_review_seconds,
            window.metrics.median_pull_request_lines,
            window.metrics.large_pull_request_percent,
        ] {
            assert_eq!(value, Comparison::new(None, None));
        }
    }
    assert!(report.by_source.is_empty());
    assert!(report.by_repository.is_empty());
}

#[test]
fn rejects_negative_elapsed_times_instead_of_reporting_invalid_speed() {
    for first_review in [false, true] {
        let mut data = activity(Source::GitHub, "alice");
        let mut pr = pull_request("2026-10-25T00:00:00Z", 20);
        let at = Some(timestamp("2026-10-24T00:00:00Z"));
        if first_review {
            pr.first_review_received_at = at;
        } else {
            pr.merged_at = at;
        }
        data.pull_requests.push(pr);
        assert!(calculate(&[data], through(), &MetricsConfig::default()).is_err());
    }
}

#[test]
fn default_exclusions_filter_commit_volume_and_pull_request_size() {
    let mut data = activity(Source::GitHub, "alice");
    let files = vec![
        file("src/lib.rs", 300, 100),
        file("Cargo.lock", 1000, 1000),
        file("frontend/package-lock.json", 1000, 1000),
        file("pnpm-lock.yaml", 1000, 1000),
        file("build/output.js", 1000, 1000),
        file("src/generated/api.rs", 1000, 1000),
        file("dist/bundle.js", 1000, 1000),
        file("client.generated.ts", 1000, 1000),
        FileChange {
            previous_path: Some("generated/old.rs".into()),
            ..file("src/moved.rs", 1000, 1000)
        },
    ];
    let mut pr = pull_request("2026-10-24T00:00:00Z", 16_400);
    pr.additions = 8300;
    pr.deletions = 8100;
    pr.files = files.clone();
    data.pull_requests.push(pr);
    let mut change = commit("2026-10-24T00:00:00Z", 8300, 8100);
    change.files = files;
    data.commits.push(change);
    let report = calculate(&[data], through(), &MetricsConfig::default()).unwrap();
    let week = &report.windows[0].metrics;
    assert_eq!(week.lines_added.current, Some(300.0));
    assert_eq!(week.lines_deleted.current, Some(100.0));
    assert_eq!(week.median_pull_request_lines.current, Some(400.0));
    assert_eq!(week.large_pull_request_percent.current, Some(0.0));
}

#[test]
fn custom_exclusions_replace_defaults_and_support_paths_basenames_and_wildcards() {
    let config = MetricsConfig {
        excluded_files: vec![
            "src/ignored.rs".into(),
            "*.snap".into(),
            "assets/?.txt".into(),
            "src/gen/*".into(),
        ],
    };
    let mut data = activity(Source::GitHub, "alice");
    let mut change = commit("2026-10-24T00:00:00Z", 24, 12);
    change.files = vec![
        file("src/ignored.rs", 1, 1),
        file("tests/a.snap", 2, 2),
        file("assets/é.txt", 3, 3),
        file("src/gen/deep/output.rs", 4, 4),
        file("other/ignored.rs", 5, 0),
        file("assets/ab.txt", 6, 0),
        file("Cargo.lock", 3, 2),
    ];
    data.commits.push(change);
    let filtered = calculate(&[data], through(), &config).unwrap();
    assert_eq!(filtered.windows[0].metrics.lines_added.current, Some(14.0));
    assert_eq!(filtered.windows[0].metrics.lines_deleted.current, Some(2.0));
    let mut data = activity(Source::GitHub, "alice");
    let mut change = commit("2026-10-24T00:00:00Z", 3, 2);
    change.files = vec![file("Cargo.lock", 3, 2)];
    data.commits.push(change);
    let unfiltered = calculate(
        &[data],
        through(),
        &MetricsConfig {
            excluded_files: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(unfiltered.windows[0].metrics.lines_added.current, Some(3.0));
}

#[test]
fn reviews_and_comments_count_actions_at_their_dates_in_each_window() {
    use crate::activity::{Comment, CommentKind, Review, ReviewState};
    let mut data = activity(Source::GitHub, "alice");
    let review = |id, date: &str| Review {
        source: Source::GitHub,
        repository: "team/other".into(),
        pull_request_number: 5,
        id,
        author: Some("ALICE".into()),
        submitted_at: timestamp(date),
        state: ReviewState::Approved,
        url: String::new(),
    };
    data.reviews_given = vec![
        review(1, "2026-10-24T00:00:00Z"),
        review(2, "2026-10-30T23:59:59Z"),
        review(3, "2026-10-23T23:59:59Z"),
        review(4, "2026-10-31T00:00:00Z"),
    ];
    data.reviews_received = vec![
        Review {
            author: None,
            ..review(1, "2026-10-24T00:00:00Z")
        },
        Review {
            author: Some("bob".into()),
            ..review(2, "2026-09-30T23:59:59Z")
        },
    ];
    data.comments = vec![
        Comment {
            source: Source::GitHub,
            repository: "team/other".into(),
            issue_number: 5,
            id: 1,
            author: "alice".into(),
            created_at: timestamp("2026-10-24T00:00:00Z"),
            url: String::new(),
            kind: CommentKind::Review,
        },
        Comment {
            source: Source::GitHub,
            repository: "team/issues".into(),
            issue_number: 8,
            id: 2,
            author: "alice".into(),
            created_at: timestamp("2026-10-25T00:00:00Z"),
            url: String::new(),
            kind: CommentKind::Discussion,
        },
    ];
    let report = calculate(&[data], through(), &MetricsConfig::default()).unwrap();
    let week = &report.windows[0].metrics;
    assert_comparison(&week.reviews_given, 2.0, 1.0, 1.0, Some(100.0));
    assert_comparison(&week.reviews_received, 1.0, 0.0, 1.0, None);
    assert_comparison(&week.comments_left, 2.0, 0.0, 2.0, None);
    assert_comparison(
        &report.windows[1].metrics.reviews_received,
        1.0,
        1.0,
        0.0,
        Some(0.0),
    );
}

#[test]
fn multi_source_and_repository_breakdowns_recompute_medians_and_proportions() {
    let mut github = activity(Source::GitHub, "alice");
    github.pull_requests = vec![
        pull_request("2026-10-24T00:00:00Z", 100),
        pull_request("2026-10-25T00:00:00Z", 200),
        pull_request("2026-10-26T00:00:00Z", 300),
    ];
    github.pull_requests[2].repository = "team/second".into();
    let mut bitbucket = activity(Source::Bitbucket, "{alice}");
    bitbucket.pull_requests = vec![
        PullRequest {
            source: Source::Bitbucket,
            author: Some("{alice}".into()),
            ..pull_request("2026-10-24T00:00:00Z", 1000)
        },
        PullRequest {
            source: Source::Bitbucket,
            author: Some("{ALICE}".into()),
            ..pull_request("2026-10-24T00:00:00Z", 5000)
        },
    ];
    bitbucket.commits = vec![Commit {
        source: Source::Bitbucket,
        ..commit("2026-10-25T00:00:00Z", 10, 20)
    }];
    let report = calculate(&[bitbucket, github], through(), &MetricsConfig::default()).unwrap();
    let overall = &report.windows[0].metrics;
    assert_eq!(overall.median_pull_request_lines.current, Some(250.0));
    assert_eq!(overall.large_pull_request_percent.current, Some(25.0));
    assert_eq!(report.by_source.len(), 2);
    assert_eq!(report.by_source[0].source, Source::GitHub);
    assert_eq!(
        report.by_source[0].windows[0]
            .metrics
            .median_pull_request_lines
            .current,
        Some(200.0)
    );
    assert_eq!(report.by_source[1].source, Source::Bitbucket);
    assert_eq!(
        report.by_source[1].windows[0]
            .metrics
            .median_pull_request_lines
            .current,
        Some(1000.0)
    );
    assert_eq!(report.by_repository.len(), 3);
    assert_eq!(report.by_repository[0].repository, "team/project");
    assert_eq!(
        report.by_repository[0].windows[0]
            .metrics
            .median_pull_request_lines
            .current,
        Some(150.0)
    );
    assert_eq!(report.by_repository[1].repository, "team/second");
    assert_eq!(report.by_repository[2].repository, "team/project");
    assert_eq!(report.by_repository[2].source, Source::Bitbucket);
    assert_eq!(
        report.by_repository[2].windows[1]
            .metrics
            .lines_deleted
            .current,
        Some(20.0)
    );
}

#[test]
fn breakdowns_include_sources_without_activity_and_repositories_with_only_comments() {
    use crate::activity::{Comment, CommentKind};
    let empty = activity(Source::Bitbucket, "{alice}");
    let mut github = activity(Source::GitHub, "alice");
    github.comments.push(Comment {
        source: Source::GitHub,
        repository: "team/comments-only".into(),
        issue_number: 8,
        id: 1,
        author: "alice".into(),
        created_at: timestamp("2026-10-24T00:00:00Z"),
        url: String::new(),
        kind: CommentKind::Discussion,
    });
    let report = calculate(&[empty, github], through(), &MetricsConfig::default()).unwrap();
    assert_eq!(report.by_source.len(), 2);
    assert_eq!(
        report.by_source[1].windows[0]
            .metrics
            .median_pull_request_lines
            .current,
        None
    );
    assert_eq!(report.by_repository.len(), 1);
    assert_eq!(
        report.by_repository[0].windows[0]
            .metrics
            .comments_left
            .current,
        Some(1.0)
    );
}

struct FixedSource {
    source: Source,
    fails: bool,
}

#[test]
fn custom_collection_covers_only_current_and_previous_days_including_leap_day() {
    struct LeapDaySource;
    impl ActivitySource for LeapDaySource {
        fn collect_activity(
            &self,
            user: Option<&str>,
            period: Period,
        ) -> Result<Activity, Box<dyn Error>> {
            assert_eq!(user, Some("alice"));
            assert_eq!(period.from.to_string(), "2024-02-29");
            assert_eq!(period.to.to_string(), "2024-03-01");
            Ok(Activity::empty(Source::GitHub, "alice", period))
        }
    }
    let report = collect_period(
        &[SourceRequest {
            source: &LeapDaySource,
            user: Some("alice"),
        }],
        Period::parse("2024-03-01", "2024-03-01").unwrap(),
        &MetricsConfig::default(),
    )
    .unwrap();
    assert_eq!(report.windows.len(), 1);
    assert_eq!(report.windows[0].days, 1);
    assert_eq!(
        report.windows[0].previous_period.from.to_string(),
        "2024-02-29"
    );
}

#[test]
fn custom_period_rejects_incomplete_collection_and_unrepresentable_previous_dates() {
    let mut data = activity(Source::GitHub, "alice");
    data.period = Period::parse("2026-10-26", "2026-10-30").unwrap();
    assert!(
        calculate_period(
            &[data],
            Period::parse("2026-10-28", "2026-10-30").unwrap(),
            &MetricsConfig::default(),
        )
        .is_err()
    );
    assert!(
        collect_period(
            &[],
            Period::parse("0000-01-01", "0000-01-01").unwrap(),
            &MetricsConfig::default(),
        )
        .is_err()
    );
}

impl ActivitySource for FixedSource {
    fn collect_activity(
        &self,
        user: Option<&str>,
        period: Period,
    ) -> Result<Activity, Box<dyn Error>> {
        assert_eq!(user, Some("alice"));
        assert_eq!(period.from, NaiveDate::from_ymd_opt(2026, 9, 1).unwrap());
        assert_eq!(period.to, through());
        if self.fails {
            return Err("collecte interrompue".into());
        }
        let mut result = Activity::empty(self.source, "alice", period);
        result.commits.push(Commit {
            source: self.source,
            ..commit("2026-09-01T00:00:00Z", 3, 1)
        });
        result.commits.push(Commit {
            source: self.source,
            ..commit("2026-10-30T23:59:59Z", 5, 2)
        });
        Ok(result)
    }
}

#[test]
fn collection_requests_sixty_days_once_per_source_and_aggregates_previous_periods() {
    let github = FixedSource {
        source: Source::GitHub,
        fails: false,
    };
    let bitbucket = FixedSource {
        source: Source::Bitbucket,
        fails: false,
    };
    let report = collect(
        &[
            SourceRequest {
                source: &github,
                user: Some("alice"),
            },
            SourceRequest {
                source: &bitbucket,
                user: Some("alice"),
            },
        ],
        through(),
        &MetricsConfig::default(),
    )
    .unwrap();
    assert_comparison(&report.windows[1].metrics.commits, 2.0, 2.0, 0.0, Some(0.0));
    assert_eq!(report.windows[1].metrics.lines_added.previous, Some(6.0));
}

#[test]
fn collection_propagates_errors_instead_of_returning_partial_metrics() {
    let github = FixedSource {
        source: Source::GitHub,
        fails: false,
    };
    let bitbucket = FixedSource {
        source: Source::Bitbucket,
        fails: true,
    };
    let error = collect(
        &[
            SourceRequest {
                source: &github,
                user: Some("alice"),
            },
            SourceRequest {
                source: &bitbucket,
                user: Some("alice"),
            },
        ],
        through(),
        &MetricsConfig::default(),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "collecte interrompue");
}

#[test]
fn rejects_insufficient_collection_periods_and_duplicate_sources() {
    for period in [
        Period::parse("2026-09-02", "2026-10-30").unwrap(),
        Period::parse("2026-09-01", "2026-10-29").unwrap(),
    ] {
        let mut data = activity(Source::GitHub, "alice");
        data.period = period;
        assert!(calculate(&[data], through(), &MetricsConfig::default()).is_err());
    }
    assert!(
        calculate(
            &[
                activity(Source::GitHub, "alice"),
                activity(Source::GitHub, "alice")
            ],
            through(),
            &MetricsConfig::default()
        )
        .is_err()
    );
}

#[test]
fn rejects_unrepresentable_windows_and_overflowing_line_statistics() {
    assert!(calculate(&[], NaiveDate::MIN, &MetricsConfig::default()).is_err());
    assert!(collect(&[], NaiveDate::MIN, &MetricsConfig::default()).is_err());
    for pull_request_overflow in [false, true] {
        let mut data = activity(Source::GitHub, "alice");
        if pull_request_overflow {
            let mut pr = pull_request("2026-10-24T00:00:00Z", u64::MAX);
            pr.deletions = 1;
            pr.files = vec![file("src/overflow.rs", u64::MAX, 1)];
            data.pull_requests.push(pr);
        } else {
            data.commits
                .push(commit("2026-10-24T00:00:00Z", u64::MAX, 0));
            data.commits.push(commit("2026-10-24T01:00:00Z", 1, 0));
        }
        assert!(calculate(&[data], through(), &MetricsConfig::default()).is_err());
    }
}

#[test]
fn api_totals_keep_unlisted_lines_and_subtract_only_available_exclusions() {
    let mut data = activity(Source::GitHub, "alice");
    let mut change = commit("2026-10-24T00:00:00Z", 100, 20);
    change.files = vec![
        file("src/lib.rs", 4, 1),
        file("Cargo.lock", 10, 3),
        file("generated/api.rs", 5, 2),
    ];
    data.commits.push(change);
    let report = calculate(&[data], through(), &MetricsConfig::default()).unwrap();
    assert_eq!(report.windows[0].metrics.lines_added.current, Some(85.0));
    assert_eq!(report.windows[0].metrics.lines_deleted.current, Some(15.0));
}

#[test]
fn inconsistent_exclusions_cannot_make_api_totals_negative_or_overflow() {
    let mut data = activity(Source::GitHub, "alice");
    let mut change = commit("2026-10-24T00:00:00Z", 10, 5);
    change.files = vec![
        file("Cargo.lock", u64::MAX, 2),
        file("generated/api.rs", u64::MAX, 1),
    ];
    data.commits.push(change);
    let report = calculate(&[data], through(), &MetricsConfig::default()).unwrap();
    assert_eq!(report.windows[0].metrics.lines_added.current, Some(0.0));
    assert_eq!(report.windows[0].metrics.lines_deleted.current, Some(2.0));
}
