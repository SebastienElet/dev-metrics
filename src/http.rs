use crate::activity::Source;
use crate::progress::Event;
use reqwest::blocking::{RequestBuilder, Response};
use reqwest::header::HeaderMap;
use std::error::Error;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) fn get(
    request: impl Fn() -> RequestBuilder,
    source: Source,
    operation: &'static str,
    progress: &impl Fn(Event),
) -> Result<Response, Box<dyn Error>> {
    send(request, source, operation, "GET", progress, None)
}

#[derive(Default)]
pub(crate) struct RateLimit {
    state: Mutex<Quota>,
}

#[derive(Default)]
struct Quota {
    until: Option<Instant>,
    stopped: bool,
}

impl RateLimit {
    fn wait(&self, source: Source, progress: &impl Fn(Event)) -> Result<(), Box<dyn Error>> {
        let mut quota = self
            .state
            .lock()
            .map_err(|_| "Coordination du débit interrompue")?;
        if quota.stopped {
            return Err("collecte arrêtée par la limite de débit".into());
        }
        if let Some(until) = quota.until.take() {
            let delay = until.saturating_duration_since(Instant::now());
            let seconds = delay.as_secs() + u64::from(delay.subsec_nanos() > 0);
            if seconds > 300 {
                quota.stopped = true;
                return Err(
                    "limite de débit, attente supérieure à 300 s ; relancez plus tard".into(),
                );
            }
            wait_for_retry(progress, source, seconds, std::thread::sleep);
        }
        Ok(())
    }

    fn observe(&self, response: &Response, source: Source) {
        let limited = matches!(response.status().as_u16(), 403 | 429);
        if !limited && header_number(response.headers(), "x-ratelimit-remaining") != Some(0) {
            return;
        }
        let seconds = retry_seconds(response.headers(), source, SystemTime::now());
        let until =
            seconds.and_then(|seconds| Instant::now().checked_add(Duration::from_secs(seconds)));
        if let Ok(mut quota) = self.state.lock() {
            match until {
                Some(until) => {
                    quota.until = Some(quota.until.map_or(until, |previous| previous.max(until)))
                }
                None => quota.stopped = true,
            }
        }
    }
}

pub(crate) fn send(
    request: impl Fn() -> RequestBuilder,
    source: Source,
    operation: &'static str,
    method: &'static str,
    progress: &impl Fn(Event),
    quota: Option<&RateLimit>,
) -> Result<Response, Box<dyn Error>> {
    let mut retried = false;
    loop {
        if let Some(quota) = quota {
            quota.wait(source, progress)?;
        }
        progress(Event::Request {
            source,
            operation,
            method,
        });
        let response = request().send()?;
        if let Some(quota) = quota {
            quota.observe(&response, source);
        }
        let headers = response.headers();
        progress(Event::RateLimit {
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
        if quota.is_none() {
            wait_for_retry(progress, source, seconds, std::thread::sleep);
        }
        retried = true;
    }
}

fn retry_delay(headers: &HeaderMap, source: Source, now: SystemTime) -> Option<u64> {
    retry_seconds(headers, source, now).filter(|seconds| *seconds <= 300)
}

fn retry_seconds(headers: &HeaderMap, source: Source, now: SystemTime) -> Option<u64> {
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
    Some(seconds)
}

fn wait_for_retry(
    progress: &impl Fn(Event),
    source: Source,
    seconds: u64,
    mut sleep: impl FnMut(Duration),
) {
    let start = Instant::now();
    progress(Event::Waiting { source, seconds });
    for remaining in (0..seconds).rev() {
        sleep(Duration::from_secs(1));
        progress(Event::Waiting {
            source,
            seconds: remaining,
        });
    }
    progress(Event::Waited {
        source,
        elapsed: start.elapsed(),
    });
}

fn header_number(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

#[cfg(test)]
mod tests;
