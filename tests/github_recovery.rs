use dev_metrics::github::{GitHubSource, Period};
use mockito::{Matcher, Server};
use serde_json::{Value, json};

fn period() -> Period {
    Period::parse("2026-10-01", "2026-10-07").unwrap()
}
fn source(server: &Server) -> GitHubSource {
    GitHubSource::new(server.url().parse().unwrap(), "test-token").unwrap()
}
fn connection(nodes: Vec<Value>, more: bool) -> Value {
    json!({"nodes":nodes,"pageInfo":{"hasNextPage":more}})
}
fn pull(number: u64) -> Value {
    json!({"number":number,"user":{"login":"octocat"},"title":"Change","html_url":"https://github.com/team/project/pull/1","created_at":"2026-10-01T00:00:00Z","merged_at":null,"additions":3,"deletions":1,"changed_files":1,"discussion":connection(vec![],false),"reviews":connection(vec![],false)})
}
fn discovery(server: &mut Server, pulls: u64, commits: Vec<&str>) {
    server.mock("GET", "/search/issues").match_query(Matcher::Any)
        .with_body(json!({"total_count":pulls,"incomplete_results":false,"items":(1..=pulls).map(|number|json!({"number":number,"repository_url":"https://api.github.com/repos/team/project","pull_request":{},"user":{"login":"octocat"},"created_at":"2026-10-01T00:00:00Z","updated_at":"2026-10-02T00:00:00Z","comments":0})).collect::<Vec<_>>()} ).to_string()).expect_at_least(0).create();
    server.mock("GET", "/search/commits").match_query(Matcher::Any)
        .with_body(json!({"total_count":commits.len(),"incomplete_results":false,"items":commits.iter().map(|sha|json!({"sha":sha,"repository":{"full_name":"team/project"}})).collect::<Vec<_>>()} ).to_string()).expect_at_least(0).create();
}
fn commit(sha: &str) -> Value {
    json!({"sha":sha,"html_url":"https://github.com/team/project/commit/a","commit":{"message":"Change","author":{"date":"2026-10-02T00:00:00Z"}},"stats":{"additions":0,"deletions":0},"files":[]})
}
fn files() -> Value {
    json!({"files":connection(vec![json!({"filename":"src/lib.rs","additions":3,"deletions":1,"changeType":"MODIFIED"})],false)})
}

#[test]
fn inaccessible_commits_are_warned_and_other_commits_survive() {
    for status in [403, 404, 503] {
        let mut server = Server::new();
        discovery(&mut server, 0, vec!["a", "b"]);
        let bad = server
            .mock("GET", "/repos/team/project/commits/a")
            .match_query(Matcher::Any)
            .with_status(status)
            .with_body("private test-token")
            .expect(if status == 503 { 4 } else { 1 })
            .create();
        let good = server
            .mock("GET", "/repos/team/project/commits/b")
            .match_query(Matcher::Any)
            .with_body(commit("b").to_string())
            .expect(1)
            .create();
        let activity = source(&server).activity("octocat", period()).unwrap();
        assert_eq!(activity.commits.len(), 1);
        assert_eq!(activity.commits[0].sha, "b");
        let value = serde_json::to_value(&activity).unwrap();
        let schema: Value =
            serde_json::from_str(include_str!("../schemas/activity-v1.schema.json")).unwrap();
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&value));
        assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
        assert_eq!(value["warnings"][0]["code"], "ITEM_UNAVAILABLE");
        assert_eq!(
            value["warnings"][0]["subject"],
            json!({"kind":"commit","sha":"a"})
        );
        assert!(
            value["warnings"][0]["message"]
                .as_str()
                .unwrap()
                .contains(&status.to_string())
        );
        assert!(!value.to_string().contains("test-token"));
        bad.assert();
        good.assert();
    }
}

#[test]
fn graphql_partial_forbidden_or_missing_pr_keeps_the_other_pr() {
    for error_type in ["FORBIDDEN", "NOT_FOUND"] {
        let mut server = Server::new();
        discovery(&mut server, 2, vec![]);
        server.mock("POST","/graphql").match_body(Matcher::PartialJson(json!({"operationName":"PullRequestActivity"})))
            .with_body(json!({"data":{"p0":{"pullRequest":null},"p1":{"pullRequest":pull(2)}},"errors":[{"type":error_type,"path":["p0","pullRequest"],"message":"private test-token"}]}).to_string()).create();
        let files = server
            .mock("POST", "/graphql")
            .match_body(Matcher::PartialJson(
                json!({"operationName":"PullRequestFiles"}),
            ))
            .with_body(json!({"data":{"p0":{"pullRequest":files()}}}).to_string())
            .expect(1)
            .create();
        let activity = source(&server).activity("octocat", period()).unwrap();
        assert_eq!(activity.pull_requests.len(), 1);
        assert_eq!(activity.pull_requests[0].number, 2);
        assert_eq!(activity.warnings.len(), 1);
        assert!(
            !serde_json::to_string(&activity)
                .unwrap()
                .contains("test-token")
        );
        files.assert();
    }
}

#[test]
fn authentication_failures_on_identity_and_search_remain_fatal() {
    for status in [401, 403] {
        let mut server = Server::new();
        server
            .mock("GET", "/user")
            .with_status(status)
            .with_body("private test-token")
            .expect(1)
            .create();
        let error = source(&server)
            .authenticated_user()
            .unwrap_err()
            .to_string();
        assert!(error.contains(&status.to_string()), "{error}");
        assert!(!error.contains("test-token"));
        server
            .mock("GET", "/search/issues")
            .match_query(Matcher::Any)
            .with_status(status)
            .with_body("private test-token")
            .expect_at_least(1)
            .create();
        let error = source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string();
        assert!(error.contains(&status.to_string()), "{error}");
        assert!(!error.contains("test-token"));
    }
}

#[test]
fn review_and_file_failures_skip_only_the_affected_pr_without_leaking_its_activity() {
    for (operation, status) in [("reviews", 403), ("files", 404)] {
        let mut server = Server::new();
        discovery(&mut server, 2, vec![]);
        let mut bad = pull(1);
        if operation == "reviews" {
            bad["reviews"] = connection(vec![], true);
        } else {
            bad["reviews"] = connection(
                vec![
                    json!({"id":"5","user":{"login":"bob"},"submitted_at":"2026-10-02T00:00:00Z","state":"APPROVED","html_url":"https://github.com/team/project/pull/1","comments":connection(vec![],false)}),
                ],
                false,
            );
            bad["discussion"] = connection(
                vec![
                    json!({"id":"6","user":{"login":"octocat"},"created_at":"2026-10-02T00:00:00Z","html_url":"https://github.com/team/project/pull/1"}),
                ],
                false,
            );
        }
        server
            .mock("POST", "/graphql")
            .match_body(Matcher::PartialJson(
                json!({"operationName":"PullRequestActivity"}),
            ))
            .with_body(
                json!({"data":{"p0":{"pullRequest":bad},"p1":{"pullRequest":pull(2)}}}).to_string(),
            )
            .create();
        if operation == "reviews" {
            server
                .mock("GET", "/repos/team/project/pulls/1/comments")
                .match_query(Matcher::Any)
                .with_body("[]")
                .create();
        }
        let bad = server
            .mock(
                "GET",
                format!("/repos/team/project/pulls/1/{operation}").as_str(),
            )
            .match_query(Matcher::Any)
            .with_status(status)
            .with_body("private test-token")
            .expect(1)
            .create();
        let data = if operation == "files" {
            json!({"p0":{"pullRequest":{"files":connection(vec![],true)}},"p1":{"pullRequest":files()}})
        } else {
            json!({"p0":{"pullRequest":files()}})
        };
        server
            .mock("POST", "/graphql")
            .match_body(Matcher::PartialJson(
                json!({"operationName":"PullRequestFiles"}),
            ))
            .with_body(json!({"data":data}).to_string())
            .create();
        let activity = source(&server).activity("octocat", period()).unwrap();
        assert_eq!(activity.pull_requests.len(), 1);
        assert_eq!(activity.pull_requests[0].number, 2);
        assert!(activity.reviews_received.is_empty());
        assert!(activity.comments.is_empty());
        assert_eq!(activity.warnings.len(), 1);
        assert!(activity.warnings[0].message.contains(&status.to_string()));
        bad.assert();
    }
}

#[test]
fn an_unwaitable_item_quota_remains_fatal_with_its_reset_time() {
    let mut server = Server::new();
    discovery(&mut server, 0, vec!["a"]);
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 900;
    server
        .mock("GET", "/repos/team/project/commits/a")
        .match_query(Matcher::Any)
        .with_status(403)
        .with_header("x-ratelimit-remaining", "0")
        .with_header("x-ratelimit-reset", &reset.to_string())
        .with_body("API rate limit exceeded test-token")
        .expect(1)
        .create();
    let error = source(&server)
        .activity("octocat", period())
        .unwrap_err()
        .to_string();
    assert!(error.contains("limite de débit"), "{error}");
    assert!(error.contains(&reset.to_string()), "{error}");
    assert!(!error.contains("test-token"));
    assert!(!error.contains("vérifiez le token"));
}

#[test]
fn a_failed_graphql_batch_does_not_poison_the_cache_or_later_requests() {
    let mut server = Server::new();
    discovery(&mut server, 1, vec![]);
    let inaccessible = server
        .mock("POST", "/graphql")
        .with_status(403)
        .with_body("Resource not accessible test-token")
        .expect(1)
        .create();
    let cache = tempfile::tempdir().unwrap();
    let source = source(&server).with_cache(cache.path().into());
    let first = source.activity("octocat", period()).unwrap();
    assert_eq!(first.warnings.len(), 1);
    assert!(first.pull_requests.is_empty());
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
    let recovered = server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            json!({"operationName":"PullRequestActivity"}),
        ))
        .with_body(json!({"data":{"p0":{"pullRequest":pull(1)}}}).to_string())
        .expect(1)
        .create();
    server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            json!({"operationName":"PullRequestFiles"}),
        ))
        .with_body(json!({"data":{"p0":{"pullRequest":files()}}}).to_string())
        .create();
    let next = source.activity("octocat", period()).unwrap();
    assert_eq!(next.pull_requests.len(), 1);
    assert!(next.warnings.is_empty());
    inaccessible.assert();
    recovered.assert();
}

#[test]
fn semantically_invalid_cached_reviews_are_refetched_instead_of_aborting() {
    let mut server = Server::new();
    discovery(&mut server, 1, vec![]);
    let metadata = server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            json!({"operationName":"PullRequestActivity"}),
        ))
        .with_body(json!({"data":{"p0":{"pullRequest":pull(1)}}}).to_string())
        .expect(2)
        .create();
    let files = server
        .mock("POST", "/graphql")
        .match_body(Matcher::PartialJson(
            json!({"operationName":"PullRequestFiles"}),
        ))
        .with_body(json!({"data":{"p0":{"pullRequest":files()}}}).to_string())
        .expect(1)
        .create();
    let cache = tempfile::tempdir().unwrap();
    let source = source(&server).with_cache(cache.path().into());
    source.activity("octocat", period()).unwrap();
    for entry in std::fs::read_dir(cache.path()).unwrap() {
        let path = entry.unwrap().path();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if value["value"].get("detail").is_some() {
            value["value"]["reviews"] = json!([{"id":5,"user":{"login":"bob"},"submitted_at":null,"state":"APPROVED","html_url":"https://github.com/team/project/pull/1"}]);
            std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }
    }
    let activity = source.activity("octocat", period()).unwrap();
    assert_eq!(activity.pull_requests.len(), 1);
    assert!(activity.warnings.is_empty());
    metadata.assert();
    files.assert();
}
