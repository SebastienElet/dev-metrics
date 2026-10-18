use dev_metrics::activity::Period;
use dev_metrics::bitbucket::BitbucketSource;
use mockito::{Matcher, Server};
use serde_json::{Value, json};
use std::time::Duration;

fn source(server: &Server) -> BitbucketSource {
    BitbucketSource::new(
        format!("{}/2.0/", server.url()).parse().unwrap(),
        "secret-token",
        "team",
    )
    .unwrap()
}

fn period() -> Period {
    Period::parse("2026-10-01", "2026-10-07").unwrap()
}

#[test]
fn final_retry_time_includes_later_responses_from_other_workers() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Instant;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, ready_requests) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let started = Instant::now();
        let mut handlers = Vec::new();
        let mut releases = Vec::new();
        while handlers.len() < 4 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let ready = ready.clone();
                    let (release, released) = mpsc::channel();
                    releases.push(release);
                    handlers.push(std::thread::spawn(move || {
                        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                        let mut request = Vec::new();
                        let mut byte = [0];
                        while !request.ends_with(b"\r\n\r\n") {
                            stream.read_exact(&mut byte).unwrap();
                            request.push(byte[0]);
                        }
                        let request = String::from_utf8(request).unwrap();
                        let path = request.split_whitespace().nth(1).unwrap().split('?').next().unwrap();
                        let (status, header, body) = match path {
                            "/2.0/user" => ("200 OK", "", "{\"uuid\":\"{alice}\"}"),
                            "/2.0/repositories/team" => ("200 OK", "", "{\"values\":[{\"full_name\":\"team/first\"},{\"full_name\":\"team/second\"}]}"),
                            "/2.0/repositories/team/first/pullrequests" => {
                                ready.send(()).unwrap();
                                released.recv_timeout(Duration::from_secs(3)).unwrap();
                                ("429 Too Many Requests", "Retry-After: 600\r\n", "")
                            },
                            "/2.0/repositories/team/second/pullrequests" => {
                                ready.send(()).unwrap();
                                released.recv_timeout(Duration::from_secs(3)).unwrap();
                                std::thread::sleep(Duration::from_millis(100));
                                ("429 Too Many Requests", "Retry-After: 1200\r\n", "")
                            },
                            _ => panic!("unexpected request"),
                        };
                        write!(stream,"HTTP/1.1 {status}\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                    }));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(started.elapsed() < Duration::from_secs(3));
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        }
        for _ in 0..2 {
            ready_requests.recv_timeout(Duration::from_secs(3)).unwrap();
        }
        for release in releases {
            let _ = release.send(());
        }
        for handler in handlers {
            handler.join().unwrap();
        }
    });
    let source = BitbucketSource::new(
        format!("http://{address}/2.0/").parse().unwrap(),
        "secret-token",
        "team",
    )
    .unwrap();
    let earliest_retry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 1200;
    let error = source.activity(None, period()).unwrap_err().to_string();
    server.join().unwrap();
    let date = error
        .split("réessayez à ")
        .nth(1)
        .unwrap()
        .split(" UTC")
        .next()
        .unwrap();
    let retry_at = chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M:%S")
        .unwrap()
        .and_utc()
        .timestamp();
    assert!(retry_at >= earliest_retry as i64, "{error}");
}

#[test]
fn rate_limit_headers_do_not_require_a_readable_error_body() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::Any)
        .with_status(403)
        .with_header("retry-after", "0")
        .with_chunked_body(|writer| {
            writer.write_all(b"partial")?;
            std::thread::sleep(Duration::from_millis(50));
            Err(std::io::ErrorKind::ConnectionReset.into())
        })
        .expect(1)
        .create();
    let success = endpoint(&mut server, "/2.0/user", json!({"uuid":"{alice}"}));
    endpoint(&mut server, "/2.0/repositories/team", json!({"values":[]}));
    source(&server).activity(None, period()).unwrap();
    limited.assert();
    success.assert();
}

fn endpoint(server: &mut Server, path: &str, body: Value) -> mockito::Mock {
    server
        .mock("GET", path)
        .match_query(Matcher::Any)
        .with_body(body.to_string())
        .expect(1)
        .create()
}

fn workspace(server: &mut Server) -> Vec<mockito::Mock> {
    vec![
        endpoint(server, "/2.0/user", json!({"uuid":"{alice}"})),
        endpoint(
            server,
            "/2.0/repositories/team",
            json!({"values":[{"full_name":"team/project"}]}),
        ),
        endpoint(
            server,
            "/2.0/repositories/team/project/pullrequests",
            json!({"values":(1..=2).map(|id| json!({
            "id":id,"author":{"uuid":"{alice}"},"title":"Change","state":"OPEN",
            "created_on":"2026-10-02T00:00:00Z","updated_on":"2026-10-03T00:00:00Z",
            "links":{"html":{"href":format!("https://bitbucket.org/team/project/pull-requests/{id}")}}
        })).collect::<Vec<_>>()}),
        ),
        endpoint(
            server,
            "/2.0/repositories/team/project/commits",
            json!({"values":[]}),
        ),
    ]
}

#[test]
fn failed_commit_diffstat_is_a_report_warning_and_other_commits_survive() {
    use dev_metrics::metrics::{MetricsConfig, SourceRequest, collect_period};
    for status in [401, 403, 404, 500] {
        let mut server = Server::new();
        let mut mocks = vec![
            endpoint(&mut server, "/2.0/user", json!({"uuid":"{alice}"})),
            endpoint(
                &mut server,
                "/2.0/repositories/team",
                json!({"values":[{"full_name":"team/project"}]}),
            ),
            endpoint(
                &mut server,
                "/2.0/repositories/team/project/pullrequests",
                json!({"values":[]}),
            ),
            endpoint(
                &mut server,
                "/2.0/repositories/team/project/commits",
                json!({"values":(["abc","def"].map(|hash| json!({
                    "hash":hash,"author":{"user":{"uuid":"{alice}"}},"date":"2026-10-02T00:00:00Z","message":"Change",
                    "links":{"html":{"href":format!("https://bitbucket.org/team/project/commits/{hash}")}}
                })))}),
            ),
            endpoint(
                &mut server,
                "/2.0/repositories/team/project/diffstat/def",
                json!({"values":[{"new":{"path":"a.rs"},"lines_added":7,"lines_removed":2}]}),
            ),
        ];
        let failed = server
            .mock("GET", "/2.0/repositories/team/project/diffstat/abc")
            .match_query(Matcher::Any)
            .with_status(status)
            .with_body("private upstream text secret-token");
        mocks.push(
            if status == 500 {
                failed.with_header("retry-after", "0").expect(3)
            } else {
                failed.expect(1)
            }
            .create(),
        );
        let source = source(&server);
        let report = collect_period(
            &[SourceRequest {
                source: &source,
                user: None,
            }],
            period(),
            &MetricsConfig::default(),
        )
        .unwrap();
        assert_eq!(report.windows[0].metrics.lines_added.current, Some(7.0));
        assert_eq!(report.warnings.len(), 1);
        let json: Value = serde_json::from_str(&report.json().unwrap()).unwrap();
        assert_eq!(
            json["warnings"][0]["subject"],
            json!({"kind":"commit","sha":"abc"})
        );
        assert!(!json.to_string().contains("secret-token"));
        let schema: Value =
            serde_json::from_str(include_str!("../schemas/report-v1.schema.json")).unwrap();
        jsonschema::draft202012::new(&schema)
            .unwrap()
            .validate(&json)
            .unwrap();
        for mock in mocks {
            mock.assert();
        }
    }
}

#[test]
fn item_rate_limit_exhaustion_remains_fatal_instead_of_becoming_a_warning() {
    let mut server = Server::new();
    workspace(&mut server);
    let limited = server
        .mock(
            "GET",
            "/2.0/repositories/team/project/pullrequests/1/activity",
        )
        .match_query(Matcher::Any)
        .with_status(429)
        .with_header("retry-after", "0")
        .expect(3)
        .create();
    for kind in ["activity", "comments", "diffstat"] {
        endpoint(
            &mut server,
            &format!("/2.0/repositories/team/project/pullrequests/2/{kind}"),
            json!({"values":[]}),
        );
    }
    let error = source(&server)
        .activity(None, period())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("limite de débit") && error.contains("réessayez"),
        "{error}"
    );
    limited.assert();
}

#[test]
fn failed_pr_listing_does_not_discard_commits_from_the_same_repository() {
    let mut server = Server::new();
    let identity = endpoint(&mut server, "/2.0/user", json!({"uuid":"{alice}"}));
    let repositories = endpoint(
        &mut server,
        "/2.0/repositories/team",
        json!({"values":[{"full_name":"team/project"}]}),
    );
    let prs = server
        .mock("GET", "/2.0/repositories/team/project/pullrequests")
        .match_query(Matcher::Any)
        .with_status(404)
        .expect(1)
        .create();
    let commits = endpoint(
        &mut server,
        "/2.0/repositories/team/project/commits",
        json!({"values":[{
            "hash":"abc","author":{"user":{"uuid":"{alice}"}},"date":"2026-10-02T00:00:00Z","message":"Change",
            "links":{"html":{"href":"https://bitbucket.org/team/project/commits/abc"}}
        }]}),
    );
    let files = endpoint(
        &mut server,
        "/2.0/repositories/team/project/diffstat/abc",
        json!({"values":[]}),
    );
    let activity = source(&server).activity(None, period()).unwrap();
    assert_eq!(activity.commits.len(), 1);
    assert_eq!(activity.warnings.len(), 1);
    assert_eq!(
        serde_json::to_value(&activity.warnings[0]).unwrap()["subject"],
        json!({"kind":"repository"})
    );
    for mock in [identity, repositories, prs, commits, files] {
        mock.assert();
    }
}

#[test]
fn base_auth_failures_remain_fatal_without_retries() {
    for endpoint_path in ["/2.0/user", "/2.0/repositories/team"] {
        for status in [401, 403] {
            let mut server = Server::new();
            if endpoint_path != "/2.0/user" {
                endpoint(&mut server, "/2.0/user", json!({"uuid":"{alice}"}));
            }
            let denied = server
                .mock("GET", endpoint_path)
                .match_query(Matcher::Any)
                .with_status(status)
                .with_body("Forbidden: secret-token")
                .expect(1)
                .create();
            let error = source(&server)
                .activity(None, period())
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!("HTTP {status}")), "{error}");
            assert!(!error.contains("secret-token"));
            denied.assert();
        }
    }
}

#[test]
fn unavailable_pr_comments_and_diffstats_do_not_leak_partial_activity() {
    for (operation, status, attempts) in [("comments", 404, 1), ("diffstat", 500, 3)] {
        let mut server = Server::new();
        let mut mocks = workspace(&mut server);
        for id in 1..=2 {
            for kind in ["activity", "comments", "diffstat"] {
                let path = format!("/2.0/repositories/team/project/pullrequests/{id}/{kind}");
                if id == 1 && kind == operation {
                    mocks.push(
                        server
                            .mock("GET", path.as_str())
                            .match_query(Matcher::Any)
                            .with_status(status)
                            .with_header("retry-after", "0")
                            .expect(attempts)
                            .create(),
                    );
                    break;
                }
                let body = match kind {
                    "activity" => {
                        json!({"values":[{"approval":{"date":"2026-10-03T00:00:00Z","user":{"uuid":"{bob}"}}}]})
                    }
                    "comments" => {
                        json!({"values":[{"id":id,"created_on":"2026-10-03T00:00:00Z","user":{"uuid":"{alice}"},"deleted":false,"links":{"html":{"href":"https://bitbucket.org/comment"}}}]})
                    }
                    _ => json!({"values":[]}),
                };
                mocks.push(endpoint(&mut server, &path, body));
            }
        }
        let activity = source(&server).activity(None, period()).unwrap();
        assert_eq!(activity.pull_requests.len(), 1);
        assert_eq!(activity.pull_requests[0].number, 2);
        assert!(
            activity
                .comments
                .iter()
                .all(|comment| comment.issue_number == 2)
        );
        assert!(
            activity
                .reviews_received
                .iter()
                .all(|review| review.pull_request_number == 2)
        );
        assert_eq!(activity.warnings.len(), 1);
        for mock in mocks {
            mock.assert();
        }
    }
}

#[test]
fn retries_transient_server_failure_before_retaining_the_item() {
    let mut server = Server::new();
    let mut mocks = workspace(&mut server);
    mocks.push(
        server
            .mock(
                "GET",
                "/2.0/repositories/team/project/pullrequests/1/activity",
            )
            .match_query(Matcher::Any)
            .with_status(503)
            .with_header("retry-after", "0")
            .expect(2)
            .create(),
    );
    for id in 1..=2 {
        for kind in ["activity", "comments", "diffstat"] {
            mocks.push(endpoint(
                &mut server,
                &format!("/2.0/repositories/team/project/pullrequests/{id}/{kind}"),
                json!({"values":[]}),
            ));
        }
    }
    let activity = source(&server).activity(None, period()).unwrap();
    assert_eq!(activity.pull_requests.len(), 2);
    assert!(activity.warnings.is_empty());
    for mock in mocks {
        mock.assert();
    }
}

#[test]
fn retries_rate_limit_403_identified_by_headers_or_body() {
    for signal in [
        "retry-after",
        "x-ratelimit-reset",
        "x-ratelimit-nearlimit",
        "body",
    ] {
        let mut server = Server::new();
        let mock = server
            .mock("GET", "/2.0/user")
            .match_query(Matcher::Any)
            .with_status(403)
            .expect(1);
        let mock = match signal {
            "retry-after" => mock.with_header("retry-after", "0"),
            "x-ratelimit-reset" => mock
                .with_header("x-ratelimit-remaining", "0")
                .with_header("x-ratelimit-reset", "0"),
            "x-ratelimit-nearlimit" => mock.with_header("x-ratelimit-nearlimit", "true"),
            _ => mock.with_body("{\"error\":{\"message\":\"Rate limit exceeded: secret-token\"}}"),
        }
        .create();
        let identity = endpoint(&mut server, "/2.0/user", json!({"uuid":"{alice}"}));
        let repositories = endpoint(&mut server, "/2.0/repositories/team", json!({"values":[]}));
        let activity = source(&server).activity(None, period()).unwrap();
        assert!(activity.warnings.is_empty());
        for mock in [mock, identity, repositories] {
            mock.assert();
        }
    }
}

#[test]
fn long_rate_limit_wait_names_the_retry_time_instead_of_token_permissions() {
    let mut server = Server::new();
    let until = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let limited = server
        .mock("GET", "/2.0/user")
        .match_query(Matcher::Any)
        .with_status(403)
        .with_header("retry-after", "1")
        .with_header("x-ratelimit-remaining", "0")
        .with_header("x-ratelimit-reset", &until.to_string())
        .expect(1)
        .create();
    let error = source(&server)
        .activity(None, period())
        .unwrap_err()
        .to_string();
    assert!(error.contains("limite de débit"), "{error}");
    assert!(
        error.contains("réessayez") && error.contains("UTC"),
        "{error}"
    );
    assert!(
        !error.contains("token") && !error.contains("permissions"),
        "{error}"
    );
    limited.assert();
}

#[test]
fn forbidden_pr_activity_is_skipped_and_reported_without_losing_healthy_prs() {
    let mut server = Server::new();
    let mut mocks = workspace(&mut server);
    mocks.push(
        server
            .mock(
                "GET",
                "/2.0/repositories/team/project/pullrequests/1/activity",
            )
            .match_query(Matcher::Any)
            .with_status(403)
            .with_body("private upstream text secret-token")
            .expect(1)
            .create(),
    );
    for operation in ["activity", "comments", "diffstat"] {
        mocks.push(endpoint(
            &mut server,
            &format!("/2.0/repositories/team/project/pullrequests/2/{operation}"),
            json!({"values":[]}),
        ));
    }
    let forbidden = server
        .mock(
            "GET",
            "/2.0/repositories/team/project/pullrequests/1/diffstat",
        )
        .match_query(Matcher::Any)
        .expect(0)
        .create();
    let activity = source(&server).activity(None, period()).unwrap();
    assert_eq!(
        activity
            .pull_requests
            .iter()
            .map(|pr| pr.number)
            .collect::<Vec<_>>(),
        vec![2]
    );
    let json = serde_json::to_value(&activity).unwrap();
    let schema: Value =
        serde_json::from_str(include_str!("../schemas/activity-v1.schema.json")).unwrap();
    jsonschema::draft202012::new(&schema)
        .unwrap()
        .validate(&json)
        .unwrap();
    assert_eq!(json["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(
        json["warnings"][0]["subject"],
        json!({"kind":"pull_request","number":1})
    );
    assert_eq!(json["warnings"][0]["code"], "ITEM_UNAVAILABLE");
    assert!(
        json["warnings"][0]["message"]
            .as_str()
            .unwrap()
            .contains("HTTP 403")
    );
    assert!(!json.to_string().contains("secret-token"));
    forbidden.assert();
    for mock in mocks {
        mock.assert();
    }
}
