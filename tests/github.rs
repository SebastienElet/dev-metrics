use dev_metrics::github::{CommentKind, GitHubSource, Period, Source};
use mockito::{Matcher, Server};
use serde_json::{Value, json};

fn source(server: &Server) -> GitHubSource {
    GitHubSource::new(server.url().parse().unwrap(), "test-token").unwrap()
}

fn period() -> Period {
    Period::parse("2026-10-01", "2026-10-07").unwrap()
}

fn issue(number: u64, is_pr: bool) -> Value {
    let mut item =
        json!({"number": number, "repository_url": "https://api.github.com/repos/octocat/project"});
    if is_pr {
        item["pull_request"] = json!({});
    }
    item
}

fn endpoint(server: &mut Server, path: &str, body: Value) -> mockito::Mock {
    server
        .mock("GET", path)
        .match_header("authorization", "Bearer test-token")
        .match_header("accept", "application/vnd.github+json")
        .match_header("user-agent", "dev-metrics")
        .expect_at_least(0)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create()
}

fn search(server: &mut Server, path: &str, query: &str, items: Vec<Value>) -> mockito::Mock {
    search_page(server, path, query, 1, items.len(), items)
}

fn search_page(
    server: &mut Server,
    path: &str,
    query: &str,
    page: usize,
    total: usize,
    items: Vec<Value>,
) -> mockito::Mock {
    server
        .mock("GET", path)
        .match_header("authorization", "Bearer test-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("q".into(), query.into()),
            Matcher::UrlEncoded("per_page".into(), "100".into()),
            Matcher::UrlEncoded("page".into(), page.to_string()),
        ]))
        .expect_at_least(0)
        .with_header("content-type", "application/json")
        .with_body(
            json!({"total_count": total, "incomplete_results": false, "items": items}).to_string(),
        )
        .create()
}

fn file(path: &str, additions: u64, deletions: u64) -> Value {
    json!({"filename": path, "additions": additions, "deletions": deletions})
}

fn pr_detail(
    number: u64,
    author: &str,
    created: &str,
    merged: Option<&str>,
    files: &[Value],
) -> Value {
    json!({
        "number": number, "user": {"login": author}, "title": format!("PR {number}"),
        "html_url": format!("https://github.com/octocat/project/pull/{number}"),
        "created_at": created, "merged_at": merged,
        "additions": files.iter().map(|f| f["additions"].as_u64().unwrap()).sum::<u64>(),
        "deletions": files.iter().map(|f| f["deletions"].as_u64().unwrap()).sum::<u64>(),
        "changed_files": files.len()
    })
}

fn mock_pr(
    server: &mut Server,
    detail: Value,
    files: Vec<Value>,
    mut reviews: Vec<Value>,
    mut discussion: Vec<Value>,
    mut inline: Vec<Value>,
) {
    let n = detail["number"].as_u64().unwrap();
    for record in reviews
        .iter_mut()
        .chain(discussion.iter_mut())
        .chain(inline.iter_mut())
    {
        let url = record["html_url"]
            .as_str()
            .unwrap()
            .replace("/pull/8", &format!("/pull/{n}"));
        record["html_url"] = json!(url);
    }
    endpoint(server, &format!("/repos/octocat/project/pulls/{n}"), detail);
    endpoint(
        server,
        &format!("/repos/octocat/project/pulls/{n}/files?per_page=100&page=1"),
        json!(files),
    );
    endpoint(
        server,
        &format!("/repos/octocat/project/pulls/{n}/reviews?per_page=100&page=1"),
        json!(reviews),
    );
    endpoint(
        server,
        &format!("/repos/octocat/project/issues/{n}/comments?per_page=100&page=1"),
        json!(discussion),
    );
    endpoint(
        server,
        &format!("/repos/octocat/project/pulls/{n}/comments?per_page=100&page=1"),
        json!(inline),
    );
}

fn review(id: u64, author: Option<&str>, submitted: Option<&str>, state: &str) -> Value {
    json!({"id": id, "user": author.map(|login| json!({"login": login})), "submitted_at": submitted, "state": state,
        "html_url": format!("https://github.com/octocat/project/pull/8#pullrequestreview-{id}")})
}

fn comment(id: u64, author: Option<&str>, created: &str) -> Value {
    json!({"id": id, "user": author.map(|login| json!({"login": login})), "created_at": created,
        "html_url": format!("https://github.com/octocat/project/pull/8#issuecomment-{id}")})
}

#[test]
fn collects_file_statistics_reviews_and_comments_with_repository_and_source() {
    let mut server = Server::new();
    search(
        &mut server,
        "/search/issues",
        "is:pr author:octocat updated:>=2026-10-01",
        vec![issue(7, true), issue(8, true)],
    );
    search(
        &mut server,
        "/search/issues",
        "is:pr reviewed-by:octocat updated:>=2026-10-01",
        vec![issue(9, true), issue(8, true)],
    );
    search(
        &mut server,
        "/search/issues",
        "is:pr commenter:octocat updated:>=2026-10-01",
        vec![issue(9, true)],
    );
    search(
        &mut server,
        "/search/issues",
        "is:issue commenter:octocat updated:>=2026-10-01",
        vec![issue(10, false)],
    );
    search(
        &mut server,
        "/search/commits",
        "author:octocat author-date:2026-10-01..2026-10-07",
        vec![json!({"sha": "abc123", "repository": {"full_name": "octocat/project"}})],
    );
    let files = vec![
        file("src/main.rs", 10, 2),
        file("Cargo.lock", 100, 50),
        json!({"filename":"src/new.rs", "previous_filename":"src/old.rs", "additions": 0, "deletions": 0}),
    ];
    mock_pr(
        &mut server,
        pr_detail(7, "octocat", "2026-10-01T00:00:00Z", None, &files),
        files.clone(),
        vec![],
        vec![],
        vec![],
    );
    mock_pr(
        &mut server,
        pr_detail(
            8,
            "octocat",
            "2026-09-01T12:00:00Z",
            Some("2026-10-07T23:59:59Z"),
            &files,
        ),
        files.clone(),
        vec![
            review(
                80,
                Some("octocat"),
                Some("2026-09-02T10:00:00Z"),
                "COMMENTED",
            ),
            review(81, Some("alice"), Some("2026-09-03T10:00:00Z"), "DISMISSED"),
            review(82, Some("alice"), Some("2026-10-04T10:00:00Z"), "APPROVED"),
            review(83, None, Some("2026-10-04T10:00:00Z"), "COMMENTED"),
            review(84, Some("bob"), None, "PENDING"),
        ],
        vec![],
        vec![],
    );
    mock_pr(
        &mut server,
        pr_detail(9, "alice", "2026-08-01T12:00:00Z", None, &files),
        files.clone(),
        vec![
            review(
                90,
                Some("Octocat"),
                Some("2026-10-03T12:00:00Z"),
                "APPROVED",
            ),
            review(
                91,
                Some("octocat"),
                Some("2026-10-07T23:30:00-02:00"),
                "COMMENTED",
            ),
            review(
                92,
                Some("octocat"),
                Some("2026-09-30T23:30:00-02:00"),
                "COMMENTED",
            ),
        ],
        vec![
            comment(900, Some("octocat"), "2026-10-04T12:00:00Z"),
            comment(901, Some("alice"), "2026-10-04T12:00:00Z"),
        ],
        vec![comment(902, Some("octocat"), "2026-10-05T12:00:00Z")],
    );
    endpoint(
        &mut server,
        "/repos/octocat/project/issues/10/comments?per_page=100&page=1",
        json!([
            comment(1000, Some("octocat"), "2026-10-05T12:00:00Z"),
            comment(1001, Some("octocat"), "2026-09-30T12:00:00Z")
        ]),
    );
    endpoint(
        &mut server,
        "/repos/octocat/project/commits/abc123?per_page=100&page=1",
        json!({
            "sha": "abc123", "html_url": "https://github.com/octocat/project/commit/abc123",
            "commit": {"message": "Add source", "author": {"date": "2026-10-02T12:00:00Z"}},
            "stats": {"additions":110,"deletions":52}, "files":files
        }),
    );
    let activity = source(&server).activity("octocat", period()).unwrap();
    assert_eq!(activity.pull_requests.len(), 3);
    assert_eq!(activity.pull_requests[0].number, 7);
    assert_eq!(activity.pull_requests[0].additions, 110);
    assert_eq!(activity.pull_requests[0].files[1].path, "Cargo.lock");
    assert_eq!(
        activity.pull_requests[0].files[2].previous_path.as_deref(),
        Some("src/old.rs")
    );
    assert_eq!(
        activity.pull_requests[1]
            .first_review_received_at
            .unwrap()
            .to_rfc3339(),
        "2026-09-03T10:00:00+00:00"
    );
    assert_eq!(
        activity
            .reviews_received
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [82, 83]
    );
    assert_eq!(
        activity
            .reviews_given
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [90, 92]
    );
    assert_eq!(
        activity.comments.iter().map(|c| c.id).collect::<Vec<_>>(),
        [900, 902, 1000]
    );
    assert_eq!(activity.comments[1].kind, CommentKind::Review);
    assert_eq!(activity.commits[0].sha, "abc123");
    assert_eq!(activity.commits[0].deletions, 52);
    assert_eq!(activity.commits[0].files.len(), 3);
    assert_eq!(activity.commits[0].source, Source::GitHub);
    assert_eq!(activity.reviews_given[0].repository, "octocat/project");
    assert_eq!(activity.reviews_received[0].source, Source::GitHub);
    assert_eq!(activity.comments[0].repository, "octocat/project");
    assert_eq!(activity.schema_version, 1);
    assert_schema(&activity);
}

fn empty_searches(server: &mut Server) {
    for query in [
        "is:pr author:octocat updated:>=2026-10-01",
        "is:pr reviewed-by:octocat updated:>=2026-10-01",
        "is:pr commenter:octocat updated:>=2026-10-01",
        "is:issue commenter:octocat updated:>=2026-10-01",
    ] {
        search(server, "/search/issues", query, vec![]);
    }
    search(
        server,
        "/search/commits",
        "author:octocat author-date:2026-10-01..2026-10-07",
        vec![],
    );
}

#[test]
fn paginates_searches_and_deduplicates_candidates() {
    let mut server = Server::new();
    empty_searches(&mut server);
    let query = "is:pr author:octocat updated:>=2026-10-01";
    search_page(
        &mut server,
        "/search/issues",
        query,
        1,
        2,
        vec![issue(7, true)],
    );
    search_page(
        &mut server,
        "/search/issues",
        query,
        2,
        2,
        vec![issue(8, true)],
    );
    for number in [7, 8] {
        mock_pr(
            &mut server,
            pr_detail(number, "octocat", "2026-10-02T00:00:00Z", None, &[]),
            vec![],
            vec![],
            vec![],
            vec![],
        );
    }
    let result = source(&server).activity("octocat", period()).unwrap();
    assert_eq!(
        result
            .pull_requests
            .iter()
            .map(|p| p.number)
            .collect::<Vec<_>>(),
        [7, 8]
    );
}

#[test]
fn paginates_files_reviews_comments_and_commit_files_without_following_foreign_links() {
    let mut server = Server::new();
    empty_searches(&mut server);
    search(
        &mut server,
        "/search/issues",
        "is:pr author:octocat updated:>=2026-10-01",
        vec![issue(7, true)],
    );
    let files = vec![file("src/a.rs", 5, 1), file("generated/client.rs", 15, 3)];
    mock_pr(
        &mut server,
        pr_detail(7, "octocat", "2026-09-01T00:00:00Z", None, &files),
        files[..1].to_vec(),
        vec![review(
            1,
            Some("alice"),
            Some("2026-10-01T00:00:00Z"),
            "APPROVED",
        )],
        vec![comment(1, Some("octocat"), "2026-10-01T00:00:00Z")],
        vec![],
    );
    for (path, first, second) in [
        ("pulls/7/files", json!([files[0]]), json!([files[1]])),
        (
            "pulls/7/reviews",
            json!([review(
                1,
                Some("alice"),
                Some("2026-10-01T00:00:00Z"),
                "APPROVED"
            )]),
            json!([review(
                2,
                Some("bob"),
                Some("2026-10-02T00:00:00Z"),
                "COMMENTED"
            )]),
        ),
        (
            "issues/7/comments",
            json!([comment(1, Some("octocat"), "2026-10-01T00:00:00Z")]),
            json!([comment(2, Some("octocat"), "2026-10-02T00:00:00Z")]),
        ),
    ] {
        server
            .mock(
                "GET",
                format!("/repos/octocat/project/{path}?per_page=100&page=1").as_str(),
            )
            .with_header(
                "link",
                "<https://foreign.invalid/steal-token?page=2>; rel=\"next\"",
            )
            .with_header("content-type", "application/json")
            .with_body(first.to_string())
            .create();
        endpoint(
            &mut server,
            &format!("/repos/octocat/project/{path}?per_page=100&page=2"),
            second,
        );
    }
    search(
        &mut server,
        "/search/commits",
        "author:octocat author-date:2026-10-01..2026-10-07",
        vec![json!({"sha":"abc123","repository":{"full_name":"octocat/project"}})],
    );
    let detail = json!({"sha":"abc123", "html_url":"https://github.com/octocat/project/commit/abc123", "commit":{"message":"Add files","author":{"date":"2026-10-03T00:00:00Z"}},"stats":{"additions":20,"deletions":4},"files":[files[0]]});
    server
        .mock(
            "GET",
            "/repos/octocat/project/commits/abc123?per_page=100&page=1",
        )
        .with_header("content-type", "application/json")
        .with_header("link", "<https://foreign.invalid/?page=2>; rel=\"next\"")
        .with_body(detail.to_string())
        .create();
    let mut second = detail;
    second["files"] = json!([files[1]]);
    endpoint(
        &mut server,
        "/repos/octocat/project/commits/abc123?per_page=100&page=2",
        second,
    );
    let result = source(&server).activity("octocat", period()).unwrap();
    assert_eq!(result.pull_requests[0].files.len(), 2);
    assert_eq!(result.reviews_received.len(), 2);
    assert_eq!(result.comments.len(), 2);
    assert_eq!(result.commits[0].files.len(), 2);
}

#[test]
fn rejects_incomplete_search_and_the_github_search_limit() {
    for body in [
        json!({"total_count":0,"incomplete_results":true,"items":[]}),
        json!({"total_count":1001,"incomplete_results":false,"items":[]}),
        json!({"total_count":1,"incomplete_results":false,"items":[]}),
    ] {
        let mut server = Server::new();
        empty_searches(&mut server);
        server
            .mock("GET", "/search/issues")
            .match_query(Matcher::Any)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create();
        assert!(
            source(&server).activity("octocat", period()).is_err(),
            "accepted {body}"
        );
    }
}

#[test]
fn rejects_reversed_noncanonical_or_impossible_periods() {
    for (from, to) in [
        ("2026-10-07", "2026-10-01"),
        ("2026-2-01", "2026-10-07"),
        ("2026-02-30", "2026-10-07"),
        ("2026-10-01 is:open", "2026-10-07"),
        ("2026-10-01", "2026-10-07T00:00:00Z"),
    ] {
        assert!(Period::parse(from, to).is_err(), "accepted {from}..{to}");
    }
}

#[test]
fn rejects_missing_or_truncated_file_statistics() {
    for changed_files in [2, 3001] {
        let mut server = Server::new();
        empty_searches(&mut server);
        search(
            &mut server,
            "/search/issues",
            "is:pr author:octocat updated:>=2026-10-01",
            vec![issue(7, true)],
        );
        let files = vec![file("src/main.rs", 1, 0)];
        let mut detail = pr_detail(7, "octocat", "2026-10-01T00:00:00Z", None, &files);
        detail["changed_files"] = json!(changed_files);
        mock_pr(&mut server, detail, files, vec![], vec![], vec![]);
        assert!(
            source(&server).activity("octocat", period()).is_err(),
            "accepted {changed_files} advertised files"
        );
    }
    let mut server = Server::new();
    empty_searches(&mut server);
    search(
        &mut server,
        "/search/commits",
        "author:octocat author-date:2026-10-01..2026-10-07",
        vec![json!({"sha":"abc123","repository":{"full_name":"octocat/project"}})],
    );
    endpoint(
        &mut server,
        "/repos/octocat/project/commits/abc123?per_page=100&page=1",
        json!({"sha":"abc123","html_url":"https://github.com/octocat/project/commit/abc123","commit":{"message":"Truncated","author":{"date":"2026-10-01T00:00:00Z"}},"stats":{"additions":100,"deletions":0},"files":[file("src/main.rs",1,0)]}),
    );
    assert!(source(&server).activity("octocat", period()).is_err());
}

#[test]
fn rejects_unsafe_user_repository_and_commit_paths_before_requests() {
    let server = Server::new();
    for user in [
        "",
        "octocat repo:other/private",
        "octocat\n",
        "../octocat",
        "@me",
    ] {
        let error = source(&server)
            .activity(user, period())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Utilisateur GitHub invalide"), "{error}");
    }
    for repo in [
        "https://evil.invalid/repos/octocat/%2F..%2Fuser",
        "https://api.github.com/repos/octocat/",
        "https://api.github.com/repos/octocat/project/extra",
    ] {
        let mut server = Server::new();
        empty_searches(&mut server);
        search(
            &mut server,
            "/search/issues",
            "is:pr author:octocat updated:>=2026-10-01",
            vec![json!({"number":7,"repository_url":repo,"pull_request":{}})],
        );
        let error = source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Dépôt GitHub invalide"), "{error}");
    }
    let mut server = Server::new();
    empty_searches(&mut server);
    search(
        &mut server,
        "/search/commits",
        "author:octocat author-date:2026-10-01..2026-10-07",
        vec![json!({"sha":"../issues/1/comments","repository":{"full_name":"octocat/project"}})],
    );
    let error = source(&server)
        .activity("octocat", period())
        .unwrap_err()
        .to_string();
    assert!(error.contains("SHA GitHub invalide"), "{error}");
}

#[test]
fn reports_http_malformed_json_and_connection_failures_without_echoing_secrets() {
    for (status, body) in [
        (401, "test-token"),
        (403, "test-token"),
        (429, "test-token"),
        (500, "test-token"),
        (200, "not JSON test-token"),
        (200, "{\"items\":[]}"),
    ] {
        let mut server = Server::new();
        server
            .mock("GET", "/search/issues")
            .match_query(Matcher::Any)
            .with_status(status)
            .with_body(body)
            .create();
        let error = source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("test-token"), "{error}");
        assert!(
            error.contains(if status == 200 { "invalide" } else { "HTTP" }),
            "{error}"
        );
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let client =
        GitHubSource::new(format!("http://{address}").parse().unwrap(), "test-token").unwrap();
    assert!(
        client
            .activity("octocat", period())
            .unwrap_err()
            .to_string()
            .contains("réseau")
    );
}

#[test]
fn empty_output_matches_versioned_json_and_table_snapshots() {
    let mut server = Server::new();
    empty_searches(&mut server);
    let activity = source(&server).activity("octocat", period()).unwrap();
    assert_eq!(
        serde_json::to_string_pretty(&activity).unwrap(),
        include_str!("fixtures/github-empty.json").trim_end()
    );
    assert_eq!(
        activity.table(),
        include_str!("fixtures/github-empty.table").trim_end()
    );
    assert_schema(&activity);
}

#[test]
fn resolves_the_authenticated_user_from_the_api() {
    let mut server = Server::new();
    endpoint(&mut server, "/user", json!({"login":"octocat"}));
    assert_eq!(source(&server).authenticated_user().unwrap(), "octocat");
}

fn assert_schema(activity: &dev_metrics::github::Activity) {
    let schema: Value =
        serde_json::from_str(include_str!("../schemas/github-activity-v1.schema.json")).unwrap();
    let common: Value =
        serde_json::from_str(include_str!("../schemas/activity-v1.schema.json")).unwrap();
    let registry = jsonschema::Registry::new().add("https://github.com/SebastienElet/dev-metrics/blob/main/schemas/activity-v1.schema.json", common).unwrap().prepare().unwrap();
    let validator = jsonschema::draft202012::options()
        .should_validate_formats(true)
        .with_registry(&registry)
        .build(&schema)
        .unwrap();
    let value = serde_json::to_value(activity).unwrap();
    validator.validate(&value).unwrap();
    let mut invalid = value.clone();
    invalid["source"] = json!("bitbucket");
    assert!(!validator.is_valid(&invalid));
    for field in [
        "pull_requests",
        "reviews_given",
        "reviews_received",
        "comments",
        "commits",
    ] {
        if value[field]
            .as_array()
            .is_some_and(|items| !items.is_empty())
        {
            let mut invalid = value.clone();
            invalid[field][0]["repository"] = json!("bad_owner/project");
            assert!(
                !validator.is_valid(&invalid),
                "accepted invalid GitHub owner in {field}"
            );
        }
    }
    if !activity.pull_requests.is_empty() {
        let mut invalid = value.clone();
        invalid["pull_requests"][0]["source"] = json!("bitbucket");
        assert!(!validator.is_valid(&invalid));
        let mut invalid = value;
        invalid["pull_requests"][0]["files"] = json!(null);
        assert!(!validator.is_valid(&invalid));
    }
}

#[test]
fn nonempty_table_matches_snapshot_and_serialized_activity_matches_schema() {
    let mut server = Server::new();
    empty_searches(&mut server);
    search(
        &mut server,
        "/search/issues",
        "is:pr author:octocat updated:>=2026-10-01",
        vec![issue(7, true)],
    );
    let files = vec![file("src/main.rs", 10, 2)];
    mock_pr(
        &mut server,
        pr_detail(
            7,
            "octocat",
            "2026-10-01T00:00:00Z",
            Some("2026-10-02T00:00:00Z"),
            &files,
        ),
        files,
        vec![review(
            80,
            Some("alice"),
            Some("2026-10-01T12:00:00Z"),
            "APPROVED",
        )],
        vec![comment(90, Some("octocat"), "2026-10-01T13:00:00Z")],
        vec![],
    );
    let activity = source(&server).activity("octocat", period()).unwrap();
    assert_eq!(
        activity.table(),
        include_str!("fixtures/github-activity.table").trim_end()
    );
    assert_eq!(
        serde_json::to_string_pretty(&activity).unwrap(),
        include_str!("fixtures/github-activity.json").trim_end()
    );
    assert_schema(&activity);
}

#[test]
fn excludes_self_reviews_and_activity_outside_the_period() {
    let mut server = Server::new();
    empty_searches(&mut server);
    search(
        &mut server,
        "/search/issues",
        "is:pr author:octocat updated:>=2026-10-01",
        vec![issue(7, true)],
    );
    mock_pr(
        &mut server,
        pr_detail(7, "octocat", "2026-09-01T00:00:00Z", None, &[]),
        vec![],
        vec![
            review(
                1,
                Some("octocat"),
                Some("2026-10-01T00:00:00Z"),
                "COMMENTED",
            ),
            review(2, Some("alice"), Some("2026-10-08T00:00:00Z"), "APPROVED"),
        ],
        vec![comment(1, None, "2026-10-01T00:00:00Z")],
        vec![],
    );
    let activity = source(&server).activity("octocat", period()).unwrap();
    assert!(activity.pull_requests.is_empty());
    assert!(activity.reviews_given.is_empty());
    assert!(activity.reviews_received.is_empty());
    assert!(activity.comments.is_empty());
}

#[test]
fn rejects_malformed_review_timestamps_and_unknown_review_states() {
    for (submitted, state) in [
        (Some("not-a-date"), "APPROVED"),
        (None, "APPROVED"),
        (Some("2026-10-01T00:00:00Z"), "UNKNOWN"),
    ] {
        let mut server = Server::new();
        empty_searches(&mut server);
        search(
            &mut server,
            "/search/issues",
            "is:pr author:octocat updated:>=2026-10-01",
            vec![issue(7, true)],
        );
        mock_pr(
            &mut server,
            pr_detail(7, "octocat", "2026-10-01T00:00:00Z", None, &[]),
            vec![],
            vec![review(1, Some("alice"), submitted, state)],
            vec![],
            vec![],
        );
        assert!(source(&server).activity("octocat", period()).is_err());
    }
}

#[test]
fn config_debug_and_parse_errors_do_not_expose_tokens() {
    let config = dev_metrics::config::Config::parse(
        "output_dir='./reports'\n[github]\ntoken='secret-value'",
    )
    .unwrap();
    assert!(!format!("{config:?}").contains("secret-value"));
    let directory = tempfile::tempdir().unwrap();
    let filename = directory.path().join("config.toml");
    std::fs::write(
        &filename,
        "output_dir='./reports'\n[github]\ntoken=['secret-value']",
    )
    .unwrap();
    let error = dev_metrics::config::Config::load(&filename)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("secret-value"), "{error}");
    assert!(error.contains(&filename.display().to_string()));
}

#[test]
fn supports_credentials_that_require_separate_issue_and_pull_request_searches() {
    let mut server = Server::new();
    empty_searches(&mut server);
    server
        .mock("GET", "/search/issues")
        .match_query(Matcher::UrlEncoded(
            "q".into(),
            "commenter:octocat updated:>=2026-10-01".into(),
        ))
        .with_status(422)
        .with_body("Mixed search is not supported for GitHub App user tokens")
        .create();
    search(
        &mut server,
        "/search/issues",
        "is:pr commenter:octocat updated:>=2026-10-01",
        vec![],
    );
    search(
        &mut server,
        "/search/issues",
        "is:issue commenter:octocat updated:>=2026-10-01",
        vec![],
    );
    assert!(source(&server).activity("octocat", period()).is_ok());
}

#[test]
fn refuses_duplicate_results_within_a_search_instead_of_silently_losing_activity() {
    let mut server = Server::new();
    empty_searches(&mut server);
    let item = json!({"sha":"abc123","repository":{"full_name":"octocat/project"}});
    let query = "author:octocat author-date:2026-10-01..2026-10-07";
    search_page(
        &mut server,
        "/search/commits",
        query,
        1,
        2,
        vec![item.clone()],
    );
    search_page(&mut server, "/search/commits", query, 2, 2, vec![item]);
    endpoint(
        &mut server,
        "/repos/octocat/project/commits/abc123?per_page=100&page=1",
        json!({"sha":"abc123","html_url":"https://github.com/octocat/project/commit/abc123","commit":{"message":"One commit","author":{"date":"2026-10-01T00:00:00Z"}},"stats":{"additions":0,"deletions":0},"files":[]}),
    );
    assert!(source(&server).activity("octocat", period()).is_err());
}

#[test]
fn refuses_changed_search_totals_overfull_pages_and_exhausted_pagination() {
    let query = "is:pr author:octocat updated:>=2026-10-01";
    let mut server = Server::new();
    empty_searches(&mut server);
    search_page(
        &mut server,
        "/search/issues",
        query,
        1,
        3,
        vec![issue(7, true)],
    );
    search_page(
        &mut server,
        "/search/issues",
        query,
        2,
        2,
        vec![issue(8, true)],
    );
    assert!(
        source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string()
            .contains("modifiés")
    );
    let mut server = Server::new();
    empty_searches(&mut server);
    search_page(
        &mut server,
        "/search/issues",
        query,
        1,
        1,
        vec![issue(7, true), issue(8, true)],
    );
    assert!(
        source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string()
            .contains("incohérente")
    );
    let mut server = Server::new();
    empty_searches(&mut server);
    for page in 1..=10 {
        search_page(
            &mut server,
            "/search/issues",
            query,
            page,
            11,
            vec![issue(page as u64, true)],
        );
    }
    assert!(
        source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string()
            .contains("incomplète")
    );
}

#[test]
fn refuses_empty_list_pages_with_a_next_link() {
    let mut server = Server::new();
    empty_searches(&mut server);
    search(
        &mut server,
        "/search/issues",
        "is:issue commenter:octocat updated:>=2026-10-01",
        vec![issue(7, false)],
    );
    server
        .mock(
            "GET",
            "/repos/octocat/project/issues/7/comments?per_page=100&page=1",
        )
        .with_header(
            "link",
            "<https://api.github.com/repos/octocat/project/issues/7/comments?page=2>; rel=\"next\"",
        )
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create();
    assert!(
        source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string()
            .contains("vide")
    );
}

fn commit_detail(files: Vec<Value>) -> Value {
    json!({"sha":"abc123","html_url":"https://github.com/octocat/project/commit/abc123","commit":{"message":"Change","author":{"date":"2026-10-01T00:00:00Z"}},"stats":{"additions":0,"deletions":0},"files":files})
}

fn commit_search(server: &mut Server) {
    empty_searches(server);
    search(
        server,
        "/search/commits",
        "author:octocat author-date:2026-10-01..2026-10-07",
        vec![json!({"sha":"abc123","repository":{"full_name":"octocat/project"}})],
    );
}

#[test]
fn refuses_inconsistent_or_empty_commit_pages() {
    for kind in ["sha", "stats", "empty-first", "empty-second"] {
        let mut server = Server::new();
        commit_search(&mut server);
        let first = commit_detail(if kind == "empty-first" {
            vec![]
        } else {
            vec![file("a.rs", 0, 0)]
        });
        server.mock("GET","/repos/octocat/project/commits/abc123?per_page=100&page=1")
            .with_header("link","<https://api.github.com/repos/octocat/project/commits/abc123?page=2>; rel=\"next\"")
            .with_header("content-type","application/json").with_body(first.to_string()).create();
        let mut second = commit_detail(vec![file("b.rs", 0, 0)]);
        match kind {
            "sha" => second["sha"] = json!("def456"),
            "stats" => second["stats"]["additions"] = json!(1),
            "empty-second" => second["files"] = json!([]),
            _ => (),
        }
        endpoint(
            &mut server,
            "/repos/octocat/project/commits/abc123?per_page=100&page=2",
            second,
        );
        assert!(
            source(&server).activity("octocat", period()).is_err(),
            "accepted {kind}"
        );
    }
}

#[test]
fn refuses_commits_at_the_3000_file_cap_even_when_line_totals_are_zero() {
    for final_next in [false, true] {
        let mut server = Server::new();
        commit_search(&mut server);
        for page in 1..=30 {
            let files = ((page - 1) * 100..page * 100)
                .map(|n| file(&format!("binary-{n}.bin"), 0, 0))
                .collect();
            let mut mock = server
                .mock(
                    "GET",
                    format!("/repos/octocat/project/commits/abc123?per_page=100&page={page}")
                        .as_str(),
                )
                .with_header("content-type", "application/json")
                .with_body(commit_detail(files).to_string());
            if page < 30 || final_next {
                mock = mock.with_header("link", "<https://api.github.com/next>; rel=\"next\"");
            }
            mock.create();
        }
        assert!(source(&server).activity("octocat", period()).is_err());
    }
}

#[test]
fn refuses_overflowing_file_totals() {
    let mut server = Server::new();
    commit_search(&mut server);
    endpoint(
        &mut server,
        "/repos/octocat/project/commits/abc123?per_page=100&page=1",
        commit_detail(vec![file("a.rs", u64::MAX, 0), file("b.rs", 1, 0)]),
    );
    assert!(
        source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string()
            .contains("incohérentes")
    );
}

#[test]
fn http_timeout_is_an_error() {
    let mut server = Server::new();
    server
        .mock("GET", "/search/issues")
        .match_query(Matcher::Any)
        .with_status_code_from_request(|_| {
            std::thread::sleep(std::time::Duration::from_secs(16));
            200
        })
        .with_body("{}")
        .create();
    assert!(
        source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string()
            .contains("timeout")
    );
}

#[test]
fn refuses_duplicate_pull_requests_within_one_search() {
    let mut server = Server::new();
    empty_searches(&mut server);
    search(
        &mut server,
        "/search/issues",
        "is:pr author:octocat updated:>=2026-10-01",
        vec![issue(7, true), issue(7, true)],
    );
    assert!(
        source(&server)
            .activity("octocat", period())
            .unwrap_err()
            .to_string()
            .contains("dupliqués")
    );
}

#[test]
fn common_source_collects_metrics_with_explicit_or_authenticated_identity() {
    use dev_metrics::metrics::{MetricsConfig, SourceRequest, collect};
    for user in [Some("octocat"), None] {
        let mut server = Server::new();
        endpoint(&mut server, "/user", json!({"login":"octocat"}));
        for query in [
            "is:pr author:octocat updated:>=2026-09-01",
            "is:pr reviewed-by:octocat updated:>=2026-09-01",
            "is:pr commenter:octocat updated:>=2026-09-01",
            "is:issue commenter:octocat updated:>=2026-09-01",
        ] {
            let candidates = if query.contains("author:") {
                vec![issue(7, true)]
            } else {
                vec![]
            };
            search(&mut server, "/search/issues", query, candidates);
        }
        search(
            &mut server,
            "/search/commits",
            "author:octocat author-date:2026-09-01..2026-10-30",
            vec![],
        );
        let files = vec![file("src/main.rs", 10, 2), file("Cargo.lock", 1000, 0)];
        mock_pr(
            &mut server,
            pr_detail(
                7,
                "octocat",
                "2026-10-24T00:00:00Z",
                Some("2026-10-25T00:00:00Z"),
                &files,
            ),
            files,
            vec![review(
                8,
                Some("alice"),
                Some("2026-10-24T12:00:00Z"),
                "APPROVED",
            )],
            vec![],
            vec![],
        );
        let source = source(&server);
        let report = collect(
            &[SourceRequest {
                source: &source,
                user,
            }],
            chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap(),
            &MetricsConfig::default(),
        )
        .unwrap();
        let week = &report.windows[0].metrics;
        assert_eq!(week.pull_requests_merged.current, Some(1.0));
        assert_eq!(week.median_merge_seconds.current, Some(86_400.0));
        assert_eq!(week.median_first_review_seconds.current, Some(43_200.0));
        assert_eq!(week.median_pull_request_lines.current, Some(12.0));
        assert_eq!(week.reviews_received.current, Some(1.0));
    }
}
