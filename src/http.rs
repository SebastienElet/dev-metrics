pub(crate) mod retry;

use crate::activity::Source;
use crate::progress::{Event, Progress};
use reqwest::blocking::{RequestBuilder, Response};
use reqwest::header::HeaderMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) fn get(
    request: impl Fn() -> RequestBuilder,
    source: Source,
    operation: &'static str,
    progress: &Progress,
) -> Result<Response, reqwest::Error> {
    let mut retried = false;
    loop {
        progress.emit(Event::Request { source, operation });
        let response = request().send()?;
        let headers = response.headers();
        progress.emit(Event::RateLimit {
            source,
            remaining: header_number(headers, "x-ratelimit-remaining"),
            limit: header_number(headers, "x-ratelimit-limit"),
            near_limit: headers
                .get("x-ratelimit-nearlimit")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok()),
        });
        let limited =
            response.status() == 429 || (source == Source::GitHub && response.status() == 403);
        let delay = if limited && !retried {
            retry_delay(headers, source, SystemTime::now())
        } else {
            None
        };
        let Some(seconds) = delay else {
            return Ok(response);
        };
        drop(response);
        wait_for_retry(progress, source, seconds, std::thread::sleep);
        retried = true;
    }
}

fn retry_delay(headers: &HeaderMap, source: Source, now: SystemTime) -> Option<u64> {
    if ["retry-after", "x-ratelimit-remaining", "x-ratelimit-reset"]
        .into_iter()
        .any(|name| headers.get_all(name).iter().count() > 1)
    {
        return None;
    }
    let now = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let reset =
        if source == Source::GitHub && header_number(headers, "x-ratelimit-remaining") == Some(0) {
            header_number(headers, "x-ratelimit-reset").map(|reset| reset.saturating_sub(now))
        } else {
            None
        };
    let seconds = match headers.get("retry-after") {
        Some(value) => {
            let value = value.to_str().ok()?;
            let delay = if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
                value.parse::<u64>().ok()?
            } else {
                let date = chrono::DateTime::parse_from_rfc2822(value)
                    .ok()?
                    .timestamp();
                u64::try_from(date).ok()?.saturating_sub(now)
            };
            delay.max(reset.unwrap_or(0))
        }
        None => reset?,
    };
    (seconds <= 300).then_some(seconds)
}

fn wait_for_retry(
    progress: &Progress,
    source: Source,
    seconds: u64,
    mut sleep: impl FnMut(Duration),
) {
    progress.emit(Event::Waiting { source, seconds });
    for remaining in (0..seconds).rev() {
        sleep(Duration::from_secs(1));
        progress.emit(Event::Waiting {
            source,
            seconds: remaining,
        });
    }
}

fn header_number(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

#[cfg(test)]
mod tests;
