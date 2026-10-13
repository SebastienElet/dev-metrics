use crate::activity::{FileChange, ReviewState};
use chrono::{DateTime, Utc};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, LINK};
use reqwest::{Url, redirect, retry};
use serde::{Deserialize, de::DeserializeOwned};
use std::error::Error;
use std::time::Duration;

pub(super) struct Api {
    base: Url,
    client: Client,
}

impl Api {
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
        Ok(Self { base, client })
    }

    pub fn get<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        query: &[(&str, String)],
    ) -> Result<(T, bool), Box<dyn Error>> {
        let url = self.base.join(endpoint)?;
        let response = self.client.get(url).query(query).send().map_err(|error| {
            let reason = if error.is_timeout() {
                "timeout"
            } else {
                "échec réseau"
            };
            format!("GitHub {endpoint} : {reason}")
        })?;
        if !response.status().is_success() {
            return Err(format!("GitHub {endpoint} : HTTP {} (vérifiez le token, les permissions et les limites API)", response.status().as_u16()).into());
        }
        let next = response
            .headers()
            .get(LINK)
            .map(|value| value.to_str())
            .transpose()?
            .is_some_and(|link| link.contains("rel=\"next\""));
        let body = response
            .json()
            .map_err(|_| format!("Réponse GitHub invalide pour {endpoint}"))?;
        Ok((body, next))
    }

    pub fn search<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        query: &str,
    ) -> Result<Vec<T>, Box<dyn Error>> {
        let mut items = Vec::new();
        let mut expected_total = None;
        for number in 1..=10 {
            let (page, _): (SearchPage<T>, _) = self.get(
                endpoint,
                &[
                    ("q", query.into()),
                    ("per_page", "100".into()),
                    ("page", number.to_string()),
                ],
            )?;
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
        }
        Err("Pagination de recherche GitHub incomplète".into())
    }

    pub fn list<T: DeserializeOwned>(&self, endpoint: &str) -> Result<Vec<T>, Box<dyn Error>> {
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

#[derive(Deserialize)]
struct SearchPage<T> {
    total_count: usize,
    incomplete_results: bool,
    items: Vec<T>,
}

#[derive(Deserialize)]
pub(super) struct User {
    pub login: String,
}

#[derive(Deserialize)]
pub(super) struct Issue {
    pub number: u64,
    pub repository_url: String,
    pub pull_request: Option<serde_json::Value>,
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

#[derive(Deserialize)]
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

#[derive(Deserialize)]
pub(super) struct ReviewDetail {
    pub id: u64,
    pub user: Option<User>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub state: ReviewState,
    pub html_url: String,
}

#[derive(Deserialize)]
pub(super) struct CommentDetail {
    pub id: u64,
    pub user: Option<User>,
    pub created_at: DateTime<Utc>,
    pub html_url: String,
}

#[derive(Deserialize)]
pub(super) struct Repository {
    pub full_name: String,
}

#[derive(Deserialize)]
pub(super) struct CommitReference {
    pub sha: String,
    pub repository: Repository,
}

#[derive(Deserialize)]
pub(super) struct CommitDetail {
    pub sha: String,
    pub html_url: String,
    pub commit: CommitMetadata,
    pub stats: Statistics,
    pub files: Vec<FileChange>,
}

#[derive(Deserialize)]
pub(super) struct CommitMetadata {
    pub message: String,
    pub author: CommitAuthor,
}

#[derive(Deserialize)]
pub(super) struct CommitAuthor {
    pub date: DateTime<Utc>,
}

#[derive(Deserialize)]
pub(super) struct Statistics {
    pub additions: u64,
    pub deletions: u64,
}
