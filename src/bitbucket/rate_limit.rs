use crate::http::retry::{self, RateLimitError};
use reqwest::header::HeaderMap;
use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime};

#[derive(Default)]
pub(super) struct RateLimit {
    requests: VecDeque<Instant>,
    cooldown: Option<(Instant, Duration)>,
    stopped: Option<RateLimitError>,
}

impl RateLimit {
    pub fn current_error(&self) -> Option<RateLimitError> {
        self.stopped
            .as_ref()
            .map(|error| error.with_delay(self.remaining_delay(Instant::now())))
    }

    pub fn start_collection(&mut self) {
        self.stopped = None;
    }

    pub fn reserve(&mut self, now: Instant) -> Result<Duration, RateLimitError> {
        if let Some(error) = &self.stopped {
            return Err(error.clone());
        }
        while self
            .requests
            .front()
            .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(3600))
        {
            self.requests.pop_front();
        }
        if self.requests.len() >= 1000 {
            let delay = Duration::from_secs(3600)
                .saturating_sub(now.saturating_duration_since(*self.requests.front().unwrap()));
            return Err(RateLimitError::new(
                "budget local de 1000 requêtes par heure épuisé",
                Some(delay),
            ));
        }
        let delay = self.remaining_delay(now);
        if delay > Duration::from_secs(300) {
            return Err(RateLimitError::new(
                "HTTP 429/403, attente supérieure à 300 s",
                Some(delay),
            ));
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

    pub fn defer(&mut self, headers: &HeaderMap, attempt: u32) -> Result<Duration, RateLimitError> {
        let delay = match retry::delay(headers, attempt, SystemTime::now(), retry::jitter()) {
            Ok(delay) => delay,
            Err(reason) => {
                let error = RateLimitError::new(format!("HTTP 429/403, {reason}"), None);
                self.stopped = Some(error.clone());
                return Err(error);
            }
        };
        let now = Instant::now();
        let delay = self.remaining_delay(now).max(delay);
        self.cooldown = Some((now, delay));
        if delay > Duration::from_secs(300) {
            let error =
                RateLimitError::new("HTTP 429/403, attente supérieure à 300 s", Some(delay));
            self.stopped = Some(error.clone());
            return Err(error);
        }
        if attempt == 2 {
            let error = RateLimitError::new("HTTP 429/403 après trois tentatives", Some(delay));
            self.stopped = Some(error.clone());
            return Err(error);
        }
        Ok(delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_shared_cooldown_when_a_later_response_requests_a_shorter_wait() {
        let mut limit = RateLimit::default();
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "600".parse().unwrap());
        let retry_at = |error: RateLimitError| {
            let message = error.to_string();
            let date = message
                .split("réessayez à ")
                .nth(1)
                .unwrap()
                .split(" UTC")
                .next()
                .unwrap();
            chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M:%S").unwrap()
        };
        let initial = retry_at(limit.defer(&headers, 0).unwrap_err());
        headers.insert("retry-after", "0".parse().unwrap());
        let subsequent = retry_at(limit.defer(&headers, 2).unwrap_err());
        assert!(subsequent >= initial - chrono::Duration::seconds(1));
    }

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
                    limit.reserve(now + Duration::from_secs(302)).unwrap(),
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
            limit.reserve(now + Duration::from_secs(11)).unwrap(),
            Duration::ZERO
        );
    }
}
