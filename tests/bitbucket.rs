use dev_metrics::activity::{Period, Source};
use dev_metrics::bitbucket::BitbucketSource;
use mockito::{Matcher, Server};
use serde_json::{Value, json};

fn source(server: &Server) -> BitbucketSource {
    BitbucketSource::new(
        format!("{}/2.0/", server.url()).parse().unwrap(),
        "test-token",
        "team",
    )
    .unwrap()
}

#[test]
fn a_new_collection_preserves_retry_after_longer_than_the_waiting_budget() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::Any)
        .with_status(429)
        .with_header("retry-after", "301")
        .expect(1)
        .create();
    let forbidden = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::Any)
        .with_body(user("{alice}").to_string())
        .expect(0)
        .create();
    endpoint(&mut server, "/2.0/repositories/team", json!({"values":[]}));
    let source = source(&server);
    for _ in 0..2 {
        let error = source.activity(None, period()).unwrap_err().to_string();
        assert!(error.contains("429"), "{error}");
    }
    limited.assert();
    forbidden.assert();
}

#[test]
fn a_new_collection_can_recover_after_429_without_ignoring_the_last_retry_after() {
    let mut server = Server::new();
    let initial = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::Any)
        .with_status(429)
        .with_header("retry-after", "0")
        .expect(2)
        .create();
    let exhausted = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::Any)
        .with_status(429)
        .with_header("retry-after", "1")
        .expect(1)
        .create();
    let success = endpoint(&mut server, "/2.0/user", user("{alice}"));
    endpoint(&mut server, "/2.0/repositories/team", json!({"values":[]}));
    let source = source(&server);
    assert!(source.activity(None, period()).is_err());
    let started = std::time::Instant::now();
    assert_eq!(source.activity(None, period()).unwrap().user, "{alice}");
    assert!(started.elapsed() >= std::time::Duration::from_millis(900));
    for mock in [initial, exhausted, success] {
        mock.assert();
    }
}

#[test]
fn rate_limit_cooldown_is_shared_by_all_workers() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};
    let mut server = Server::new();
    repository(
        &mut server,
        (1..=12)
            .map(|id| pr(id, Some("{alice}"), "2026-10-02T00:00:00Z", "OPEN"))
            .collect(),
        vec![],
    );
    let observed = Arc::new(Mutex::new((None, Vec::new())));
    for id in 1..=12 {
        for kind in ["activity", "comments", "diffstat"] {
            let observed = observed.clone();
            let attempts = AtomicUsize::new(0);
            server
                .mock(
                    "GET",
                    format!("/2.0/repositories/team/project/pullrequests/{id}/{kind}").as_str(),
                )
                .match_query(Matcher::Any)
                .with_status_code_from_request(move |_| {
                    let now = Instant::now();
                    let mut observed = observed.lock().unwrap();
                    observed.1.push(now);
                    if id == 1 && kind == "activity" && attempts.fetch_add(1, Ordering::SeqCst) == 0
                    {
                        observed.0 = Some(now);
                        429
                    } else {
                        200
                    }
                })
                .with_header("retry-after", "1")
                .with_chunked_body(|writer| {
                    std::thread::sleep(Duration::from_millis(50));
                    writer.write_all(b"{\"values\":[]}")
                })
                .create();
        }
    }
    assert_eq!(
        source(&server)
            .activity(None, period())
            .unwrap()
            .pull_requests
            .len(),
        12
    );
    let observed = observed.lock().unwrap();
    let limited_at = observed.0.unwrap();
    assert_eq!(observed.1.len(), 37);
    assert!(
        observed.1.iter().all(|time| {
            let elapsed = time.saturating_duration_since(limited_at);
            elapsed < Duration::from_millis(150) || elapsed >= Duration::from_secs(1)
        }),
        "requests started during the shared cooldown"
    );
}

#[test]
fn every_endpoint_requests_partial_responses_with_pagination_metadata() {
    use std::sync::{Arc, Mutex};
    let mut server = Server::new();
    let queries = Arc::new(Mutex::new(Vec::new()));
    let base = "/2.0/repositories/team/project";
    let mut mocks = Vec::new();
    for (path, body) in [
        ("/2.0/user".into(), user("{alice}")),
        (
            "/2.0/repositories/team".into(),
            json!({"values":[{"full_name":"team/project"}]}),
        ),
        (
            format!("{base}/pullrequests"),
            json!({"values":[pr(7,Some("{alice}"),"2026-10-02T00:00:00Z","OPEN")]}),
        ),
        (
            format!("{base}/pullrequests/7/activity"),
            json!({"values":[]}),
        ),
        (
            format!("{base}/pullrequests/7/comments"),
            json!({"values":[]}),
        ),
        (
            format!("{base}/pullrequests/7/diffstat"),
            json!({"values":[diff(Some("a.rs"),Some("old.rs"),3,1)]}),
        ),
        (
            format!("{base}/commits"),
            json!({"values":[commit("abc",Some("{alice}"),"2026-10-02T00:00:00Z")]}),
        ),
        (format!("{base}/diffstat/abc"), json!({"values":[]})),
    ] {
        let queries = queries.clone();
        mocks.push(
            server
                .mock("GET", path.as_str())
                .match_query(Matcher::Any)
                .with_status_code_from_request(move |request| {
                    queries.lock().unwrap().push(
                        reqwest::Url::parse(&format!(
                            "http://localhost{}",
                            request.path_and_query()
                        ))
                        .unwrap(),
                    );
                    200
                })
                .with_body(body.to_string())
                .expect(1)
                .create(),
        );
    }
    assert_eq!(
        source(&server)
            .activity(None, period())
            .unwrap()
            .pull_requests[0]
            .additions,
        3
    );
    for mock in mocks {
        mock.assert();
    }
    for url in queries.lock().unwrap().iter() {
        let fields = url
            .query_pairs()
            .find(|(key, _)| key == "fields")
            .unwrap()
            .1
            .into_owned();
        if url.path() == "/2.0/user" {
            assert_eq!(fields, "uuid");
            continue;
        }
        let fields = fields.split(',').collect::<Vec<_>>();
        assert!(fields.contains(&"next") && fields.contains(&"size"));
        assert!(fields.iter().any(|field| field.starts_with("values.")));
        assert!(!fields.contains(&"values") && !fields.iter().any(|field| field.contains('*')));
        let pagelen = url
            .query_pairs()
            .find(|(key, _)| key == "pagelen")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(
            pagelen,
            if url.path().ends_with("/pullrequests") {
                "50"
            } else {
                "100"
            }
        );
    }
}

#[test]
fn preserves_recent_commits_after_older_pages_in_topological_order() {
    let mut server = Server::new();
    endpoint(&mut server, "/2.0/user", user("{alice}"));
    endpoint(
        &mut server,
        "/2.0/repositories/team",
        json!({"values":[{"full_name":"team/project","updated_on":"2020-01-01T00:00:00Z"}]}),
    );
    endpoint(
        &mut server,
        "/2.0/repositories/team/project/pullrequests",
        json!({"values":[]}),
    );
    let first = server.mock("GET", "/2.0/repositories/team/project/commits")
        .match_query(Matcher::Regex("^fields=".into()))
        .with_body(json!({"values":[commit("abc", Some("{alice}"), "2026-09-01T00:00:00Z")],"next":"?cursor=older-topology","size":2}).to_string()).expect(1).create();
    let second = server
        .mock("GET", "/2.0/repositories/team/project/commits")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("cursor".into(), "older-topology".into()),
            Matcher::Regex("fields=next%2Csize%2Cvalues".into()),
            Matcher::UrlEncoded("pagelen".into(), "100".into()),
        ]))
        .with_body(
            json!({"values":[commit("def", Some("{alice}"), "2026-10-02T00:00:00Z")],"size":2})
                .to_string(),
        )
        .expect(1)
        .create();
    let excluded = server
        .mock("GET", "/2.0/repositories/team/project/diffstat/abc")
        .match_query(Matcher::Any)
        .expect(0)
        .create();
    let selected = endpoint(
        &mut server,
        "/2.0/repositories/team/project/diffstat/def",
        json!({"values":[]}),
    );
    let activity = source(&server).activity(None, period()).unwrap();
    assert_eq!(activity.commits.len(), 1);
    assert_eq!(activity.commits[0].sha, "def");
    for mock in [first, second, excluded, selected] {
        mock.assert();
    }
}

#[test]
fn rejects_unsafe_retry_delays_without_retrying_or_disclosing_them() {
    for delay in ["-1", "301", "9999999999999999999999999", "test-token", ""] {
        let mut server = Server::new();
        let limited = server
            .mock("GET", "/2.0/user")
            .match_query(Matcher::Any)
            .with_status(429)
            .with_header("retry-after", delay)
            .expect(1)
            .create();
        let error = source(&server)
            .activity(None, period())
            .unwrap_err()
            .to_string();
        assert!(error.contains("429"));
        assert!(!error.contains("test-token"));
        limited.assert();
    }
}

#[test]
fn backs_off_when_retry_after_is_absent() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::Any)
        .with_status(429)
        .expect(1)
        .create();
    let success = endpoint(&mut server, "/2.0/user", user("{alice}"));
    endpoint(&mut server, "/2.0/repositories/team", json!({"values":[]}));
    let started = std::time::Instant::now();
    assert!(source(&server).activity(None, period()).is_ok());
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
    limited.assert();
    success.assert();
}

#[test]
fn retries_two_rate_limits_and_counts_every_attempt_in_verbose_output() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::Any)
        .match_header("authorization", "Bearer test-token")
        .with_status(429)
        .with_header("retry-after", "0")
        .expect(2)
        .create();
    let success = endpoint(&mut server, "/2.0/user", user("{alice}"));
    endpoint(&mut server, "/2.0/repositories/team", json!({"values":[]}));
    let log = tempfile::NamedTempFile::new().unwrap();
    let progress = dev_metrics::progress::Progress::new(log.reopen().unwrap(), true, false, true);
    assert!(
        source(&server)
            .with_progress(progress)
            .activity(None, period())
            .is_ok()
    );
    limited.assert();
    success.assert();
    assert!(
        std::fs::read_to_string(log.path())
            .unwrap()
            .contains("identité : 3 requêtes, 0 pages")
    );
}

#[test]
fn stops_persistent_rate_limits_after_three_attempts() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::Any)
        .with_status(429)
        .with_header("retry-after", "0")
        .expect(3)
        .create();
    let error = source(&server)
        .activity(None, period())
        .unwrap_err()
        .to_string();
    assert!(error.contains("429"), "{error}");
    limited.assert();
}

#[test]
fn bounds_the_local_hourly_budget_including_identity_requests() {
    let mut server = Server::new();
    let identity = endpoint(&mut server, "/2.0/user", user("{alice}"));
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let pages = server.mock("GET", "/2.0/repositories/team").match_query(Matcher::Any)
        .with_status_code_from_request(move |_| if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 999 { 200 } else { 500 })
        .with_body_from_request(|request| {
            let url = reqwest::Url::parse(&format!("http://localhost{}", request.path_and_query())).unwrap();
            let page = url.query_pairs().find(|(key, _)| key == "cursor").map_or(0, |(_, value)| value.parse::<usize>().unwrap());
            json!({"values":[{"full_name":format!("team/project{page}")}], "next":format!("?cursor={}",page+1)}).to_string().into_bytes()
        }).expect(999).create();
    let source = source(&server);
    for _ in 0..2 {
        let error = source.activity(None, period()).unwrap_err().to_string();
        assert!(error.contains("1000 requêtes"), "{error}");
    }
    identity.assert();
    pages.assert();
}

#[test]
fn verbose_summarizes_attempts_pages_and_collection_time() {
    let mut server = Server::new();
    endpoint(&mut server, "/2.0/user", user("{alice}"));
    endpoint(&mut server, "/2.0/repositories/team", json!({"values":[]}));
    let log = tempfile::NamedTempFile::new().unwrap();
    let progress = dev_metrics::progress::Progress::new(log.reopen().unwrap(), true, false, true);
    source(&server)
        .with_progress(progress)
        .activity(None, period())
        .unwrap();
    let output = std::fs::read_to_string(log.path()).unwrap();
    assert!(
        output.contains("identité : 1 requêtes, 0 pages"),
        "{output}"
    );
    assert!(output.contains("dépôts : 1 requêtes, 1 pages"), "{output}");
    assert!(output.contains("Bitbucket : collecte en "), "{output}");
    assert!(output.contains("cache : 0 hits, 0 miss"), "{output}");
}

#[test]
fn collects_prs_concurrently_with_at_most_four_in_flight_and_stable_output() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;
    let mut server = Server::new();
    repository(
        &mut server,
        (1..=12)
            .map(|id| pr(id, Some("{alice}"), "2026-10-02T00:00:00Z", "OPEN"))
            .collect(),
        vec![],
    );
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut mocks = Vec::new();
    for id in 1..=12 {
        for kind in ["activity", "comments", "diffstat"] {
            let active = active.clone();
            let peak = peak.clone();
            mocks.push(
                server
                    .mock(
                        "GET",
                        format!("/2.0/repositories/team/project/pullrequests/{id}/{kind}").as_str(),
                    )
                    .match_query(Matcher::Any)
                    .with_chunked_body(move |writer| {
                        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(count, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(30));
                        active.fetch_sub(1, Ordering::SeqCst);
                        writer.write_all(b"{\"values\":[]}")
                    })
                    .expect(1)
                    .create(),
            );
        }
    }
    let activity = source(&server).activity(None, period()).unwrap();
    assert_eq!(
        activity
            .pull_requests
            .iter()
            .map(|pr| pr.number)
            .collect::<Vec<_>>(),
        (1..=12).collect::<Vec<_>>()
    );
    assert!(
        (2..=4).contains(&peak.load(Ordering::SeqCst)),
        "peak={}",
        peak.load(Ordering::SeqCst)
    );
    for mock in mocks {
        mock.assert();
    }
}

#[test]
fn excludes_stale_prs_before_fetching_history_and_requests_a_server_date_filter() {
    let mut server = Server::new();
    endpoint(&mut server, "/2.0/user", user("{alice}"));
    endpoint(
        &mut server,
        "/2.0/repositories/team",
        json!({"values":[{"full_name":"team/project"}]}),
    );
    let mut stale = pr(7, Some("{alice}"), "2026-01-01T00:00:00Z", "OPEN");
    stale["updated_on"] = json!("2026-09-30T23:59:59Z");
    let prs = server
        .mock("GET", "/2.0/repositories/team/project/pullrequests")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("q".into(), "updated_on>=\"2026-10-01T00:00:00Z\"".into()),
            Matcher::UrlEncoded("pagelen".into(), "50".into()),
            Matcher::Regex("fields=.*updated_on".into()),
        ]))
        .with_body(json!({"values":[stale]}).to_string())
        .expect(1)
        .create();
    let forbidden = ["activity", "comments", "diffstat"].map(|kind| {
        server
            .mock(
                "GET",
                format!("/2.0/repositories/team/project/pullrequests/7/{kind}").as_str(),
            )
            .match_query(Matcher::Any)
            .expect(0)
            .create()
    });
    endpoint(
        &mut server,
        "/2.0/repositories/team/project/commits",
        json!({"values":[]}),
    );
    assert!(
        source(&server)
            .activity(None, period())
            .unwrap()
            .pull_requests
            .is_empty()
    );
    prs.assert();
    for mock in forbidden {
        mock.assert();
    }
}

#[test]
fn authenticated_requests_preserve_credentials_when_retried_after_rate_limit() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::UrlEncoded("fields".into(), "uuid".into()))
        .match_header("authorization", "Bearer test-token")
        .with_status(429)
        .with_header("retry-after", "0")
        .expect(1)
        .create();
    let success = endpoint(&mut server, "/2.0/user", user("{alice}"));
    endpoint(&mut server, "/2.0/repositories/team", json!({"values":[]}));
    let log = tempfile::NamedTempFile::new().unwrap();
    let progress = dev_metrics::progress::Progress::new(log.reopen().unwrap(), true, false, false);
    assert_eq!(
        source(&server)
            .with_progress(progress)
            .activity(None, period())
            .unwrap()
            .user,
        "{alice}"
    );
    limited.assert();
    success.assert();
    assert!(
        std::fs::read_to_string(log.path())
            .unwrap()
            .contains("Bitbucket · rate limit, reprise dans")
    );
}

#[test]
fn progress_tracks_repositories_and_opaque_pages_without_changing_json_or_exposing_secrets() {
    let mut server = Server::new();
    endpoint(&mut server, "/2.0/user", user("{alice}"));
    server
        .mock("GET", "/2.0/repositories/team")
        .match_query(Matcher::UrlEncoded("pagelen".into(), "100".into()))
        .with_body(
            json!({"values":[{"full_name":"team/first"}],"next":"?cursor=test-token","size":2})
                .to_string(),
        )
        .create();
    server
        .mock("GET", "/2.0/repositories/team")
        .match_query(Matcher::UrlEncoded("cursor".into(), "test-token".into()))
        .with_header("x-ratelimit-limit", "1000")
        .with_header("x-ratelimit-nearlimit", "true")
        .with_body(json!({"values":[{"full_name":"team/second"}],"size":2}).to_string())
        .create();
    for repository in ["first", "second"] {
        for operation in ["pullrequests", "commits"] {
            endpoint(
                &mut server,
                &format!("/2.0/repositories/team/{repository}/{operation}"),
                json!({"values":[]}),
            );
        }
    }
    let baseline =
        serde_json::to_string(&source(&server).activity(None, period()).unwrap()).unwrap();
    for (terminal, quiet, verbose) in [
        (true, false, false),
        (true, false, true),
        (false, false, true),
        (true, true, true),
    ] {
        let log = tempfile::NamedTempFile::new().unwrap();
        let progress =
            dev_metrics::progress::Progress::new(log.reopen().unwrap(), terminal, quiet, verbose);
        let activity = source(&server)
            .with_progress(progress)
            .activity(None, period())
            .unwrap();
        assert_eq!(serde_json::to_string(&activity).unwrap(), baseline);
        let output = std::fs::read_to_string(log.path()).unwrap();
        if !terminal || quiet {
            assert!(output.is_empty());
            continue;
        }
        for expected in [
            "Bitbucket · collecte",
            "PRs 1/2 dépôts",
            "PRs 2/2 dépôts",
            "commits 2/2 dépôts",
        ] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
        assert_eq!(output.contains("page 2"), verbose);
        assert_eq!(output.contains("GET"), verbose);
        assert_eq!(output.contains("limite 1000"), verbose);
        assert_eq!(output.contains("quota presque épuisé"), verbose);
        assert!(!output.contains("test-token"));
        assert!(!output.contains("cursor"));
    }
}

fn query_pairs(query: &str) -> Matcher {
    Matcher::AllOf(
        query
            .split('&')
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap();
                if key == "state" {
                    Matcher::Regex(format!("(^|&)state={value}(&|$)"))
                } else {
                    Matcher::UrlEncoded(key.into(), value.into())
                }
            })
            .collect(),
    )
}

fn period() -> Period {
    Period::parse("2026-10-01", "2026-10-07").unwrap()
}

fn endpoint(server: &mut Server, path: &str, body: Value) -> mockito::Mock {
    server
        .mock("GET", path)
        .match_query(Matcher::Any)
        .match_header("authorization", "Bearer test-token")
        .match_header("accept", "application/json")
        .match_header("user-agent", "dev-metrics")
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create()
}

fn user(id: &str) -> Value {
    json!({"uuid": id, "nickname": "same-display-name"})
}

#[test]
fn resolves_authenticated_identity_and_returns_versioned_empty_output() {
    let mut server = Server::new();
    let identity = endpoint(&mut server, "/2.0/user", user("{alice}"));
    let repositories = endpoint(&mut server, "/2.0/repositories/team", json!({"values": []}));
    let activity = source(&server).activity(None, period()).unwrap();
    assert_eq!(activity.user, "{alice}");
    assert_eq!(activity.source, Source::Bitbucket);
    assert_eq!(activity.schema_version, 1);
    assert!(activity.pull_requests.is_empty());
    assert!(activity.table().starts_with("Bitbucket · {alice}"));
    assert!(activity.table().ends_with("Aucune activité."));
    identity.assert();
    repositories.assert();
}

#[test]
fn authenticates_with_atlassian_email_when_configured() {
    let mut server = Server::new();
    let authorization = "Basic YWxpY2VAZXhhbXBsZS50ZXN0OmZha2UtYml0YnVja2V0LXRva2Vu";
    let identity = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::UrlEncoded("fields".into(), "uuid".into()))
        .match_header("authorization", authorization)
        .with_header("content-type", "application/json")
        .with_body(user("{alice}").to_string())
        .create();
    let repositories = server
        .mock("GET", "/2.0/repositories/team")
        .match_query(Matcher::Any)
        .match_header("authorization", authorization)
        .with_header("content-type", "application/json")
        .with_body(json!({"values": []}).to_string())
        .create();
    let source = BitbucketSource::new_with_email(
        format!("{}/2.0/", server.url()).parse().unwrap(),
        "fake-bitbucket-token",
        "alice@example.test",
        "team",
    )
    .unwrap();
    assert_eq!(source.activity(None, period()).unwrap().user, "{alice}");
    identity.assert();
    repositories.assert();
}

#[test]
fn basic_auth_survives_rate_limit_retry_and_redirect_with_progress() {
    let mut server = Server::new();
    let authorization = "Basic YWxpY2VAZXhhbXBsZS50ZXN0OmZha2UtYml0YnVja2V0LXRva2Vu";
    let limited = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::UrlEncoded("fields".into(), "uuid".into()))
        .match_header("authorization", authorization)
        .with_status(429)
        .with_header("retry-after", "0")
        .expect(1)
        .create();
    let redirect = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::UrlEncoded("fields".into(), "uuid".into()))
        .match_header("authorization", authorization)
        .with_status(302)
        .with_header("location", "/2.0/identity")
        .expect(1)
        .create();
    let identity = server
        .mock("GET", "/2.0/identity")
        .match_query(Matcher::UrlEncoded("fields".into(), "uuid".into()))
        .match_header("authorization", authorization)
        .with_header("content-type", "application/json")
        .with_body(user("{alice}").to_string())
        .create();
    let repositories = server
        .mock("GET", "/2.0/repositories/team")
        .match_query(Matcher::Any)
        .match_header("authorization", authorization)
        .with_header("content-type", "application/json")
        .with_body(json!({"values":[]}).to_string())
        .create();
    let log = tempfile::NamedTempFile::new().unwrap();
    let progress = dev_metrics::progress::Progress::new(log.reopen().unwrap(), true, false, true);
    let source = BitbucketSource::new_with_email(
        format!("{}/2.0/", server.url()).parse().unwrap(),
        "fake-bitbucket-token",
        "alice@example.test",
        "team",
    )
    .unwrap()
    .with_progress(progress);
    assert_eq!(source.activity(None, period()).unwrap().user, "{alice}");
    limited.assert();
    redirect.assert();
    identity.assert();
    repositories.assert();
    let output = std::fs::read_to_string(log.path()).unwrap();
    assert!(output.contains("Bitbucket · rate limit, reprise dans"));
    for secret in ["fake-bitbucket-token", "alice@example.test", authorization] {
        assert!(!output.contains(secret));
    }
}

#[test]
fn refuses_http_and_malformed_responses_without_echoing_secrets() {
    for (status, body) in [
        (401, "test-token"),
        (403, "test-token"),
        (429, "test-token"),
        (500, "test-token"),
        (200, "test-token"),
        (200, "{}"),
        (200, "{\"uuid\":42}"),
    ] {
        let mut server = Server::new();
        server
            .mock("GET", "/2.0/user")
            .match_query(Matcher::UrlEncoded("fields".into(), "uuid".into()))
            .with_status(status)
            .with_body(body)
            .create();
        let error = source(&server)
            .activity(None, period())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("test-token"), "{error}");
    }
}

#[test]
fn follows_opaque_next_links_and_rejects_incomplete_or_cyclic_pages() {
    let mut server = Server::new();
    endpoint(&mut server, "/2.0/user", user("{alice}"));
    let next = format!("{}/2.0/repositories/team?cursor=opaque", server.url());
    server
        .mock("GET", "/2.0/repositories/team")
        .match_query(Matcher::UrlEncoded("pagelen".into(), "100".into()))
        .with_body(
            json!({"values": [{"full_name":"team/project"}], "next":next, "size":1}).to_string(),
        )
        .create();
    endpoint(
        &mut server,
        "/2.0/repositories/team",
        json!({"values":[], "size":1}),
    );
    endpoint(
        &mut server,
        "/2.0/repositories/team/project/pullrequests",
        json!({"values":[]}),
    );
    endpoint(
        &mut server,
        "/2.0/repositories/team/project/commits",
        json!({"values":[]}),
    );
    assert!(source(&server).activity(None, period()).is_ok());
    for page in [
        json!({"values":[], "next":next}),
        json!({"values":[], "size":1}),
        json!({"values":[],"next":"https://evil.example/2.0/x"}),
    ] {
        let mut server = Server::new();
        endpoint(&mut server, "/2.0/user", user("{alice}"));
        endpoint(&mut server, "/2.0/repositories/team", page);
        assert!(source(&server).activity(None, period()).is_err());
    }
}

fn pr(number: u64, author: Option<&str>, created: &str, state: &str) -> Value {
    json!({"id":number,"author":author.map(user),"title":format!("PR {number}"),"state":state,
        "created_on":created,"updated_on":"2026-10-12T00:00:00Z",
        "links":{"html":{"href":format!("https://bitbucket.org/team/project/pull-requests/{number}")}}})
}

fn comment(id: u64, author: Option<&str>, date: &str, inline: bool) -> Value {
    let mut value = json!({"id":id,"user":author.map(user),"created_on":date,"deleted":false,
        "links":{"html":{"href":format!("https://bitbucket.org/team/project/pull-requests/7#comment-{id}")}}});
    if inline {
        value["inline"] = json!({"path":"src/lib.rs","to":1});
    }
    value
}

fn approval(author: Option<&str>, date: &str) -> Value {
    json!({"approval":{"date":date,"user":author.map(user)}})
}

fn diff(path: Option<&str>, old: Option<&str>, added: u64, removed: u64) -> Value {
    json!({"new":path.map(|path| json!({"path":path})),"old":old.map(|path| json!({"path":path})),
        "lines_added":added,"lines_removed":removed})
}

fn repository(server: &mut Server, prs: Vec<Value>, commits: Vec<Value>) {
    endpoint(server, "/2.0/user", user("{alice}"));
    endpoint(
        server,
        "/2.0/repositories/team",
        json!({"values":[{"full_name":"team/project"}]}),
    );
    server
        .mock("GET", "/2.0/repositories/team/project/pullrequests")
        .match_query(query_pairs(
            "pagelen=50&state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED",
        ))
        .with_body(json!({"values":prs}).to_string())
        .create();
    endpoint(
        server,
        "/2.0/repositories/team/project/commits",
        json!({"values":commits}),
    );
}

fn pr_history(
    server: &mut Server,
    number: u64,
    events: Vec<Value>,
    comments: Vec<Value>,
    files: Vec<Value>,
) {
    endpoint(
        server,
        &format!("/2.0/repositories/team/project/pullrequests/{number}/activity"),
        json!({"values":events}),
    );
    endpoint(
        server,
        &format!("/2.0/repositories/team/project/pullrequests/{number}/comments"),
        json!({"values":comments}),
    );
    endpoint(
        server,
        &format!("/2.0/repositories/team/project/pullrequests/{number}/diffstat"),
        json!({"values":files}),
    );
}

#[test]
fn collects_authored_merged_and_reviewed_prs_with_historical_dates_and_paths() {
    let mut server = Server::new();
    repository(
        &mut server,
        vec![
            pr(7, Some("{alice}"), "2026-10-01T23:00:00-01:00", "OPEN"),
            pr(8, Some("{alice}"), "2026-09-01T00:00:00Z", "MERGED"),
            pr(9, Some("{bob}"), "2026-09-01T00:00:00Z", "DECLINED"),
            pr(10, Some("{alice}"), "2026-09-01T00:00:00Z", "OPEN"),
        ],
        vec![],
    );
    pr_history(
        &mut server,
        7,
        vec![
            approval(Some("{alice}"), "2026-10-01T00:00:00Z"),
            approval(Some("{bob}"), "2026-10-03T00:00:00Z"),
            approval(Some("{bob}"), "2026-10-04T00:00:00Z"),
        ],
        vec![
            comment(11, Some("{bob}"), "2026-10-02T02:00:00Z", true),
            comment(12, Some("{alice}"), "2026-10-07T23:59:59Z", false),
        ],
        vec![
            diff(Some("src/lib.rs"), Some("src/old.rs"), 12, 4),
            diff(Some("Cargo.lock"), None, 30, 0),
        ],
    );
    pr_history(
        &mut server,
        8,
        vec![
            json!({"update":{"state":"MERGED","date":"2026-10-05T00:00:00Z"}}),
            json!({"update":{"state":"MERGED","date":"2026-10-12T00:00:00Z"}}),
            approval(Some("{bob}"), "2026-09-02T00:00:00Z"),
        ],
        vec![],
        vec![diff(None, Some("gone.rs"), 0, 8)],
    );
    pr_history(
        &mut server,
        9,
        vec![approval(Some("{alice}"), "2026-10-06T00:00:00Z")],
        vec![comment(13, Some("{alice}"), "2026-10-03T00:00:00Z", true)],
        vec![],
    );
    pr_history(
        &mut server,
        10,
        vec![approval(None, "2026-10-07T23:59:59.999999Z")],
        vec![],
        vec![],
    );
    let activity = source(&server).activity(None, period()).unwrap();
    assert_eq!(activity.pull_requests.len(), 4);
    let created = &activity.pull_requests[0];
    assert_eq!(created.created_at.to_rfc3339(), "2026-10-02T00:00:00+00:00");
    assert_eq!(
        created.first_review_received_at.unwrap().to_rfc3339(),
        "2026-10-02T02:00:00+00:00"
    );
    assert_eq!((created.additions, created.deletions), (42, 4));
    assert_eq!(
        created.files[0].previous_path.as_deref(),
        Some("src/old.rs")
    );
    assert_eq!(
        activity.pull_requests[1].merged_at.unwrap().to_rfc3339(),
        "2026-10-05T00:00:00+00:00"
    );
    assert_eq!(activity.pull_requests[1].files[0].path, "gone.rs");
    assert_eq!(
        activity.pull_requests[1]
            .first_review_received_at
            .unwrap()
            .to_rfc3339(),
        "2026-09-02T00:00:00+00:00"
    );
    assert_eq!(activity.reviews_given.len(), 2);
    assert_eq!(activity.reviews_received.len(), 4);
    assert_eq!(activity.comments.len(), 2);
    assert!(activity.reviews_received.last().unwrap().author.is_none());
    let json = serde_json::to_value(&activity).unwrap();
    for key in [
        "pull_requests",
        "reviews_given",
        "reviews_received",
        "comments",
    ] {
        for record in json[key].as_array().unwrap() {
            assert_eq!(record["source"], "bitbucket");
            assert_eq!(record["repository"], "team/project");
        }
    }
    assert!(
        activity
            .table()
            .contains("bitbucket | team/project | première review")
    );
}

fn commit(hash: &str, author: Option<&str>, date: &str) -> Value {
    json!({"hash":hash,"author":{"raw":"Unmapped <unknown@example.com>","user":author.map(user)},
        "date":date,"message":"Improve metrics", "links":{"html":{"href":format!("https://bitbucket.org/team/project/commits/{hash}")}}})
}

#[test]
fn collects_only_attributed_commits_in_period_and_preserves_file_statistics() {
    let mut server = Server::new();
    repository(
        &mut server,
        vec![],
        vec![
            commit("abc123", Some("{alice}"), "2026-10-01T00:00:00Z"),
            commit("def456", Some("{alice}"), "2026-09-30T23:59:59Z"),
            commit("abcd", Some("{bob}"), "2026-10-02T00:00:00Z"),
            commit("aaaa", None, "2026-10-02T00:00:00Z"),
            commit("bbb", Some("{alice}"), "2026-10-08T00:00:00Z"),
        ],
    );
    let diffstat = endpoint(
        &mut server,
        "/2.0/repositories/team/project/diffstat/abc123",
        json!({"values":[
            diff(Some("src/lib.rs"),Some("src/old.rs"),8,3),
            diff(Some("generated/output.js"),None,20,0),
            diff(None,Some("removed.rs"),0,7),
        ]}),
    );
    let activity = source(&server).activity(None, period()).unwrap();
    assert_eq!(activity.commits.len(), 1);
    let record = &activity.commits[0];
    assert_eq!(record.sha, "abc123");
    assert_eq!(record.repository, "team/project");
    assert_eq!(record.source, Source::Bitbucket);
    assert_eq!((record.additions, record.deletions), (28, 10));
    assert_eq!(record.files[1].path, "generated/output.js");
    diffstat.assert();
}

#[test]
fn rejects_unsafe_inputs_before_they_can_select_another_endpoint() {
    let server = Server::new();
    for workspace in [
        "",
        "../user",
        "team?x=1",
        "team/project",
        "team#fragment",
        "%2e%2e",
    ] {
        let result = BitbucketSource::new(
            format!("{}/2.0/", server.url()).parse().unwrap(),
            "test-token",
            workspace,
        );
        assert!(result.is_err(), "accepted {workspace}");
    }
    for user in [
        "../user",
        "alice?token=test-token",
        "team/alice",
        "%2e%2e",
        "",
    ] {
        assert!(
            source(&server)
                .activity(Some(user), period())
                .unwrap_err()
                .to_string()
                .contains("Utilisateur Bitbucket invalide")
        );
    }
}

#[test]
fn refuses_unrecognized_history_and_missing_merge_dates() {
    for (state, events) in [
        ("MERGED", vec![]),
        (
            "OPEN",
            vec![json!({"unknown_review":{"date":"2026-10-03T00:00:00Z"}})],
        ),
        ("OPEN", vec![json!({"approval":{"user":user("{bob}")}})]),
        (
            "OPEN",
            vec![json!({"approval":{"date":"invalid","user":user("{bob}")}})],
        ),
    ] {
        let mut server = Server::new();
        repository(
            &mut server,
            vec![pr(7, Some("{alice}"), "2026-10-01T00:00:00Z", state)],
            vec![],
        );
        pr_history(&mut server, 7, events, vec![], vec![]);
        assert!(source(&server).activity(None, period()).is_err());
    }
}

#[test]
fn rejects_duplicate_entities_unsafe_repositories_and_commit_hashes() {
    for repositories in [
        json!([{"full_name":"team/project"},{"full_name":"team/project"}]),
        json!([{"full_name":"other/project"}]),
        json!([{"full_name":"team/../user"}]),
    ] {
        let mut server = Server::new();
        endpoint(&mut server, "/2.0/user", user("{alice}"));
        endpoint(
            &mut server,
            "/2.0/repositories/team",
            json!({"values":repositories}),
        );
        endpoint(
            &mut server,
            "/2.0/repositories/team/project/pullrequests",
            json!({"values":[]}),
        );
        endpoint(
            &mut server,
            "/2.0/repositories/team/project/commits",
            json!({"values":[]}),
        );
        assert!(source(&server).activity(None, period()).is_err());
    }
    let mut server = Server::new();
    repository(
        &mut server,
        vec![],
        vec![
            commit("abc123", Some("{alice}"), "2026-10-02T00:00:00Z"),
            commit("abc123", Some("{alice}"), "2026-10-02T00:00:00Z"),
        ],
    );
    endpoint(
        &mut server,
        "/2.0/repositories/team/project/diffstat/abc123",
        json!({"values":[]}),
    );
    assert!(source(&server).activity(None, period()).is_err());
    let mut server = Server::new();
    repository(
        &mut server,
        vec![],
        vec![commit("../user", Some("{alice}"), "2026-10-02T00:00:00Z")],
    );
    assert!(
        source(&server)
            .activity(None, period())
            .unwrap_err()
            .to_string()
            .contains("Hash Bitbucket invalide")
    );
}

#[test]
fn paginates_prs_history_comments_commits_and_diffstats_through_a_redirect() {
    let mut server = Server::new();
    endpoint(&mut server, "/2.0/users/712020:alice", user("{alice}"));
    endpoint(
        &mut server,
        "/2.0/repositories/team",
        json!({"values":[{"full_name":"team/project"}]}),
    );
    for (path, query, first, second) in [
        (
            "pullrequests",
            "pagelen=50&state=OPEN&state=MERGED&state=DECLINED&state=SUPERSEDED",
            pr(7, Some("{alice}"), "2026-10-01T00:00:00Z", "OPEN"),
            pr(8, Some("{bob}"), "2026-09-01T00:00:00Z", "OPEN"),
        ),
        (
            "pullrequests/7/activity",
            "pagelen=100",
            approval(Some("{bob}"), "2026-10-02T00:00:00Z"),
            json!({"changes_requested":{"user":user("{bob}"),"date":"2026-10-03T00:00:00Z"}}),
        ),
        (
            "pullrequests/7/comments",
            "pagelen=100",
            comment(11, Some("{alice}"), "2026-10-04T00:00:00Z", false),
            comment(12, Some("{alice}"), "2026-10-05T00:00:00Z", true),
        ),
        (
            "commits",
            "pagelen=100",
            commit("abc", Some("{alice}"), "2026-10-02T00:00:00Z"),
            commit("def", Some("{alice}"), "2026-10-03T00:00:00Z"),
        ),
        (
            "diffstat/abc",
            "pagelen=100",
            diff(Some("a.rs"), None, 2, 0),
            diff(Some("b.rs"), None, 3, 1),
        ),
    ] {
        let path = format!("/2.0/repositories/team/project/{path}");
        let next = format!("{}{path}?cursor=opaque", server.url());
        server
            .mock("GET", path.as_str())
            .match_query(query_pairs(query))
            .with_body(json!({"values":[first],"next":next}).to_string())
            .create();
        server
            .mock("GET", path.as_str())
            .match_query(query_pairs("cursor=opaque"))
            .with_body(json!({"values":[second]}).to_string())
            .create();
    }
    pr_history(&mut server, 8, vec![], vec![], vec![]);
    server
        .mock(
            "GET",
            "/2.0/repositories/team/project/pullrequests/7/diffstat",
        )
        .match_query(Matcher::Any)
        .with_status(302)
        .with_header(
            "location",
            "/2.0/repositories/team/project/diffstat/abc?pagelen=100",
        )
        .create();
    endpoint(
        &mut server,
        "/2.0/repositories/team/project/diffstat/def",
        json!({"values":[]}),
    );
    let activity = source(&server)
        .activity(Some("712020:alice"), period())
        .unwrap();
    assert_eq!(activity.pull_requests.len(), 1);
    assert_eq!(activity.pull_requests[0].files.len(), 2);
    assert_eq!(
        (
            activity.pull_requests[0].additions,
            activity.pull_requests[0].deletions
        ),
        (5, 1)
    );
    assert_eq!(activity.reviews_received.len(), 2);
    assert_eq!(
        serde_json::to_value(&activity.reviews_received[1]).unwrap()["state"],
        "CHANGES_REQUESTED"
    );
    assert_eq!(activity.comments.len(), 2);
    assert_eq!(activity.commits.len(), 2);
    assert_eq!(activity.commits[0].files.len(), 2);
}

#[test]
fn refuses_empty_paths_duplicate_files_and_overflowing_totals() {
    for files in [
        vec![diff(None, None, 1, 0)],
        vec![diff(Some(""), None, 1, 0)],
        vec![
            diff(Some("a.rs"), None, 1, 0),
            diff(Some("a.rs"), None, 1, 0),
        ],
        vec![
            diff(Some("a.rs"), None, u64::MAX, 0),
            diff(Some("b.rs"), None, 1, 0),
        ],
        vec![
            diff(Some("a.rs"), None, 0, u64::MAX),
            diff(Some("b.rs"), None, 0, 1),
        ],
    ] {
        let mut server = Server::new();
        repository(
            &mut server,
            vec![],
            vec![commit("abc", Some("{alice}"), "2026-10-02T00:00:00Z")],
        );
        endpoint(
            &mut server,
            "/2.0/repositories/team/project/diffstat/abc",
            json!({"values":files}),
        );
        assert!(source(&server).activity(None, period()).is_err());
    }
}

#[test]
fn never_sends_credentials_to_foreign_or_credentialed_links() {
    let mut foreign = Server::new();
    let forbidden = foreign
        .mock("GET", "/2.0/steal")
        .match_query(Matcher::Any)
        .expect(0)
        .create();
    for redirect in [false, true] {
        for case in 0..16 {
            let mut server = Server::new();
            let base = server.url();
            let host = base.strip_prefix("http://").unwrap();
            let target = match case {
                0 => format!("{}/2.0/steal", foreign.url()),
                1 => format!("http://username@{host}/2.0/steal"),
                2 => format!("http://:password@{host}/2.0/steal"),
                3 => "//evil.example/2.0/steal".into(),
                4 => "/2.0/%2e%2e/steal".into(),
                5 => "/2.0/.%2e/steal".into(),
                6 => "/2.0/%2e./steal".into(),
                7 => "/2.0/..\\steal".into(),
                8 => "\\\\evil.example\\2.0\\steal".into(),
                9 => "/2.0evil/steal".into(),
                10 => "/2.0/steal#".into(),
                11 => "/2.0/steal#test-token".into(),
                12 => "ftp://127.0.0.1/2.0/steal".into(),
                13 => "/other-api/steal".into(),
                14 => "/2.0/../other-api/steal".into(),
                _ => "https://evil.example/2.0/steal".into(),
            };
            endpoint(&mut server, "/2.0/user", user("{alice}"));
            let mock = server
                .mock("GET", "/2.0/repositories/team")
                .match_query(Matcher::Any);
            if redirect {
                mock.with_status(302)
                    .with_header("location", &target)
                    .create();
            } else {
                mock.with_body(
                    json!({"values":[{"full_name":"team/project"}],"next":target}).to_string(),
                )
                .create();
            }
            let error = source(&server)
                .activity(None, period())
                .unwrap_err()
                .to_string();
            assert!(error.contains("hors de l'API autorisée"), "{error}");
        }
    }
    forbidden.assert();
}

#[test]
fn refuses_cycles_changing_totals_and_truncated_pages() {
    for (first_size, second_size, next_again) in [
        (Some(2), Some(3), false),
        (Some(3), Some(3), false),
        (None, None, true),
    ] {
        let mut server = Server::new();
        endpoint(&mut server, "/2.0/user", user("{alice}"));
        let base = format!("{}/2.0/repositories/team", server.url());
        server.mock("GET","/2.0/repositories/team").match_query(Matcher::AllOf(vec![query_pairs("pagelen=100"), Matcher::Regex("^fields=".into())]))
            .with_body(json!({"values":[{"full_name":"team/project"}],"next":format!("{base}?cursor=opaque"),"size":first_size}).to_string()).create();
        server.mock("GET","/2.0/repositories/team").match_query(query_pairs("cursor=opaque"))
            .with_body(json!({"values":[{"full_name":"team/second"}],"next":if next_again {Some(format!("{base}?pagelen=100"))} else {None},"size":second_size}).to_string()).create();
        assert!(source(&server).activity(None, period()).is_err());
    }
}

#[test]
fn outputs_match_snapshots_and_the_shared_versioned_schema() {
    let mut server = Server::new();
    repository(
        &mut server,
        vec![pr(7, Some("{alice}"), "2026-10-01T00:00:00Z", "OPEN")],
        vec![commit("abc123", Some("{alice}"), "2026-10-03T00:00:00Z")],
    );
    pr_history(
        &mut server,
        7,
        vec![approval(Some("{bob}"), "2026-10-02T00:00:00Z")],
        vec![comment(11, Some("{alice}"), "2026-10-04T00:00:00Z", true)],
        vec![diff(Some("src/lib.rs"), None, 8, 2)],
    );
    endpoint(
        &mut server,
        "/2.0/repositories/team/project/diffstat/abc123",
        json!({"values":[diff(Some("src/lib.rs"),None,8,2)]}),
    );
    let activity = source(&server).activity(None, period()).unwrap();
    let common: Value =
        serde_json::from_str(include_str!("../schemas/activity-v1.schema.json")).unwrap();
    let wrapper: Value =
        serde_json::from_str(include_str!("../schemas/bitbucket-activity-v1.schema.json")).unwrap();
    let registry = jsonschema::Registry::new().add("https://github.com/SebastienElet/dev-metrics/blob/main/schemas/activity-v1.schema.json", common).unwrap().prepare().unwrap();
    let validator = jsonschema::draft202012::options()
        .should_validate_formats(true)
        .with_registry(&registry)
        .build(&wrapper)
        .unwrap();
    let value = serde_json::to_value(&activity).unwrap();
    validator.validate(&value).unwrap();
    for invalid in [
        {
            let mut v = value.clone();
            v["source"] = json!("github");
            v
        },
        {
            let mut v = value.clone();
            v["pull_requests"][0]["source"] = json!("github");
            v
        },
        {
            let mut v = value.clone();
            v["schema_version"] = json!(2);
            v
        },
        {
            let mut v = value.clone();
            v["commits"][0]["files"][0]["path"] = json!(null);
            v
        },
        {
            let mut v = value.clone();
            v["reviews_received"][0]["submitted_at"] = json!("invalid");
            v
        },
        {
            let mut v = value.clone();
            v["unexpected"] = json!(true);
            v
        },
    ] {
        assert!(!validator.is_valid(&invalid));
    }
    assert_eq!(
        serde_json::to_string_pretty(&activity).unwrap(),
        include_str!("fixtures/bitbucket-activity.json").trim_end()
    );
    assert_eq!(
        activity.table(),
        include_str!("fixtures/bitbucket-activity.table").trim_end()
    );
}

#[test]
fn token_environment_overrides_configuration_and_missing_or_invalid_values_fail() {
    use dev_metrics::bitbucket::{resolve_token, resolve_workspace};
    assert_eq!(
        resolve_token(Some("env-token".into()), Some("config-token")).unwrap(),
        "env-token"
    );
    assert_eq!(
        resolve_token(None, Some("config-token")).unwrap(),
        "config-token"
    );
    assert!(resolve_token(None, None).is_err());
    for token in [
        "",
        " ",
        "secret\nvalue",
        "secret\tvalue",
        "secret value",
        "é",
    ] {
        assert!(resolve_token(Some(token.into()), Some("config-token")).is_err());
        assert!(resolve_token(None, Some(token)).is_err());
    }
    assert_eq!(
        resolve_workspace(Some("cli-team"), Some("config-team")).unwrap(),
        "cli-team"
    );
    assert_eq!(
        resolve_workspace(None, Some("config-team")).unwrap(),
        "config-team"
    );
    assert!(resolve_workspace(None, None).is_err());
    assert!(resolve_workspace(Some(""), Some("config-team")).is_err());
}

#[test]
fn excludes_deleted_comments_self_reviews_and_out_of_period_actions() {
    let mut server = Server::new();
    repository(
        &mut server,
        vec![pr(7, Some("{alice}"), "2026-09-01T00:00:00Z", "OPEN")],
        vec![],
    );
    let mut deleted = comment(11, Some("{bob}"), "2026-10-02T00:00:00Z", true);
    deleted["deleted"] = json!(true);
    pr_history(
        &mut server,
        7,
        vec![
            approval(Some("{alice}"), "2026-10-02T00:00:00Z"),
            approval(Some("{bob}"), "2026-09-30T23:59:59Z"),
            approval(Some("{bob}"), "2026-10-08T00:00:00Z"),
            json!({"comment":{"id":11}}),
        ],
        vec![
            deleted,
            comment(12, Some("{alice}"), "2026-10-08T00:00:00Z", false),
        ],
        vec![],
    );
    let activity = source(&server).activity(None, period()).unwrap();
    assert!(activity.pull_requests.is_empty());
    assert!(activity.reviews_received.is_empty());
    assert!(activity.comments.is_empty());
}

#[test]
fn malformed_entity_dates_states_and_statistics_are_errors() {
    for (field, value) in [
        ("created_on", json!("not-a-date")),
        ("updated_on", json!("not-a-date")),
        ("updated_on", json!(null)),
        ("state", json!("UNKNOWN")),
        ("id", json!(-1)),
    ] {
        let mut server = Server::new();
        let mut detail = pr(7, Some("{alice}"), "2026-10-01T00:00:00Z", "OPEN");
        detail[field] = value;
        repository(&mut server, vec![detail], vec![]);
        assert!(source(&server).activity(None, period()).is_err());
    }
    for stats in [
        json!({"new":{"path":"a.rs"},"lines_added":-1,"lines_removed":0}),
        json!({"new":{"path":"a.rs"},"lines_removed":0}),
    ] {
        let mut server = Server::new();
        repository(
            &mut server,
            vec![],
            vec![commit("abc", Some("{alice}"), "2026-10-02T00:00:00Z")],
        );
        endpoint(
            &mut server,
            "/2.0/repositories/team/project/diffstat/abc",
            json!({"values":[stats]}),
        );
        assert!(source(&server).activity(None, period()).is_err());
    }
}

#[test]
fn refuses_redirect_cycles_missing_locations_and_network_failures() {
    for location in [None, Some("/2.0/user")] {
        let mut server = Server::new();
        let mock = server
            .mock("GET", "/2.0/user")
            .match_query(Matcher::UrlEncoded("fields".into(), "uuid".into()))
            .with_status(302);
        if let Some(location) = location {
            mock.with_header("location", location).create();
        } else {
            mock.create();
        }
        assert!(source(&server).activity(None, period()).is_err());
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let connection = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.shutdown(std::net::Shutdown::Both).unwrap();
    });
    let source = BitbucketSource::new(
        format!("http://{address}/2.0/").parse().unwrap(),
        "test-token",
        "team",
    )
    .unwrap();
    let error = source.activity(None, period()).unwrap_err().to_string();
    connection.join().unwrap();
    assert!(error.contains("échec réseau"), "{error}");
}

#[test]
fn http_timeout_fails_the_collection() {
    let mut server = Server::new();
    server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::UrlEncoded("fields".into(), "uuid".into()))
        .with_status_code_from_request(|_| {
            std::thread::sleep(std::time::Duration::from_secs(16));
            200
        })
        .with_body(user("{alice}").to_string())
        .create();
    assert!(
        source(&server)
            .activity(None, period())
            .unwrap_err()
            .to_string()
            .contains("timeout")
    );
}

#[test]
fn resolves_explicit_atlassian_account_id_to_canonical_uuid() {
    let mut server = Server::new();
    let identity = endpoint(
        &mut server,
        "/2.0/users/712020:48fbd099-735d-4e56-901f-770ecc63d8ac",
        user("{alice}"),
    );
    endpoint(&mut server, "/2.0/repositories/team", json!({"values":[]}));
    let activity = source(&server)
        .activity(
            Some("712020:48fbd099-735d-4e56-901f-770ecc63d8ac"),
            period(),
        )
        .unwrap();
    assert_eq!(activity.user, "{alice}");
    identity.assert();
}

#[test]
fn pending_review_comments_do_not_count_or_set_the_first_received_review() {
    let mut server = Server::new();
    repository(
        &mut server,
        vec![
            pr(7, Some("{alice}"), "2026-10-01T00:00:00Z", "OPEN"),
            pr(8, Some("{bob}"), "2026-09-01T00:00:00Z", "OPEN"),
        ],
        vec![],
    );
    let mut received = comment(11, Some("{bob}"), "2026-10-02T00:00:00Z", true);
    received["pending"] = json!(true);
    let mut given = comment(12, Some("{alice}"), "2026-10-03T00:00:00Z", true);
    given["pending"] = json!(true);
    pr_history(
        &mut server,
        7,
        vec![approval(Some("{bob}"), "2026-10-04T00:00:00Z")],
        vec![received],
        vec![],
    );
    pr_history(&mut server, 8, vec![], vec![given], vec![]);
    let activity = source(&server).activity(None, period()).unwrap();
    assert_eq!(activity.pull_requests.len(), 1);
    assert_eq!(
        activity.pull_requests[0]
            .first_review_received_at
            .unwrap()
            .to_rfc3339(),
        "2026-10-04T00:00:00+00:00"
    );
    assert_eq!(activity.reviews_received.len(), 1);
    assert!(activity.reviews_given.is_empty());
    assert!(activity.comments.is_empty());
}

#[test]
fn common_source_collects_metrics_from_the_existing_bitbucket_adapter() {
    use dev_metrics::metrics::{MetricsConfig, SourceRequest, collect};
    let mut server = Server::new();
    repository(
        &mut server,
        vec![pr(7, Some("{alice}"), "2026-10-24T00:00:00Z", "MERGED")],
        vec![commit("abc123", Some("{alice}"), "2026-10-24T00:00:00Z")],
    );
    pr_history(
        &mut server,
        7,
        vec![
            approval(Some("{bob}"), "2026-10-24T12:00:00Z"),
            json!({"update":{"state":"MERGED","date":"2026-10-25T00:00:00Z"}}),
        ],
        vec![comment(12, Some("{alice}"), "2026-10-24T13:00:00Z", true)],
        vec![
            diff(Some("src/lib.rs"), None, 10, 2),
            diff(Some("Cargo.lock"), None, 1000, 0),
        ],
    );
    endpoint(
        &mut server,
        "/2.0/repositories/team/project/diffstat/abc123",
        json!({"values":[diff(Some("src/lib.rs"), None, 10, 2), diff(Some("Cargo.lock"), None, 1000, 0)]}),
    );
    let source = source(&server);
    let report = collect(
        &[SourceRequest {
            source: &source,
            user: None,
        }],
        chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
        &MetricsConfig::default(),
    )
    .unwrap();
    let week = &report.windows[0].metrics;
    assert_eq!(week.lines_added.current, Some(10.0));
    assert_eq!(week.lines_deleted.current, Some(2.0));
    assert_eq!(week.median_pull_request_lines.current, Some(12.0));
    assert_eq!(week.median_merge_seconds.current, Some(86_400.0));
    assert_eq!(week.median_first_review_seconds.current, Some(43_200.0));
    assert_eq!(week.reviews_received.current, Some(1.0));
    assert_eq!(week.comments_left.current, Some(1.0));
    assert_eq!(report.by_repository[0].source, Source::Bitbucket);
}
