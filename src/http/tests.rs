use super::*;
use mockito::Server;
use reqwest::blocking::Client;

#[test]
fn retries_throttled_get_once_and_reports_wait_and_remaining_quota() {
    for (source, status) in [
        (Source::GitHub, 403),
        (Source::GitHub, 429),
        (Source::Bitbucket, 429),
    ] {
        let mut server = Server::new();
        let limited = server
            .mock("GET", "/resource")
            .with_status(status)
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
            source,
            "ressource",
            &progress,
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
}

#[test]
fn repeated_throttling_stops_after_one_retry() {
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
        Source::GitHub,
        "ressource",
        &Progress::default(),
    )
    .unwrap();
    assert_eq!(response.status(), 429);
    limited.assert();
}

#[test]
fn unusable_delays_and_other_errors_do_not_retry_or_echo_headers() {
    for (source, status, delay) in [
        (Source::GitHub, 429, None),
        (Source::GitHub, 429, Some("test-token")),
        (Source::GitHub, 429, Some("-1")),
        (Source::GitHub, 429, Some("301")),
        (Source::GitHub, 429, Some("18446744073709551616")),
        (Source::GitHub, 401, Some("0")),
        (Source::GitHub, 500, Some("0")),
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
            &progress,
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
        wait_for_retry(&progress, Source::Bitbucket, 2, |duration| {
            elapsed += duration
        });
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
