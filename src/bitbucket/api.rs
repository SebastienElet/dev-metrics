use super::rate_limit::RateLimit;
use crate::activity::Source;
use crate::progress::{Event, Progress};
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, LOCATION};
use reqwest::{Url, redirect, retry};
use serde::{Deserialize, de::DeserializeOwned};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(super) struct Api {
    base: Url,
    client: Client,
    token: String,
    email: Option<String>,
    pub progress: Progress,
    statistics: Mutex<BTreeMap<Endpoint, Statistics>>,
    rate_limit: Mutex<RateLimit>,
}

impl Api {
    pub fn new(base: Url, token: &str) -> Result<Self, Box<dyn Error>> {
        Self::build(base, token, None)
    }

    pub fn new_with_email(base: Url, token: &str, email: &str) -> Result<Self, Box<dyn Error>> {
        crate::config::validate_setting("bitbucket.email", email)?;
        Self::build(base, token, Some(email))
    }

    fn build(mut base: Url, token: &str, email: Option<&str>) -> Result<Self, Box<dyn Error>> {
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        if !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("URL API Bitbucket invalide".into());
        }
        if token.is_empty()
            || token
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        {
            return Err("Token Bitbucket invalide".into());
        }
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let client = Client::builder()
            .user_agent("dev-metrics")
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(redirect::Policy::none())
            .retry(retry::never())
            .build()?;
        Ok(Self {
            base,
            client,
            token: token.into(),
            email: email.map(str::to_owned),
            progress: Progress::default(),
            statistics: Mutex::default(),
            rate_limit: Mutex::default(),
        })
    }

    pub fn summarize(&self, elapsed: Duration) {
        for (endpoint, statistics) in self.statistics.lock().unwrap().iter() {
            self.progress.emit(Event::EndpointSummary {
                source: Source::Bitbucket,
                operation: endpoint.label(),
                requests: statistics.requests,
                pages: statistics.pages,
                elapsed: statistics.elapsed,
            });
        }
        self.progress.emit(Event::CollectionSummary {
            source: Source::Bitbucket,
            elapsed,
            cache_hits: 0,
            cache_misses: 0,
        });
    }

    pub fn start_collection(&self) {
        self.statistics.lock().unwrap().clear();
        self.rate_limit.lock().unwrap().start_collection();
    }

    fn checked_url(&self, url: Url) -> Result<Url, Box<dyn Error>> {
        if url.origin() != self.base.origin()
            || !url.path().starts_with(self.base.path())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err("Lien Bitbucket hors de l'API autorisée".into());
        }
        Ok(url)
    }

    pub fn get<T: DeserializeOwned>(&self, endpoint: &str) -> Result<T, Box<dyn Error>> {
        let (body, _) = self.request(self.base.join(endpoint)?, Endpoint::for_path(endpoint))?;
        Ok(body)
    }

    fn request<T: DeserializeOwned>(
        &self,
        mut url: Url,
        endpoint: Endpoint,
    ) -> Result<(T, Url), Box<dyn Error>> {
        let mut visited = BTreeSet::new();
        for _ in 0..=5 {
            url = self.checked_url(url)?;
            endpoint.select_fields(&mut url);
            if !visited.insert(url.to_string()) {
                return Err("Redirection Bitbucket cyclique".into());
            }
            let response = self.send(&url, endpoint)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .ok_or("Redirection Bitbucket sans destination")?
                    .to_str()
                    .map_err(|_| "Redirection Bitbucket invalide")?;
                url = url
                    .join(location)
                    .map_err(|_| "Redirection Bitbucket invalide")?;
                continue;
            }
            if !response.status().is_success() {
                return Err(format!(
                    "Bitbucket : HTTP {} (vérifiez le token, les permissions et les limites API)",
                    response.status().as_u16()
                )
                .into());
            }
            let started = Instant::now();
            let body = response.json().map_err(|_| "Réponse Bitbucket invalide");
            self.statistics
                .lock()
                .unwrap()
                .entry(endpoint)
                .or_default()
                .elapsed += started.elapsed();
            return Ok((body?, url));
        }
        Err("Trop de redirections Bitbucket".into())
    }

    fn send(&self, url: &Url, endpoint: Endpoint) -> Result<Response, Box<dyn Error>> {
        for attempt in 0..3 {
            loop {
                let delay = self.rate_limit.lock().unwrap().reserve(Instant::now())?;
                if delay.is_zero() {
                    break;
                }
                self.progress.emit(Event::Waiting {
                    source: Source::Bitbucket,
                    seconds: delay.as_secs_f64().ceil() as u64,
                });
                std::thread::sleep(delay.min(Duration::from_secs(1)));
            }
            self.statistics
                .lock()
                .unwrap()
                .entry(endpoint)
                .or_default()
                .requests += 1;
            self.progress.emit(Event::Request {
                source: Source::Bitbucket,
                operation: endpoint.label(),
            });
            let started = Instant::now();
            let request = self.client.get(url.clone());
            let response = match &self.email {
                Some(email) => request.basic_auth(email, Some(&self.token)),
                None => request.bearer_auth(&self.token),
            }
            .send();
            self.statistics
                .lock()
                .unwrap()
                .entry(endpoint)
                .or_default()
                .elapsed += started.elapsed();
            let response = response.map_err(|error| {
                if error.is_timeout() {
                    "Bitbucket : timeout"
                } else {
                    "Bitbucket : échec réseau"
                }
            })?;
            let number = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse().ok())
            };
            self.progress.emit(Event::RateLimit {
                source: Source::Bitbucket,
                remaining: number("x-ratelimit-remaining"),
                limit: number("x-ratelimit-limit"),
                near_limit: response
                    .headers()
                    .get("x-ratelimit-nearlimit")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse().ok()),
            });
            if response.status() != 429 {
                return Ok(response);
            }
            let delay = self
                .rate_limit
                .lock()
                .unwrap()
                .defer(response.headers(), attempt)?;
            self.progress.emit(Event::Waiting {
                source: Source::Bitbucket,
                seconds: delay.as_secs(),
            });
        }
        Err("Bitbucket : HTTP 429 après trois tentatives".into())
    }

    pub fn list<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        query: &[(&str, &str)],
    ) -> Result<Vec<T>, Box<dyn Error>> {
        let mut url = self.base.join(endpoint)?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let mut visited = BTreeSet::new();
        let mut expected_size = None;
        let mut items = Vec::new();
        let mut number = 1;
        let operation = Endpoint::for_path(endpoint);
        loop {
            operation.select_fields(&mut url);
            if !visited.insert(url.to_string()) {
                return Err("Pagination Bitbucket cyclique".into());
            }
            self.statistics
                .lock()
                .unwrap()
                .entry(operation)
                .or_default()
                .pages += 1;
            self.progress.emit(Event::Page {
                source: Source::Bitbucket,
                operation: operation.label(),
                number,
            });
            let (page, effective): (Page<T>, _) = self.request(url, operation)?;
            if let Some(size) = page.size {
                if expected_size.is_some_and(|previous| previous != size) {
                    return Err("Taille Bitbucket modifiée pendant la pagination".into());
                }
                expected_size = Some(size);
            }
            if page.values.is_empty() && page.next.is_some() {
                return Err("Pagination Bitbucket vide avec une page suivante".into());
            }
            items.extend(page.values);
            if expected_size.is_some_and(|size| items.len() > size) {
                return Err("Pagination Bitbucket incohérente".into());
            }
            match page.next {
                Some(next) => {
                    number += 1;
                    url = self.checked_url(
                        effective
                            .join(&next)
                            .map_err(|_| "Lien de pagination Bitbucket invalide")?,
                    )?
                }
                None => {
                    if expected_size.is_some_and(|size| size != items.len()) {
                        return Err("Pagination Bitbucket incomplète".into());
                    }
                    return Ok(items);
                }
            }
        }
    }
}

#[derive(Default)]
struct Statistics {
    requests: usize,
    pages: usize,
    elapsed: Duration,
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum Endpoint {
    Identity,
    Repositories,
    PullRequests,
    Commits,
    Activity,
    Comments,
    PullRequestDiff,
    CommitDiff,
}

impl Endpoint {
    fn for_path(path: &str) -> Self {
        match path.split('/').collect::<Vec<_>>().as_slice() {
            ["repositories", _] => Self::Repositories,
            ["repositories", _, _, "pullrequests"] => Self::PullRequests,
            ["repositories", _, _, "commits"] => Self::Commits,
            ["repositories", _, _, "pullrequests", _, "activity"] => Self::Activity,
            ["repositories", _, _, "pullrequests", _, "comments"] => Self::Comments,
            ["repositories", _, _, "pullrequests", _, "diffstat"] => Self::PullRequestDiff,
            ["repositories", _, _, "diffstat", _] => Self::CommitDiff,
            _ => Self::Identity,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Identity => "identité",
            Self::Repositories => "dépôts",
            Self::PullRequests => "PRs",
            Self::Commits => "commits",
            Self::Activity => "activité PR",
            Self::Comments => "commentaires",
            Self::PullRequestDiff => "diffstat PR",
            Self::CommitDiff => "diffstat commit",
        }
    }

    fn fields(self) -> &'static str {
        match self {
            Self::Identity => "uuid",
            Self::Repositories => "next,size,values.full_name",
            Self::PullRequests => {
                "next,size,values.id,values.author.uuid,values.title,values.state,values.created_on,values.updated_on,values.links.html.href"
            }
            Self::Commits => {
                "next,size,values.hash,values.author.user.uuid,values.date,values.message,values.links.html.href"
            }
            Self::Activity => {
                "next,size,values.approval.date,values.approval.user.uuid,values.changes_requested.date,values.changes_requested.user.uuid,values.update.date,values.update.state,values.comment.id"
            }
            Self::Comments => {
                "next,size,values.id,values.user.uuid,values.created_on,values.deleted,values.pending,values.inline,values.links.html.href"
            }
            Self::PullRequestDiff | Self::CommitDiff => {
                "next,size,values.old.path,values.new.path,values.lines_added,values.lines_removed"
            }
        }
    }

    fn select_fields(self, url: &mut Url) {
        let pairs = url
            .query_pairs()
            .filter(|(key, _)| key != "fields" && key != "pagelen")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        let mut query = url.query_pairs_mut();
        query
            .clear()
            .extend_pairs(pairs)
            .append_pair("fields", self.fields());
        match self {
            Self::Identity => {}
            Self::PullRequests => {
                query.append_pair("pagelen", "50");
            }
            _ => {
                query.append_pair("pagelen", "100");
            }
        }
    }
}

#[derive(Deserialize)]
struct Page<T> {
    values: Vec<T>,
    next: Option<String>,
    size: Option<usize>,
}
