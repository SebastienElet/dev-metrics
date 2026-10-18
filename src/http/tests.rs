use super::*;
use crate::progress::Progress;
use mockito::Server;
use reqwest::blocking::Client;

#[test]
fn bitbucket_retries_throttled_get_once_and_reports_wait_and_remaining_quota() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/resource")
        .with_status(429)
        .with_header("retry-after", "0")
        .with_header("x-ratelimit-remaining", "0")
        .with_body("test-token")
        .expect(1)
        .create();
    let success = server
        .mock("GET", "/resource")
        .with_status(200)
        .with_header("x-ratelimit-remaining", "42")
        .with_body("{}")
        .expect(1)
        .create();
    let log = tempfile::NamedTempFile::new().unwrap();
    let progress = Progress::new(log.reopen().unwrap(), true, false, true);
    let client = Client::new();
    let response = get(
        || client.get(format!("{}/resource", server.url())),
        Source::Bitbucket,
        "ressource",
        &|event| progress.emit(event),
    )
    .unwrap();
    assert_eq!(response.status(), 200);
    limited.assert();
    success.assert();
    let output = std::fs::read_to_string(log.path()).unwrap();
    assert!(output.contains("rate limit, reprise dans 0 s"), "{output}");
    assert!(output.contains("42 restantes"), "{output}");
    assert!(!output.contains("test-token"));
}

#[test]
fn bitbucket_repeated_throttling_stops_after_one_retry() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/resource")
        .with_status(429)
        .with_header("retry-after", "0")
        .expect(2)
        .create();
    let client = Client::new();
    let response = get(
        || client.get(format!("{}/resource", server.url())),
        Source::Bitbucket,
        "ressource",
        &|event| Progress::default().emit(event),
    )
    .unwrap();
    assert_eq!(response.status(), 429);
    limited.assert();
}

#[test]
fn unusable_delays_and_other_errors_do_not_retry_or_echo_headers() {
    for (source, status, delay) in [
        (Source::Bitbucket, 429, None),
        (Source::Bitbucket, 429, Some("test-token")),
        (Source::Bitbucket, 429, Some("-1")),
        (Source::Bitbucket, 429, Some("301")),
        (Source::Bitbucket, 429, Some("18446744073709551616")),
        (Source::Bitbucket, 401, Some("0")),
        (Source::Bitbucket, 500, Some("0")),
        (Source::Bitbucket, 403, Some("0")),
    ] {
        let mut server = Server::new();
        let mut mock = server
            .mock("GET", "/resource")
            .with_status(status)
            .with_header("x-ratelimit-remaining", "test-token")
            .expect(1);
        if let Some(delay) = delay {
            mock = mock.with_header("retry-after", delay);
        }
        let mock = mock.create();
        let log = tempfile::NamedTempFile::new().unwrap();
        let progress = Progress::new(log.reopen().unwrap(), true, false, true);
        let client = Client::new();
        let response = get(
            || client.get(format!("{}/resource", server.url())),
            source,
            "ressource",
            &|event| progress.emit(event),
        )
        .unwrap();
        assert_eq!(usize::from(response.status().as_u16()), status);
        mock.assert();
        let output = std::fs::read_to_string(log.path()).unwrap();
        assert!(!output.contains("test-token"));
        assert!(!output.contains("reprise"));
    }
}

#[test]
fn reset_and_http_dates_wait_until_the_deadline_without_truncating_long_delays() {
    let now: SystemTime = chrono::DateTime::parse_from_rfc3339("2026-10-17T12:00:00Z")
        .unwrap()
        .into();
    let epoch = now.duration_since(UNIX_EPOCH).unwrap().as_secs();
    let mut headers = HeaderMap::new();
    headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
    headers.insert(
        "x-ratelimit-reset",
        (epoch + 42).to_string().parse().unwrap(),
    );
    assert_eq!(retry_delay(&headers, Source::GitHub, now), Some(42));
    assert_eq!(retry_delay(&headers, Source::Bitbucket, now), None);
    headers.insert("retry-after", "3".parse().unwrap());
    assert_eq!(retry_delay(&headers, Source::GitHub, now), Some(42));
    headers.remove("x-ratelimit-reset");
    headers.insert(
        "retry-after",
        "Sat, 17 Oct 2026 12:00:42 GMT".parse().unwrap(),
    );
    assert_eq!(retry_delay(&headers, Source::Bitbucket, now), Some(42));
    headers.insert(
        "retry-after",
        "Sat, 17 Oct 2026 12:10:00 GMT".parse().unwrap(),
    );
    assert_eq!(retry_delay(&headers, Source::Bitbucket, now), None);
    headers.insert(
        "retry-after",
        "Sat, 17 Oct 2026 11:59:00 GMT".parse().unwrap(),
    );
    assert_eq!(retry_delay(&headers, Source::Bitbucket, now), Some(0));
    headers.remove("retry-after");
    for reset in [
        "test-token".into(),
        (epoch + 301).to_string(),
        u64::MAX.to_string(),
    ] {
        headers.insert("x-ratelimit-reset", reset.parse().unwrap());
        assert_eq!(retry_delay(&headers, Source::GitHub, now), None);
    }
    headers.insert(
        "x-ratelimit-reset",
        (epoch + 42).to_string().parse().unwrap(),
    );
    headers.insert("x-ratelimit-remaining", "1".parse().unwrap());
    assert_eq!(retry_delay(&headers, Source::GitHub, now), None);
}

#[test]
fn countdown_reports_remaining_time_and_preserves_waits_when_silent() {
    for terminal in [false, true] {
        let log = tempfile::NamedTempFile::new().unwrap();
        let progress = Progress::new(log.reopen().unwrap(), terminal, false, false);
        let mut elapsed = Duration::ZERO;
        wait_for_retry(
            &|event| progress.emit(event),
            Source::Bitbucket,
            2,
            |duration| elapsed += duration,
        );
        assert_eq!(elapsed, Duration::from_secs(2));
        let output = std::fs::read_to_string(log.path()).unwrap();
        if terminal {
            assert_eq!(
                output,
                "Bitbucket : rate limit, reprise dans 2 s\nBitbucket : rate limit, reprise dans 1 s\nBitbucket : rate limit, reprise dans 0 s\n"
            );
        } else {
            assert!(output.is_empty());
        }
    }
}

#[test]
fn ambiguous_retry_headers_do_not_authorize_an_early_retry() {
    let mut headers = HeaderMap::new();
    headers.append("retry-after", "0".parse().unwrap());
    headers.append("retry-after", "600".parse().unwrap());
    assert_eq!(
        retry_delay(&headers, Source::GitHub, SystemTime::now()),
        None
    );
}

#[test]
fn github_primary_limit_reports_resource_and_reset_without_returning_a_generic_403() {
    let mut server = Server::new();
    let reset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 900;
    let limited = server
        .mock("GET", "/resource")
        .with_status(403)
        .with_header("x-ratelimit-resource", "core")
        .with_header("x-ratelimit-remaining", "0")
        .with_header("x-ratelimit-reset", &reset.to_string())
        .with_body(r#"{"message":"API rate limit exceeded test-token"}"#)
        .expect(1)
        .create();
    let client = Client::new();
    let quota = RateLimit::default();
    let result = send(
        || client.get(format!("{}/resource", server.url())),
        Source::GitHub,
        "/repos/{owner}/{repo}/pulls/{number}",
        "GET",
        &|_| {},
        Some(&quota),
    );
    let error = match result {
        Ok(_) => panic!("primary exhaustion must be a rate-limit error"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("limite de débit"), "{error}");
    assert!(error.contains("core"), "{error}");
    assert!(error.contains(&reset.to_string()), "{error}");
    assert!(!error.contains("test-token"));
    limited.assert();
}

#[test]
fn github_secondary_limit_allows_several_attempts_before_success() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/resource")
        .with_status(429)
        .with_header("retry-after", "0")
        .with_header("x-ratelimit-remaining", "42")
        .with_body(r#"{"message":"secondary rate limit"}"#)
        .expect(3)
        .create();
    let success = server
        .mock("GET", "/resource")
        .with_body("{}")
        .expect(1)
        .create();
    let client = Client::new();
    let quota = RateLimit::new(std::sync::Arc::new(FakeClock::new()));
    let response = send(
        || client.get(format!("{}/resource", server.url())),
        Source::GitHub,
        "/resource",
        "GET",
        &|_| {},
        Some(&quota),
    )
    .unwrap();
    assert_eq!(response.status(), 200);
    limited.assert();
    success.assert();
}

struct FakeClock {
    now: std::sync::Mutex<SystemTime>,
    sleeps: std::sync::Mutex<Vec<Duration>>,
}
impl FakeClock {
    fn new() -> Self {
        Self {
            now: std::sync::Mutex::new(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            sleeps: std::sync::Mutex::default(),
        }
    }
}
impl github::Clock for FakeClock {
    fn now(&self) -> SystemTime {
        *self.now.lock().unwrap()
    }
    fn sleep(&self, delay: Duration) {
        *self.now.lock().unwrap() += delay;
        self.sleeps.lock().unwrap().push(delay);
    }
    fn jitter(&self) -> Duration {
        Duration::from_millis(250)
    }
}

#[test]
fn github_body_only_secondary_limit_uses_backoff_with_jitter_and_exposes_the_deadline() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/resource")
        .with_status(403)
        .with_body(r#"{"message":"You have exceeded a secondary rate limit. test-token"}"#)
        .expect(1)
        .create();
    let success = server
        .mock("GET", "/resource")
        .with_body("{}")
        .expect(1)
        .create();
    let clock = std::sync::Arc::new(FakeClock::new());
    let quota = RateLimit::new(clock.clone());
    let log = tempfile::NamedTempFile::new().unwrap();
    let progress = Progress::new(log.reopen().unwrap(), true, false, true);
    let client = Client::new();
    assert_eq!(
        send(
            || client.get(format!("{}/resource", server.url())),
            Source::GitHub,
            "/resource",
            "GET",
            &|event| progress.emit(event),
            Some(&quota)
        )
        .unwrap()
        .status(),
        200
    );
    assert_eq!(
        clock.sleeps.lock().unwrap().iter().sum::<Duration>(),
        Duration::from_millis(60250)
    );
    let output = std::fs::read_to_string(log.path()).unwrap();
    assert!(output.contains("core"), "{output}");
    assert!(output.contains("1800000060"), "{output}");
    assert!(!output.contains("test-token"));
    limited.assert();
    success.assert();
}

#[test]
fn primary_core_exhaustion_does_not_block_the_graphql_bucket() {
    let mut server = Server::new();
    server
        .mock("GET", "/resource")
        .with_status(403)
        .with_header("x-ratelimit-remaining", "0")
        .with_header("x-ratelimit-reset", "1800000900")
        .with_body("rate limit exceeded")
        .expect(1)
        .create();
    let graph = server
        .mock("POST", "/graphql")
        .with_body(r#"{"data":{}}"#)
        .expect(1)
        .create();
    let quota = RateLimit::new(std::sync::Arc::new(FakeClock::new()));
    let client = Client::new();
    assert!(
        send(
            || client.get(format!("{}/resource", server.url())),
            Source::GitHub,
            "/resource",
            "GET",
            &|_| {},
            Some(&quota)
        )
        .is_err()
    );
    assert_eq!(
        send(
            || client.post(format!("{}/graphql", server.url())),
            Source::GitHub,
            "/graphql",
            "POST",
            &|_| {},
            Some(&quota)
        )
        .unwrap()
        .status(),
        200
    );
    graph.assert();
}

#[test]
fn ordinary_forbidden_does_not_poison_other_requests() {
    let mut server = Server::new();
    server
        .mock("GET", "/forbidden")
        .with_status(403)
        .with_body("Resource not accessible test-token")
        .expect(1)
        .create();
    let success = server
        .mock("GET", "/available")
        .with_body("{}")
        .expect(1)
        .create();
    let quota = RateLimit::default();
    let client = Client::new();
    assert_eq!(
        send(
            || client.get(format!("{}/forbidden", server.url())),
            Source::GitHub,
            "/resource",
            "GET",
            &|_| {},
            Some(&quota)
        )
        .unwrap()
        .status(),
        403
    );
    assert_eq!(
        send(
            || client.get(format!("{}/available", server.url())),
            Source::GitHub,
            "/resource",
            "GET",
            &|_| {},
            Some(&quota)
        )
        .unwrap()
        .status(),
        200
    );
    success.assert();
}

#[test]
fn graphql_rate_errors_on_http_200_are_retried_and_server_errors_have_bounded_retries() {
    for (status, body, expected_wait) in [
        (
            200,
            r#"{"errors":[{"type":"RATE_LIMITED","message":"rate limit"}]}"#,
            Duration::from_millis(60250),
        ),
        (503, "temporarily unavailable", Duration::from_millis(250)),
    ] {
        let mut server = Server::new();
        let first = server
            .mock("POST", "/graphql")
            .with_status(status)
            .with_body(body)
            .expect(1)
            .create();
        let success = server
            .mock("POST", "/graphql")
            .with_body(r#"{"data":{}}"#)
            .expect(1)
            .create();
        let clock = std::sync::Arc::new(FakeClock::new());
        let quota = RateLimit::new(clock.clone());
        let client = Client::new();
        assert_eq!(
            send(
                || client.post(format!("{}/graphql", server.url())),
                Source::GitHub,
                "/graphql",
                "POST",
                &|_| {},
                Some(&quota)
            )
            .unwrap()
            .status(),
            200
        );
        assert_eq!(
            clock.sleeps.lock().unwrap().iter().sum::<Duration>(),
            expected_wait
        );
        first.assert();
        success.assert();
    }
}

#[test]
fn persistent_secondary_limits_abort_other_requests_without_more_waiting() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/resource")
        .with_status(403)
        .with_body("secondary rate limit")
        .expect(3)
        .create();
    let later = server
        .mock("GET", "/later")
        .with_body("{}")
        .expect(0)
        .create();
    let clock = std::sync::Arc::new(FakeClock::new());
    let quota = RateLimit::new(clock.clone());
    let client = Client::new();
    assert!(
        send(
            || client.get(format!("{}/resource", server.url())),
            Source::GitHub,
            "/resource",
            "GET",
            &|_| {},
            Some(&quota)
        )
        .is_err()
    );
    let waited = clock.sleeps.lock().unwrap().iter().sum::<Duration>();
    assert!(
        send(
            || client.get(format!("{}/later", server.url())),
            Source::GitHub,
            "/later",
            "GET",
            &|_| {},
            Some(&quota)
        )
        .is_err()
    );
    assert_eq!(
        waited,
        clock.sleeps.lock().unwrap().iter().sum::<Duration>()
    );
    limited.assert();
    later.assert();
}

#[test]
fn primary_short_reset_is_respected_before_retrying() {
    let mut server = Server::new();
    let limited = server
        .mock("GET", "/resource")
        .with_status(403)
        .with_header("x-ratelimit-remaining", "0")
        .with_header("x-ratelimit-reset", "1800000002")
        .with_body("API rate limit exceeded")
        .expect(1)
        .create();
    let success = server
        .mock("GET", "/resource")
        .with_body("{}")
        .expect(1)
        .create();
    let clock = std::sync::Arc::new(FakeClock::new());
    let quota = RateLimit::new(clock.clone());
    let client = Client::new();
    assert_eq!(
        send(
            || client.get(format!("{}/resource", server.url())),
            Source::GitHub,
            "/resource",
            "GET",
            &|_| {},
            Some(&quota)
        )
        .unwrap()
        .status(),
        200
    );
    assert_eq!(
        clock.sleeps.lock().unwrap().iter().sum::<Duration>(),
        Duration::from_secs(2)
    );
    limited.assert();
    success.assert();
}

#[test]
fn github_retries_rate_limits_and_server_errors_even_when_the_body_is_truncated() {
    for status in [429, 503] {
        let mut server = Server::new();
        let broken = server
            .mock("GET", "/resource")
            .with_status(status)
            .with_chunked_body(|writer| {
                writer.write_all(b"{")?;
                writer.flush()?;
                std::thread::sleep(Duration::from_millis(100));
                Err(std::io::Error::other("truncated response"))
            })
            .expect(1)
            .create();
        let success = server
            .mock("GET", "/resource")
            .with_body("{}")
            .expect(1)
            .create();
        let clock = std::sync::Arc::new(FakeClock::new());
        let quota = RateLimit::new(clock.clone());
        let client = Client::new();
        let response = send(
            || client.get(format!("{}/resource", server.url())),
            Source::GitHub,
            "/resource",
            "GET",
            &|_| {},
            Some(&quota),
        );
        assert!(
            response.is_ok(),
            "HTTP {status} was not retried: {:?}",
            response.as_ref().err()
        );
        assert_eq!(response.unwrap().status(), 200);
        assert!(!clock.sleeps.lock().unwrap().is_empty());
        broken.assert();
        success.assert();
    }
}

#[test]
fn independent_github_batches_share_a_global_limit_of_eight_http_requests() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut server = Server::new();
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let active_writer = active.clone();
    let peak_writer = peak.clone();
    let mock = server
        .mock("GET", "/resource")
        .with_chunked_body(move |writer| {
            peak_writer.fetch_max(
                active_writer.fetch_add(1, Ordering::SeqCst) + 1,
                Ordering::SeqCst,
            );
            std::thread::sleep(Duration::from_millis(200));
            let result = writer.write_all(b"{}");
            active_writer.fetch_sub(1, Ordering::SeqCst);
            result
        })
        .expect(16)
        .create();
    let quota = RateLimit::default();
    let client = Client::new();
    std::thread::scope(|scope| {
        let tasks = (0..16)
            .map(|_| {
                scope.spawn(|| {
                    send(
                        || client.get(format!("{}/resource", server.url())),
                        Source::GitHub,
                        "/resource",
                        "GET",
                        &|_| {},
                        Some(&quota),
                    )
                    .unwrap()
                    .status()
                })
            })
            .collect::<Vec<_>>();
        for task in tasks {
            assert_eq!(task.join().unwrap(), 200);
        }
    });
    assert!(
        (2..=8).contains(&peak.load(Ordering::SeqCst)),
        "peak={}",
        peak.load(Ordering::SeqCst)
    );
    mock.assert();
}
