use reqwest::header::HeaderMap;
use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Default)]
pub(super) struct RateLimit {
    requests: VecDeque<Instant>,
    cooldown: Option<(Instant, Duration)>,
    stopped: bool,
}

impl RateLimit {
    pub fn start_collection(&mut self) {
        self.stopped = false;
    }

    pub fn reserve(&mut self, now: Instant) -> Result<Duration, &'static str> {
        if self.stopped {
            return Err("Bitbucket : HTTP 429, collecte interrompue après rate limit");
        }
        while self
            .requests
            .front()
            .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(3600))
        {
            self.requests.pop_front();
        }
        if self.requests.len() >= 1000 {
            return Err("Bitbucket : budget local de 1000 requêtes par heure épuisé");
        }
        let delay = self.remaining_delay(now);
        if delay > Duration::from_secs(300) {
            return Err("Bitbucket : HTTP 429, Retry-After supérieur à 300 s");
        }
        if delay.is_zero() {
            self.requests.push_back(now);
        }
        Ok(delay)
    }

    fn remaining_delay(&self, now: Instant) -> Duration {
        self.cooldown.map_or(Duration::ZERO, |(started, delay)| {
            delay.saturating_sub(now.saturating_duration_since(started))
        })
    }

    pub fn defer(&mut self, headers: &HeaderMap, attempt: u32) -> Result<Duration, &'static str> {
        let delay = match retry_delay(headers, attempt, SystemTime::now()) {
            Ok(delay) => delay,
            Err(error) => {
                self.stopped = true;
                return Err(error);
            }
        };
        let now = Instant::now();
        self.cooldown = Some((now, self.remaining_delay(now).max(delay)));
        if delay > Duration::from_secs(300) {
            self.stopped = true;
            return Err("Bitbucket : HTTP 429, Retry-After supérieur à 300 s");
        }
        if attempt == 2 {
            self.stopped = true;
            return Err("Bitbucket : HTTP 429 après trois tentatives");
        }
        Ok(delay)
    }
}

fn retry_delay(
    headers: &HeaderMap,
    attempt: u32,
    now: SystemTime,
) -> Result<Duration, &'static str> {
    let invalid = "Bitbucket : HTTP 429, Retry-After invalide";
    if headers.get_all("retry-after").iter().count() > 1 {
        return Err(invalid);
    }
    let seconds = match headers.get("retry-after") {
        None => 1_u64 << attempt,
        Some(header) => {
            let value = header.to_str().map_err(|_| invalid)?;
            if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
                value.parse::<u64>().map_err(|_| invalid)?
            } else {
                let timestamp = chrono::DateTime::parse_from_rfc2822(value)
                    .map_err(|_| invalid)?
                    .timestamp();
                let timestamp = u64::try_from(timestamp).map_err(|_| invalid)?;
                let now = now.duration_since(UNIX_EPOCH).map_err(|_| invalid)?;
                Duration::from_secs(timestamp)
                    .saturating_sub(now)
                    .as_secs_f64()
                    .ceil() as u64
            }
        }
    };
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_cooldowns_survive_new_collections_and_expire_without_instant_overflow() {
        for seconds in [301, u64::MAX] {
            let mut limit = RateLimit::default();
            let mut headers = HeaderMap::new();
            headers.insert("retry-after", seconds.to_string().parse().unwrap());
            assert!(limit.defer(&headers, 0).is_err());
            limit.start_collection();
            let now = Instant::now();
            assert!(limit.reserve(now).is_err());
            if seconds == 301 {
                assert!(
                    limit.reserve(now + Duration::from_secs(2)).unwrap() > Duration::from_secs(298)
                );
                assert_eq!(
                    limit.reserve(now + Duration::from_secs(301)).unwrap(),
                    Duration::ZERO
                );
            }
        }
    }

    #[test]
    fn hourly_budget_expires_each_reservation_after_a_full_hour() {
        let mut limit = RateLimit::default();
        let start = Instant::now();
        for _ in 0..1000 {
            assert_eq!(limit.reserve(start).unwrap(), Duration::ZERO);
        }
        assert!(limit.reserve(start + Duration::from_secs(3599)).is_err());
        assert_eq!(
            limit.reserve(start + Duration::from_secs(3600)).unwrap(),
            Duration::ZERO
        );
    }

    #[test]
    fn waiting_does_not_spend_the_budget_and_shorter_retries_cannot_shorten_a_cooldown() {
        let mut limit = RateLimit::default();
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "10".parse().unwrap());
        limit.defer(&headers, 0).unwrap();
        headers.insert("retry-after", "0".parse().unwrap());
        limit.defer(&headers, 1).unwrap();
        let now = Instant::now();
        for _ in 0..1001 {
            assert!(limit.reserve(now).unwrap() > Duration::from_secs(9));
        }
        assert_eq!(
            limit.reserve(now + Duration::from_secs(10)).unwrap(),
            Duration::ZERO
        );
    }

    #[test]
    fn parses_http_dates_and_long_delays_but_rejects_duplicate_retry_after() {
        let now = UNIX_EPOCH + Duration::from_secs(1_445_412_479);
        let mut headers = HeaderMap::new();
        headers.insert(
            "retry-after",
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(
            retry_delay(&headers, 0, now).unwrap(),
            Duration::from_secs(1)
        );
        assert_eq!(
            retry_delay(&headers, 0, now + Duration::from_millis(1)).unwrap(),
            Duration::from_secs(1)
        );
        assert_eq!(
            retry_delay(&headers, 0, now + Duration::from_secs(5)).unwrap(),
            Duration::ZERO
        );
        headers.append("retry-after", "0".parse().unwrap());
        assert!(retry_delay(&headers, 0, now).is_err());
        headers.clear();
        headers.insert("retry-after", "301".parse().unwrap());
        assert_eq!(
            retry_delay(&headers, 0, now).unwrap(),
            Duration::from_secs(301)
        );
    }
}
