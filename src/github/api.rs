use crate::activity::{ReviewState, Source};
use crate::progress::Event;
use chrono::{DateTime, Utc};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, LINK};
use reqwest::{Url, redirect, retry};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::error::Error;
use std::time::Duration;

#[derive(Clone)]
pub(super) struct Api {
    base: Url,
    client: Client,
    quota: std::sync::Arc<crate::http::RateLimit>,
    pub cache: Option<super::cache::Cache>,
    pub events: Option<std::sync::mpsc::Sender<Event>>,
}

impl Api {
    pub fn emit(&self, event: Event) {
        if let Some(events) = &self.events {
            let _ = events.send(event);
        }
    }

    pub fn new(base: Url, token: &str) -> Result<Self, Box<dyn Error>> {
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "Token GitHub invalide")?;
        authorization.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2026-03-10"),
        );
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
            events: None,
            cache: None,
            quota: std::sync::Arc::default(),
        })
    }

    pub fn get<T: DeserializeOwned + Serialize>(
        &self,
        endpoint: &str,
        query: &[(&str, String)],
    ) -> Result<(T, bool), Box<dyn Error>> {
        let mut url = self.base.join(endpoint)?;
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
        }
        let cache_key = self.cache_key(url.as_str());
        let cache = self.cache.as_ref().filter(|_| {
            matches!(
                endpoint.split('/').collect::<Vec<_>>().as_slice(),
                ["repos", _, _, "commits", _]
            )
        });
        if let Some((body, next)) =
            cache.and_then(|cache| cache.read::<(T, bool)>(&cache_key, &cache_key))
        {
            return Ok((body, next));
        }
        let operation = match endpoint.split('/').collect::<Vec<_>>().as_slice() {
            ["user"] => "identité",
            ["search", "issues"] => "recherche PRs et commentaires",
            ["search", "commits"] => "recherche commits",
            ["repos", _, _, "pulls", _] => "PR",
            ["repos", _, _, "commits", _] => "commit et fichiers",
            ["repos", _, _, _, _, "comments"] => "commentaires",
            ["repos", _, _, _, _, "reviews"] => "reviews",
            ["repos", _, _, _, _, "files"] => "fichiers",
            _ => "ressource",
        };
        if let Some(number) = query
            .iter()
            .find(|(key, _)| *key == "page")
            .and_then(|(_, value)| value.parse().ok())
        {
            self.emit(Event::Page {
                source: Source::GitHub,
                operation,
                number,
            });
        }
        let response = crate::http::send(
            || self.client.get(url.clone()),
            Source::GitHub,
            match endpoint.split('/').collect::<Vec<_>>().as_slice() {
                ["user"] => "/user",
                ["search", "issues"] => "/search/issues",
                ["search", "commits"] => "/search/commits",
                ["repos", _, _, "pulls", _] => "/repos/{owner}/{repo}/pulls/{number}",
                ["repos", _, _, "commits", _] => "/repos/{owner}/{repo}/commits/{sha}",
                ["repos", _, _, "issues", _, "comments"] => {
                    "/repos/{owner}/{repo}/issues/{number}/comments"
                }
                ["repos", _, _, "pulls", _, "comments"] => {
                    "/repos/{owner}/{repo}/pulls/{number}/comments"
                }
                ["repos", _, _, "pulls", _, "reviews"] => {
                    "/repos/{owner}/{repo}/pulls/{number}/reviews"
                }
                ["repos", _, _, "pulls", _, "files"] => {
                    "/repos/{owner}/{repo}/pulls/{number}/files"
                }
                _ => "ressource",
            },
            "GET",
            &|event| self.emit(event),
            Some(&self.quota),
        )
        .map_err(|error| request_error(endpoint, error))?;
        if !response.status().is_success() {
            return Err(format!("GitHub {endpoint} : HTTP {}", response.status().as_u16()).into());
        }
        let next = response
            .headers()
            .get(LINK)
            .map(|value| value.to_str())
            .transpose()?
            .is_some_and(|link| link.contains("rel=\"next\""));
        let body: T = response
            .json()
            .map_err(|_| format!("Réponse GitHub invalide pour {endpoint}"))?;
        if let Some(cache) = cache {
            cache.write(&cache_key, &cache_key, &(&body, next));
        }
        Ok((body, next))
    }

    pub fn cache_key(&self, resource: &str) -> String {
        format!("{}{resource}", self.base)
    }

    pub fn graphql(&self, operation: &str, query: &str) -> Result<GraphData, Box<dyn Error>> {
        let url = self.base.join("graphql")?;
        let body = serde_json::json!({"operationName":operation,"query":query});
        let response = crate::http::send(
            || self.client.post(url.clone()).json(&body),
            Source::GitHub,
            "/graphql",
            "POST",
            &|event| self.emit(event),
            Some(&self.quota),
        )
        .map_err(|error| request_error("GraphQL", error))?;
        if !response.status().is_success() {
            return Err(format!("GitHub GraphQL : HTTP {}", response.status().as_u16()).into());
        }
        response
            .json()
            .map_err(|_| "Réponse GitHub GraphQL invalide".into())
    }

    pub fn search<T: DeserializeOwned + Serialize + Send>(
        &self,
        endpoint: &str,
        query: &str,
    ) -> Result<Vec<T>, Box<dyn Error>> {
        let mut items = Vec::new();
        let mut expected_total = None;
        let mut prefetched = std::collections::BTreeMap::new();
        for number in 1..=10 {
            let (page, _): (SearchPage<T>, _) = match prefetched.remove(&number) {
                Some(page) => page,
                None => self.get(
                    endpoint,
                    &[
                        ("q", query.into()),
                        ("per_page", "100".into()),
                        ("page", number.to_string()),
                    ],
                )?,
            };
            if page.incomplete_results {
                return Err(format!("Recherche GitHub incomplète pour {endpoint}").into());
            }
            if page.total_count > 1000 {
                return Err(
                    "Recherche GitHub au-delà de 1000 résultats ; réduisez la période".into(),
                );
            }
            if expected_total.is_some_and(|total| total != page.total_count) {
                return Err(
                    "Résultats GitHub modifiés pendant la pagination ; relancez la collecte".into(),
                );
            }
            expected_total = Some(page.total_count);
            let empty = page.items.is_empty();
            items.extend(page.items);
            if items.len() == page.total_count {
                return Ok(items);
            }
            if empty || items.len() > page.total_count {
                return Err("Pagination de recherche GitHub incohérente".into());
            }
            if endpoint == "search/commits" && number == 1 && items.len() == 100 {
                prefetched.extend(super::parallel::collect(
                    (2..=page.total_count.div_ceil(100)).collect(),
                    |number| {
                        let page: (SearchPage<T>, bool) = self.get(
                            endpoint,
                            &[
                                ("q", query.into()),
                                ("per_page", "100".into()),
                                ("page", number.to_string()),
                            ],
                        )?;
                        Ok((number, page))
                    },
                )?);
            }
        }
        Err("Pagination de recherche GitHub incomplète".into())
    }

    pub fn list<T: DeserializeOwned + Serialize>(
        &self,
        endpoint: &str,
    ) -> Result<Vec<T>, Box<dyn Error>> {
        let mut items = Vec::new();
        let mut number = 1;
        loop {
            let (page, next): (Vec<T>, _) = self.get(
                endpoint,
                &[("per_page", "100".into()), ("page", number.to_string())],
            )?;
            if page.is_empty() && next {
                return Err("Pagination GitHub vide avec une page suivante".into());
            }
            items.extend(page);
            if !next {
                return Ok(items);
            }
            number += 1;
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct SearchPage<T> {
    total_count: usize,
    incomplete_results: bool,
    items: Vec<T>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct User {
    pub login: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Issue {
    pub number: u64,
    pub repository_url: String,
    pub pull_request: Option<PullRequestReference>,
    pub user: Option<User>,
    pub created_at: DateTime<Utc>,
    pub comments: u64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct PullRequestReference {
    pub merged_at: Option<DateTime<Utc>>,
}

impl Issue {
    pub fn repository(&self) -> Result<String, Box<dyn Error>> {
        let url = Url::parse(&self.repository_url).map_err(|_| "Dépôt GitHub invalide")?;
        let segments: Vec<_> = url
            .path_segments()
            .ok_or("Dépôt GitHub invalide")?
            .collect();
        match segments.as_slice() {
            ["repos", owner, repository] => {
                let name = format!("{owner}/{repository}");
                validate_repository(&name)?;
                Ok(name)
            }
            _ => Err("Dépôt GitHub invalide".into()),
        }
    }
}

pub(super) fn validate_repository(value: &str) -> Result<(), Box<dyn Error>> {
    let Some((owner, name)) = value.split_once('/') else {
        return Err("Dépôt GitHub invalide".into());
    };
    if owner.is_empty()
        || !owner
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        || name.is_empty()
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    {
        return Err("Dépôt GitHub invalide".into());
    }
    Ok(())
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct PullRequestDetail {
    pub number: u64,
    pub user: Option<User>,
    pub title: String,
    pub html_url: String,
    pub created_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: usize,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct ReviewDetail {
    #[serde(deserialize_with = "super::graphql::identifier")]
    pub id: u64,
    pub user: Option<User>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub state: ReviewState,
    pub html_url: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct CommentDetail {
    #[serde(deserialize_with = "super::graphql::identifier")]
    pub id: u64,
    pub user: Option<User>,
    pub created_at: DateTime<Utc>,
    pub html_url: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Repository {
    pub full_name: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct CommitReference {
    pub sha: String,
    pub repository: Repository,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct CommitDetail {
    pub sha: String,
    pub html_url: String,
    pub commit: CommitMetadata,
    pub stats: Statistics,
    pub files: Vec<FileDetail>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct CommitMetadata {
    pub message: String,
    pub author: CommitAuthor,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct CommitAuthor {
    pub date: DateTime<Utc>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Statistics {
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct FileDetail {
    pub filename: String,
    pub previous_filename: Option<String>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Deserialize)]
pub(super) struct GraphData {
    data: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    errors: Option<Vec<GraphError>>,
}

#[derive(Deserialize)]
struct GraphError {
    path: Option<Vec<serde_json::Value>>,
}

impl GraphData {
    pub fn take(&mut self, alias: &str, count: usize) -> Result<serde_json::Value, Box<dyn Error>> {
        if self.errors.as_ref().is_some_and(|errors| {
            errors.iter().any(|error| {
                error
                    .path
                    .as_ref()
                    .and_then(|path| path.first())
                    .and_then(|part| part.as_str())
                    .is_none_or(|path| {
                        path == alias
                            || path
                                .strip_prefix('p')
                                .and_then(|index| index.parse::<usize>().ok())
                                .is_none_or(|index| index >= count || path != format!("p{index}"))
                    })
            })
        }) {
            return Err("GitHub GraphQL : élément inaccessible ou réponse partielle".into());
        }
        self.data
            .as_mut()
            .and_then(|data| data.remove(alias))
            .filter(|value| !value.is_null())
            .ok_or_else(|| "GitHub GraphQL : élément absent".into())
    }
}

#[derive(Debug)]
struct RequestError {
    operation: String,
    cause: Box<dyn Error>,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "GitHub {} : {}", self.operation, self.cause)
    }
}
impl Error for RequestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

fn request_error(operation: &str, error: Box<dyn Error>) -> Box<dyn Error> {
    let cause = match error.downcast::<reqwest::Error>() {
        Ok(error) if error.is_timeout() => "timeout".into(),
        Ok(_) => "échec réseau".into(),
        Err(error) => error,
    };
    Box::new(RequestError {
        operation: operation.into(),
        cause,
    })
}

pub(super) fn is_rate_limit(error: &(dyn Error + 'static)) -> bool {
    error.is::<crate::http::RateLimitError>() || error.source().is_some_and(is_rate_limit)
}
