use super::{header_number, retry_seconds};
use crate::activity::Source;
use crate::progress::Event;
use reqwest::StatusCode;
use reqwest::blocking::RequestBuilder;
use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WAIT_BUDGET: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Resource {
    Core,
    Search,
    Graphql,
}

impl Resource {
    fn for_operation(operation: &str) -> Self {
        match operation {
            "/graphql" => Self::Graphql,
            value if value.starts_with("/search/") => Self::Search,
            _ => Self::Core,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Search => "search",
            Self::Graphql => "graphql",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RateLimitError {
    resource: Resource,
    remaining: Option<u64>,
    resume_at: Option<SystemTime>,
    reason: &'static str,
}

impl fmt::Display for RateLimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "limite de débit GitHub ({}, restant {}) : {} ; reprise {}",
            self.resource.label(),
            self.remaining
                .map_or_else(|| "inconnu".into(), |value| value.to_string()),
            self.reason,
            reset_label(self.resume_at)
        )
    }
}
impl Error for RateLimitError {}

pub(super) trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn sleep(&self, delay: Duration);
    fn jitter(&self) -> Duration;
}

struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
    fn sleep(&self, delay: Duration) {
        std::thread::sleep(delay);
    }
    fn jitter(&self) -> Duration {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let nanos = self
            .now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64;
        Duration::from_millis(1 + (nanos ^ SEQUENCE.fetch_add(7919, Ordering::Relaxed)) % 1000)
    }
}

#[derive(Clone)]
struct Limit {
    error: RateLimitError,
    stopped: bool,
}

#[derive(Default)]
struct Limits {
    primary: BTreeMap<Resource, Limit>,
    secondary: Option<Limit>,
}

pub(crate) struct RateLimit {
    limits: Mutex<Limits>,
    clock: Arc<dyn Clock>,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self::new(Arc::new(SystemClock))
    }
}

impl RateLimit {
    pub(super) fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            limits: Mutex::default(),
            clock,
        }
    }

    fn remember(&self, limit: Limit, secondary: bool) {
        let mut limits = self.limits.lock().unwrap();
        let previous = if secondary {
            limits.secondary.as_ref()
        } else {
            limits.primary.get(&limit.error.resource)
        };
        if previous.is_some_and(|previous| {
            previous.stopped || (!limit.stopped && previous.error.resume_at > limit.error.resume_at)
        }) {
            return;
        }
        if secondary {
            limits.secondary = Some(limit);
        } else {
            limits.primary.insert(limit.error.resource, limit);
        }
    }

    fn wait(
        &self,
        resource: Resource,
        progress: &impl Fn(Event),
        waited: &mut Duration,
    ) -> Result<(), Box<dyn Error>> {
        loop {
            let limit = {
                let limits = self.limits.lock().unwrap();
                [limits.primary.get(&resource), limits.secondary.as_ref()]
                    .into_iter()
                    .flatten()
                    .max_by_key(|limit| (limit.stopped, limit.error.resume_at))
                    .cloned()
            };
            let Some(limit) = limit else {
                return Ok(());
            };
            if limit.stopped {
                return Err(limit.error.into());
            }
            let Some(resume_at) = limit.error.resume_at else {
                return Err(limit.error.into());
            };
            let delay = resume_at
                .duration_since(self.clock.now())
                .unwrap_or_default();
            if delay.is_zero() {
                return Ok(());
            }
            if delay > WAIT_BUDGET.saturating_sub(*waited) {
                let error = RateLimitError {
                    reason: "attente supérieure au budget de 300 s ; relancez après la réinitialisation",
                    ..limit.error
                };
                self.remember(
                    Limit {
                        error: error.clone(),
                        stopped: true,
                    },
                    error.remaining != Some(0),
                );
                return Err(error.into());
            }
            progress(Event::QuotaWaiting {
                source: Source::GitHub,
                resource: limit.error.resource.label(),
                remaining: limit.error.remaining,
                reset: epoch(Some(resume_at)),
                seconds: delay.as_secs() + u64::from(delay.subsec_nanos() > 0),
            });
            let step = delay.min(Duration::from_secs(1));
            self.clock.sleep(step);
            *waited += step;
            progress(Event::Waited {
                source: Source::GitHub,
                elapsed: step,
            });
        }
    }
}

pub(crate) struct Response {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Response {
    pub fn status(&self) -> StatusCode {
        self.status
    }
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }
    pub fn json<T: DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
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
    let default_quota = RateLimit::default();
    let quota = quota.unwrap_or(&default_quota);
    let resource = Resource::for_operation(operation);
    let mut waited = Duration::ZERO;
    for attempt in 0..4 {
        quota.wait(resource, progress, &mut waited)?;
        progress(Event::Request {
            source,
            operation,
            method,
        });
        let response = request().send()?;
        let status = response.status();
        let headers = response.headers().clone();
        let header_limited = matches!(status.as_u16(), 403 | 429)
            && (header_number(&headers, "x-ratelimit-remaining") == Some(0)
                || headers.contains_key("retry-after"));
        let body = if header_limited
            || status == StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
        {
            drop(response);
            Vec::new()
        } else {
            response.bytes()?.to_vec()
        };
        let remaining = header_number(&headers, "x-ratelimit-remaining");
        let reset = header_number(&headers, "x-ratelimit-reset");
        progress(Event::RateLimit {
            source,
            remaining,
            limit: header_number(&headers, "x-ratelimit-limit"),
            near_limit: None,
        });
        progress(Event::Quota {
            source,
            resource: resource.label(),
            remaining,
            reset,
        });
        let primary = remaining == Some(0);
        let limited = (matches!(status.as_u16(), 403 | 429)
            && (primary || headers.contains_key("retry-after") || rate_message(&body)))
            || status == StatusCode::TOO_MANY_REQUESTS
            || (operation == "/graphql" && graphql_rate_limit(&body));
        if !limited {
            if primary {
                let resume_at =
                    retry_seconds(&headers, source, quota.clock.now()).and_then(|seconds| {
                        quota.clock.now().checked_add(Duration::from_secs(seconds))
                    });
                quota.remember(
                    Limit {
                        error: RateLimitError {
                            resource,
                            remaining,
                            resume_at,
                            reason: "quota épuisé",
                        },
                        stopped: resume_at.is_none(),
                    },
                    false,
                );
            }
            if status.is_server_error() && attempt < 3 {
                let delay = retry_seconds(&headers, source, quota.clock.now())
                    .map(Duration::from_secs)
                    .unwrap_or(Duration::from_millis(250 * (1 << attempt)));
                if delay <= WAIT_BUDGET.saturating_sub(waited) {
                    quota.clock.sleep(delay);
                    waited += delay;
                    continue;
                }
            }
            return Ok(Response {
                status,
                headers,
                body,
            });
        }
        let malformed = ["retry-after", "x-ratelimit-remaining", "x-ratelimit-reset"]
            .into_iter()
            .any(|name| headers.get_all(name).iter().count() > 1)
            || (headers.contains_key("retry-after")
                && retry_seconds(&headers, source, quota.clock.now()).is_none());
        let declared = retry_seconds(&headers, source, quota.clock.now());
        let delay = if primary {
            declared.map(Duration::from_secs)
        } else {
            Some(
                Duration::from_secs(declared.unwrap_or(60).max(u64::from(attempt > 0)))
                    .saturating_mul(1 << attempt)
                    .saturating_add(quota.clock.jitter()),
            )
        };
        let resume_at = delay.and_then(|delay| quota.clock.now().checked_add(delay));
        let reason = if malformed || resume_at.is_none() {
            "délai de reprise absent ou invalide"
        } else if attempt == 3 {
            "limite persistante après 4 tentatives"
        } else {
            "quota temporairement épuisé"
        };
        let error = RateLimitError {
            resource,
            remaining,
            resume_at,
            reason,
        };
        let stopped = malformed || resume_at.is_none() || attempt == 3;
        quota.remember(
            Limit {
                error: error.clone(),
                stopped,
            },
            !primary,
        );
        if stopped {
            return Err(error.into());
        }
        if delay.is_some_and(|delay| delay > WAIT_BUDGET.saturating_sub(waited)) {
            let error = RateLimitError {
                reason: "attente supérieure au budget de 300 s ; relancez après la réinitialisation",
                ..error
            };
            quota.remember(
                Limit {
                    error: error.clone(),
                    stopped: true,
                },
                !primary,
            );
            return Err(error.into());
        }
    }
    unreachable!()
}

fn rate_message(body: &[u8]) -> bool {
    String::from_utf8_lossy(body)
        .to_ascii_lowercase()
        .contains("rate limit")
}

fn graphql_rate_limit(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("errors").cloned())
        .and_then(|value| value.as_array().cloned())
        .is_some_and(|errors| {
            errors.iter().any(|error| {
                error.get("type").and_then(|value| value.as_str()) == Some("RATE_LIMITED")
                    || error
                        .get("message")
                        .and_then(|value| value.as_str())
                        .is_some_and(|message| message.to_ascii_lowercase().contains("rate limit"))
            })
        })
}

fn epoch(time: Option<SystemTime>) -> Option<u64> {
    time?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

fn reset_label(time: Option<SystemTime>) -> String {
    crate::progress::reset_time(epoch(time))
}
