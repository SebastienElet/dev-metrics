use reqwest::header::HeaderMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub(crate) struct RateLimitError {
    reason: String,
    message: String,
    known_deadline: bool,
}

impl RateLimitError {
    pub fn new(reason: impl AsRef<str>, delay: Option<Duration>) -> Self {
        let retry = match delay {
            Some(delay) => {
                let seconds = delay
                    .as_secs()
                    .saturating_add(u64::from(delay.subsec_nanos() > 0));
                let date = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .and_then(|now| now.checked_add(delay))
                    .and_then(|time| {
                        time.as_secs()
                            .checked_add(u64::from(time.subsec_nanos() > 0))
                    })
                    .and_then(|seconds| i64::try_from(seconds).ok())
                    .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0));
                match date {
                    Some(date) => format!(
                        "réessayez à {} UTC (dans {seconds} s)",
                        date.format("%Y-%m-%d %H:%M:%S")
                    ),
                    None => format!("réessayez dans {seconds} s"),
                }
            }
            None => "réessayez après réinitialisation du quota (échéance serveur inconnue)".into(),
        };
        Self {
            reason: reason.as_ref().into(),
            message: format!("limite de débit : {} ; {retry}", reason.as_ref()),
            known_deadline: delay.is_some(),
        }
    }

    pub fn with_delay(&self, delay: Duration) -> Self {
        if self.known_deadline {
            Self::new(&self.reason, Some(delay))
        } else {
            self.clone()
        }
    }
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RateLimitError {}

pub(crate) fn is_rate_limited(status: u16, headers: &HeaderMap, body: &[u8]) -> bool {
    if status == 429 {
        return true;
    }
    if status != 403 {
        return false;
    }
    if headers.contains_key("retry-after")
        || headers
            .keys()
            .any(|key| key.as_str().starts_with("x-ratelimit-"))
    {
        return true;
    }
    let body = String::from_utf8_lossy(body).to_ascii_lowercase();
    ["rate limit", "rate_limit", "rate-limit", "ratelimit"]
        .iter()
        .any(|term| body.contains(term))
}

pub(crate) fn jitter() -> Duration {
    Duration::from_millis(RandomState::new().build_hasher().finish() % 251)
}

pub(crate) fn backoff(attempt: u32, jitter: Duration) -> Duration {
    Duration::from_secs(1_u64 << attempt.min(6)).saturating_add(jitter)
}

pub(crate) fn delay(
    headers: &HeaderMap,
    attempt: u32,
    now: SystemTime,
    jitter: Duration,
) -> Result<Duration, &'static str> {
    let invalid = "en-têtes de reprise invalides ou ambigus";
    for name in ["retry-after", "x-ratelimit-reset", "x-ratelimit-remaining"] {
        if headers.get_all(name).iter().count() > 1 {
            return Err(invalid);
        }
    }
    let now = now.duration_since(UNIX_EPOCH).map_err(|_| invalid)?;
    let reset = headers
        .get("x-ratelimit-reset")
        .map(|value| {
            let seconds = value
                .to_str()
                .map_err(|_| invalid)?
                .parse::<u64>()
                .map_err(|_| invalid)?;
            Ok::<_, &'static str>(Duration::from_secs(seconds).saturating_sub(now))
        })
        .transpose()?;
    let retry = headers
        .get("retry-after")
        .map(|value| {
            let value = value.to_str().map_err(|_| invalid)?;
            if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
                return value
                    .parse::<u64>()
                    .map(Duration::from_secs)
                    .map_err(|_| invalid);
            }
            let timestamp = chrono::DateTime::parse_from_rfc2822(value)
                .map_err(|_| invalid)?
                .timestamp();
            let timestamp = u64::try_from(timestamp).map_err(|_| invalid)?;
            Ok(Duration::from_secs(timestamp).saturating_sub(now))
        })
        .transpose()?;
    Ok(match (retry, reset) {
        (None, None) => backoff(attempt, jitter),
        (retry, reset) => retry
            .unwrap_or_default()
            .max(reset.unwrap_or_default())
            .saturating_add(jitter),
    })
}

pub(crate) fn wait(
    delay: Duration,
    mut progress: impl FnMut(u64),
    mut sleep: impl FnMut(Duration),
) {
    let mut remaining = delay;
    loop {
        progress(
            remaining
                .as_secs()
                .saturating_add(u64::from(remaining.subsec_nanos() > 0)),
        );
        if remaining.is_zero() {
            break;
        }
        let step = remaining.min(Duration::from_secs(1));
        sleep(step);
        remaining -= step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_later_server_deadline_and_adds_jitter_without_retrying_early() {
        let now = UNIX_EPOCH + Duration::from_secs(1_445_412_479);
        let mut headers = HeaderMap::new();
        headers.insert(
            "retry-after",
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        headers.insert("x-ratelimit-reset", "1445412482".parse().unwrap());
        assert_eq!(
            delay(&headers, 0, now, Duration::from_millis(125)).unwrap(),
            Duration::from_millis(3125)
        );
        headers.insert("retry-after", "5".parse().unwrap());
        assert_eq!(
            delay(&headers, 0, now, Duration::ZERO).unwrap(),
            Duration::from_secs(5)
        );
        headers.insert("retry-after", "301".parse().unwrap());
        assert_eq!(
            delay(&headers, 0, now, Duration::ZERO).unwrap(),
            Duration::from_secs(301)
        );
        headers.insert("retry-after", u64::MAX.to_string().parse().unwrap());
        assert_eq!(
            delay(&headers, 0, now, Duration::ZERO).unwrap(),
            Duration::from_secs(u64::MAX)
        );
    }

    #[test]
    fn absent_headers_back_off_exponentially_and_jitter_is_bounded() {
        let headers = HeaderMap::new();
        for (attempt, seconds) in [(0, 1), (1, 2), (2, 4)] {
            assert_eq!(
                delay(&headers, attempt, UNIX_EPOCH, Duration::from_millis(200)).unwrap(),
                Duration::from_millis(seconds * 1000 + 200)
            );
        }
        for _ in 0..16 {
            assert!(jitter() <= Duration::from_millis(250));
        }
    }

    #[test]
    fn rejects_invalid_or_ambiguous_retry_instructions_without_echoing_them() {
        for name in ["retry-after", "x-ratelimit-reset", "x-ratelimit-remaining"] {
            let mut headers = HeaderMap::new();
            headers.append(name, "0".parse().unwrap());
            headers.append(name, "600".parse().unwrap());
            assert!(delay(&headers, 0, UNIX_EPOCH, Duration::ZERO).is_err());
        }
        for name in ["retry-after", "x-ratelimit-reset"] {
            for value in ["secret-token", "-1", "18446744073709551616"] {
                let mut headers = HeaderMap::new();
                headers.insert(name, value.parse().unwrap());
                let error = delay(&headers, 0, UNIX_EPOCH, Duration::ZERO).unwrap_err();
                assert!(!error.contains("secret-token"));
            }
        }
    }

    #[test]
    fn countdown_covers_fractional_jitter_without_shortening_the_wait() {
        let mut elapsed = Duration::ZERO;
        let mut updates = Vec::new();
        wait(
            Duration::from_millis(2125),
            |seconds| updates.push(seconds),
            |duration| elapsed += duration,
        );
        assert_eq!(elapsed, Duration::from_millis(2125));
        assert_eq!(updates, vec![3, 2, 1, 0]);
    }
}
