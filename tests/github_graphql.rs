use dev_metrics::github::{GitHubSource, Period};
use mockito::{Matcher, Server};
use serde_json::{Value, json};

fn connection(nodes: Vec<Value>) -> Value {
    json!({"nodes":nodes,"pageInfo":{"hasNextPage":false}})
}

fn pull_request(number: u64) -> Value {
    json!({"number":number,"user":{"login":"octocat"},"title":"Change","html_url":"https://github.com/team/project/pull/1","created_at":"2026-10-01T00:00:00Z","merged_at":null,"additions":3,"deletions":1,"changed_files":1,"discussion":connection(vec![]),"reviews":connection(vec![])})
}

fn discovery(server: &mut Server, count: u64) -> (mockito::Mock, mockito::Mock) {
    let issues = server.mock("GET", "/search/issues").match_query(Matcher::Any)
        .with_body(json!({"total_count":count,"incomplete_results":false,"items":(1..=count).map(|number| json!({"number":number,"repository_url":"https://api.github.com/repos/team/project","pull_request":{},"user":{"login":"octocat"},"created_at":"2026-10-01T00:00:00Z","updated_at":"2026-10-02T00:00:00Z","comments":0})).collect::<Vec<_>>()} ).to_string()).expect(4).create();
    let commits = server
        .mock("GET", "/search/commits")
        .match_query(Matcher::Any)
        .with_body(json!({"total_count":0,"incomplete_results":false,"items":[]}).to_string())
        .expect_at_least(0)
        .create();
    (issues, commits)
}

#[test]
fn batches_pull_request_reviews_comments_statistics_and_files() {
    let mut server = Server::new();
    let (issues, commits) = discovery(&mut server, 2);
    let metadata = server.mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({"operationName":"PullRequestActivity"})))
        .with_body(json!({"data":{"p0":{"pullRequest":pull_request(1)},"p1":{"pullRequest":pull_request(2)}}}).to_string())
        .expect(1).create();
    let files = server.mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({"operationName":"PullRequestFiles"})))
        .with_body(json!({"data":{"p0":{"pullRequest":{"files":connection(vec![json!({"filename":"src/lib.rs","additions":3,"deletions":1,"changeType":"MODIFIED"})])}},"p1":{"pullRequest":{"files":connection(vec![json!({"filename":"src/lib.rs","additions":3,"deletions":1,"changeType":"MODIFIED"})])}}}}).to_string())
        .expect(1).create();
    let activity = GitHubSource::new(server.url().parse().unwrap(), "test-token")
        .unwrap()
        .activity(
            "octocat",
            Period::parse("2026-10-01", "2026-10-07").unwrap(),
        )
        .unwrap();
    assert_eq!(activity.pull_requests.len(), 2);
    assert_eq!(activity.pull_requests[0].additions, 3);
    assert_eq!(activity.pull_requests[1].files[0].path, "src/lib.rs");
    metadata.assert();
    files.assert();
    issues.assert();
    commits.assert();
}

#[test]
fn cache_reuses_unchanged_pull_requests_and_refreshes_changed_versions() {
    let mut server = Server::new();
    discovery(&mut server, 1);
    let metadata = server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            json!({"operationName":"PullRequestActivity"}),
        ))
        .with_body(json!({"data":{"p0":{"pullRequest":pull_request(1)}}}).to_string())
        .expect(3)
        .create();
    let files = server.mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(json!({"operationName":"PullRequestFiles"})))
        .with_body(json!({"data":{"p0":{"pullRequest":{"files":connection(vec![json!({"filename":"src/lib.rs","additions":3,"deletions":1,"changeType":"MODIFIED"})])}}}}).to_string())
        .expect(3).create();
    let cache = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let result = GitHubSource::new(server.url().parse().unwrap(), "test-token")
            .unwrap()
            .with_cache(cache.path().into())
            .activity(
                "octocat",
                Period::parse("2026-10-01", "2026-10-07").unwrap(),
            )
            .unwrap();
        assert_eq!(result.pull_requests.len(), 1);
    }
    for entry in std::fs::read_dir(cache.path()).unwrap() {
        let path = entry.unwrap().path();
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("test-token")
        );
        std::fs::write(path, b"{broken").unwrap();
    }
    let collect = || {
        GitHubSource::new(server.url().parse().unwrap(), "test-token")
            .unwrap()
            .with_cache(cache.path().into())
            .activity(
                "octocat",
                Period::parse("2026-10-01", "2026-10-07").unwrap(),
            )
            .unwrap()
    };
    assert_eq!(collect().pull_requests.len(), 1);
    server.mock("GET", "/search/issues").match_query(Matcher::Any)
        .with_body(json!({"total_count":1,"incomplete_results":false,"items":[{"number":1,"repository_url":"https://api.github.com/repos/team/project","pull_request":{},"user":{"login":"octocat"},"created_at":"2026-10-01T00:00:00Z","updated_at":"2026-10-03T00:00:00Z","comments":0}]}).to_string()).expect(4).create();
    let changed = GitHubSource::new(server.url().parse().unwrap(), "test-token")
        .unwrap()
        .with_cache(cache.path().into())
        .activity(
            "octocat",
            Period::parse("2026-10-01", "2026-10-07").unwrap(),
        )
        .unwrap();
    assert_eq!(changed.pull_requests.len(), 1);
    metadata.assert();
    files.assert();
}

#[test]
fn commit_requests_run_concurrently_with_at_most_eight_in_flight() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut server = Server::new();
    discovery(&mut server, 0);
    server.mock("GET", "/search/commits").match_query(Matcher::Any)
        .with_body(json!({"total_count":16,"incomplete_results":false,"items":(1..=16).map(|number| json!({"sha":format!("{number:040x}"),"repository":{"full_name":"team/project"}})).collect::<Vec<_>>()} ).to_string()).create();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut requests = Vec::new();
    for number in 1..=16 {
        let sha = format!("{number:040x}");
        let body = json!({"sha":sha,"html_url":"https://github.com/team/project/commit/a","commit":{"message":"Change","author":{"date":"2026-10-02T00:00:00Z"}},"stats":{"additions":0,"deletions":0},"files":[]}).to_string();
        let active = active.clone();
        let peak = peak.clone();
        requests.push(
            server
                .mock("GET", format!("/repos/team/project/commits/{sha}").as_str())
                .match_query(Matcher::Any)
                .with_chunked_body(move |writer| {
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let result = writer.write_all(body.as_bytes());
                    active.fetch_sub(1, Ordering::SeqCst);
                    result
                })
                .expect(1)
                .create(),
        );
    }
    let activity = GitHubSource::new(server.url().parse().unwrap(), "test-token")
        .unwrap()
        .activity(
            "octocat",
            Period::parse("2026-10-01", "2026-10-07").unwrap(),
        )
        .unwrap();
    assert_eq!(activity.commits.len(), 16);
    for request in requests {
        request.assert();
    }
    let peak = peak.load(Ordering::SeqCst);
    assert!((2..=8).contains(&peak), "in-flight requests: {peak}");
    assert!(
        activity
            .commits
            .windows(2)
            .all(|pair| pair[0].sha < pair[1].sha)
    );
}

#[test]
fn exhausted_quota_on_success_delays_the_next_request() {
    let mut server = Server::new();
    let empty = json!({"total_count":0,"incomplete_results":false,"items":[]}).to_string();
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 2;
    let first = server
        .mock("GET", "/search/issues")
        .match_query(Matcher::Any)
        .with_header("x-ratelimit-remaining", "0")
        .with_header("x-ratelimit-reset", &reset.to_string())
        .with_body(empty.clone())
        .expect(1)
        .create();
    let remaining = server
        .mock("GET", "/search/issues")
        .match_query(Matcher::Any)
        .with_body(empty.clone())
        .expect(3)
        .create();
    server
        .mock("GET", "/search/commits")
        .match_query(Matcher::Any)
        .with_body(empty)
        .create();
    let started = std::time::Instant::now();
    GitHubSource::new(server.url().parse().unwrap(), "test-token")
        .unwrap()
        .activity(
            "octocat",
            Period::parse("2026-10-01", "2026-10-07").unwrap(),
        )
        .unwrap();
    first.assert();
    remaining.assert();
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
}

#[test]
fn cached_issue_comments_preserve_comments_left_without_reloading() {
    let mut server = Server::new();
    let issue = json!({"number":1,"repository_url":"https://api.github.com/repos/team/project","user":{"login":"alice"},"created_at":"2026-09-01T00:00:00Z","updated_at":"2026-10-02T00:00:00Z","comments":1});
    server
        .mock("GET", "/search/issues")
        .match_query(Matcher::Any)
        .with_body(json!({"total_count":1,"incomplete_results":false,"items":[issue]}).to_string())
        .expect(8)
        .create();
    server
        .mock("GET", "/search/commits")
        .match_query(Matcher::Any)
        .with_body(json!({"total_count":0,"incomplete_results":false,"items":[]}).to_string())
        .expect(2)
        .create();
    let comments = server.mock("GET", "/repos/team/project/issues/1/comments").match_query(Matcher::Any)
        .with_body(json!([{"id":1,"user":{"login":"octocat"},"created_at":"2026-10-02T00:00:00Z","html_url":"https://github.com/team/project/issues/1#issuecomment-1"}]).to_string()).expect(1).create();
    let cache = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let activity = GitHubSource::new(server.url().parse().unwrap(), "test-token")
            .unwrap()
            .with_cache(cache.path().into())
            .activity(
                "octocat",
                Period::parse("2026-10-01", "2026-10-07").unwrap(),
            )
            .unwrap();
        assert_eq!(activity.comments.len(), 1);
    }
    comments.assert();
}

#[test]
fn graphql_errors_missing_nodes_and_invalid_ids_fail_without_caching_or_echoing_payloads() {
    let mut invalid = pull_request(1);
    invalid["discussion"] = connection(vec![
        json!({"id":"test-token","user":{"login":"octocat"},"created_at":"2026-10-02T00:00:00Z","html_url":"https://github.com/team/project/pull/1"}),
    ]);
    let mut missing_date = pull_request(1);
    missing_date["reviews"] = connection(vec![
        json!({"id":"3","user":{"login":"alice"},"submitted_at":null,"state":"APPROVED","html_url":"https://github.com/team/project/pull/1","comments":connection(vec![])}),
    ]);
    for body in [
        json!({"errors":[{"message":"test-token"}],"data":{"p0":{"pullRequest":pull_request(1)}}}),
        json!({"data":null}),
        json!({"data":{}}),
        json!({"data":{"p0":{"pullRequest":null}}}),
        json!({"data":{"p0":{"pullRequest":invalid}}}),
        json!({"data":{"p0":{"pullRequest":missing_date}}}),
    ] {
        let mut server = Server::new();
        discovery(&mut server, 1);
        server
            .mock("POST", "/graphql")
            .with_body(body.to_string())
            .create();
        let cache = tempfile::tempdir().unwrap();
        let error = GitHubSource::new(server.url().parse().unwrap(), "test-token")
            .unwrap()
            .with_cache(cache.path().into())
            .activity(
                "octocat",
                Period::parse("2026-10-01", "2026-10-07").unwrap(),
            )
            .unwrap_err()
            .to_string();
        assert!(!error.contains("test-token"), "{error}");
        assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
    }
}

#[test]
fn irrelevant_pull_requests_never_fetch_files_but_historical_comments_are_kept() {
    let mut server = Server::new();
    discovery(&mut server, 2);
    let mut stale = pull_request(1);
    stale["created_at"] = json!("2026-09-01T00:00:00Z");
    let mut commented = pull_request(2);
    commented["created_at"] = json!("2026-09-01T00:00:00Z");
    commented["user"] = json!({"login":"alice"});
    commented["discussion"] = connection(vec![
        json!({"id":"12345678901234","user":{"login":"octocat"},"created_at":"2026-10-02T00:00:00Z","html_url":"https://github.com/team/project/pull/2#issuecomment-1"}),
    ]);
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            json!({"operationName":"PullRequestActivity"}),
        ))
        .with_body(
            json!({"data":{"p0":{"pullRequest":stale},"p1":{"pullRequest":commented}}}).to_string(),
        )
        .create();
    let files = server.mock("POST", "/graphql")
        .match_body(Matcher::AllOf(vec![Matcher::PartialJson(json!({"operationName":"PullRequestFiles"})), Matcher::Regex("pullRequest\\(number:2\\)".into())]))
        .with_body(json!({"data":{"p0":{"pullRequest":{"files":connection(vec![json!({"filename":"src/lib.rs","additions":3,"deletions":1,"changeType":"MODIFIED"})])}}}}).to_string()).expect(1).create();
    let activity = GitHubSource::new(server.url().parse().unwrap(), "test-token")
        .unwrap()
        .activity(
            "octocat",
            Period::parse("2026-10-01", "2026-10-07").unwrap(),
        )
        .unwrap();
    assert_eq!(activity.pull_requests.len(), 1);
    assert_eq!(activity.pull_requests[0].number, 2);
    assert_eq!(activity.comments[0].id, 12345678901234);
    files.assert();
}

#[test]
fn unpublished_review_comments_do_not_count_as_activity() {
    let mut server = Server::new();
    discovery(&mut server, 1);
    let mut pull = pull_request(1);
    pull["created_at"] = json!("2026-09-01T00:00:00Z");
    pull["reviews"] = connection(vec![
        json!({"id":"3","user":{"login":"octocat"},"submitted_at":null,"state":"PENDING","html_url":"https://github.com/team/project/pull/1","comments":connection(vec![json!({"id":"4","user":{"login":"octocat"},"created_at":"2026-10-02T00:00:00Z","html_url":"https://github.com/team/project/pull/1"})])}),
    ]);
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            json!({"operationName":"PullRequestActivity"}),
        ))
        .with_body(json!({"data":{"p0":{"pullRequest":pull}}}).to_string())
        .create();
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            json!({"operationName":"PullRequestFiles"}),
        ))
        .with_body(json!({"data":{"p0":{"pullRequest":{"files":connection(vec![])}}}}).to_string())
        .expect(0)
        .create();
    let activity = GitHubSource::new(server.url().parse().unwrap(), "test-token")
        .unwrap()
        .activity(
            "octocat",
            Period::parse("2026-10-01", "2026-10-07").unwrap(),
        )
        .unwrap();
    assert!(activity.comments.is_empty());
    assert!(activity.pull_requests.is_empty());
}

#[test]
fn long_quota_wait_is_reported_as_a_rate_limit_not_a_network_failure() {
    let mut server = Server::new();
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    server
        .mock("GET", "/search/issues")
        .match_query(Matcher::Any)
        .with_header("x-ratelimit-remaining", "0")
        .with_header("x-ratelimit-reset", &reset.to_string())
        .with_body(json!({"total_count":0,"incomplete_results":false,"items":[]}).to_string())
        .expect_at_least(1)
        .create();
    let error = GitHubSource::new(server.url().parse().unwrap(), "test-token")
        .unwrap()
        .activity(
            "octocat",
            Period::parse("2026-10-01", "2026-10-07").unwrap(),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("limite de débit"), "{error}");
    assert!(!error.contains("échec réseau"), "{error}");
}

#[test]
fn full_commit_search_pages_are_fetched_concurrently_and_partial_results_still_fail() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut server = Server::new();
    discovery(&mut server, 0);
    let references = (0..100).map(|number| json!({"sha":format!("{number:040x}"),"repository":{"full_name":"team/project"}})).collect::<Vec<_>>();
    server
        .mock("GET", "/search/commits")
        .match_query(Matcher::UrlEncoded("page".into(), "1".into()))
        .with_body(
            json!({"total_count":201,"incomplete_results":false,"items":references}).to_string(),
        )
        .expect(1)
        .create();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    for number in [2, 3] {
        let body = json!({"total_count":201,"incomplete_results":number==3,"items":if number==2 { references.clone() } else { references[..1].to_vec() }}).to_string();
        let active = active.clone();
        let peak = peak.clone();
        server
            .mock("GET", "/search/commits")
            .match_query(Matcher::UrlEncoded("page".into(), number.to_string()))
            .with_chunked_body(move |writer| {
                peak.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(200));
                let result = writer.write_all(body.as_bytes());
                active.fetch_sub(1, Ordering::SeqCst);
                result
            })
            .expect(1)
            .create();
    }
    let error = GitHubSource::new(server.url().parse().unwrap(), "test-token")
        .unwrap()
        .activity(
            "octocat",
            Period::parse("2026-10-01", "2026-10-07").unwrap(),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("incomplète"), "{error}");
    assert_eq!(peak.load(Ordering::SeqCst), 2);
}
