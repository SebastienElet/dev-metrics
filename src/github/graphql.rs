use super::api::{Api, CommentDetail, FileDetail, Issue, PullRequestDetail, ReviewDetail};
use super::recovery::{self, Batch};
use crate::activity::WarningSubject;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;

type PullRequestFiles = Batch<Vec<FileDetail>>;

const FILES: &str = "files(first:100) { nodes { filename:path additions deletions changeType } pageInfo { hasNextPage } }";

const COMMENT: &str = "id:fullDatabaseId user:author { login } created_at:createdAt html_url:url";

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Snapshot {
    pub detail: PullRequestDetail,
    pub reviews: Vec<ReviewDetail>,
    pub discussion: Vec<CommentDetail>,
    pub inline: Vec<CommentDetail>,
    #[serde(skip)]
    pub files: Option<PrefetchedFiles>,
}

#[derive(Clone)]
pub(super) enum PrefetchedFiles {
    Complete(Vec<FileDetail>),
    RestRequired,
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next: bool,
}

#[derive(Deserialize)]
struct Connection<T> {
    nodes: Vec<T>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Deserialize)]
struct Review {
    #[serde(flatten)]
    detail: ReviewDetail,
    comments: Connection<CommentDetail>,
}

#[derive(Deserialize)]
struct PullRequest {
    #[serde(flatten)]
    detail: PullRequestDetail,
    reviews: Connection<Review>,
    discussion: Connection<CommentDetail>,
}

#[derive(Deserialize)]
struct Repository<T> {
    #[serde(rename = "pullRequest")]
    pull_request: T,
}

fn field(repository: &str, number: u64, index: usize, selection: &str) -> String {
    let (owner, name) = repository.split_once('/').unwrap();
    format!(
        "p{index}:repository(owner:{},name:{}) {{ pullRequest(number:{number}) {{ {selection} }} }}",
        serde_json::json!(owner),
        serde_json::json!(name)
    )
}

pub(super) fn snapshots(
    api: &Api,
    candidates: &BTreeMap<(String, u64), Issue>,
    user: &str,
    period: &crate::activity::Period,
) -> Result<Batch<Snapshot>, Box<dyn Error>> {
    let mut snapshots = Batch::empty();
    let mut keys = Vec::new();
    for (key, issue) in candidates
        .iter()
        .filter(|(_, issue)| issue.pull_request.is_some())
    {
        let (slot, version) = cache_key(api, key, issue, "activity");
        if let Some(snapshot) = api
            .cache
            .as_ref()
            .and_then(|cache| cache.read::<Snapshot>(&slot, &version))
            .filter(|snapshot| {
                snapshot.detail.number == key.1
                    && snapshot.reviews.iter().all(|review| {
                        review.state == crate::activity::ReviewState::Pending
                            || review.submitted_at.is_some()
                    })
            })
        {
            snapshots.items.insert(key.clone(), snapshot);
        } else {
            keys.push(key.clone());
        }
    }
    let fetched = super::parallel::collect(
        keys.chunks(8).map(|chunk| chunk.to_vec()).collect(),
        |batch| {
            let mut nodes = Batch::empty();
            let selection = format!(
                "number user:author {{ login }} title html_url:url created_at:createdAt merged_at:mergedAt additions deletions changed_files:changedFiles discussion:comments(first:100) {{ nodes {{ {COMMENT} }} pageInfo {{ hasNextPage }} }} reviews(first:100) {{ nodes {{ id:fullDatabaseId user:author {{ login }} submitted_at:submittedAt state html_url:url comments(first:100) {{ nodes {{ {COMMENT} }} pageInfo {{ hasNextPage }} }} }} pageInfo {{ hasNextPage }} }}"
            );
            let fields = batch
                .iter()
                .enumerate()
                .map(|(index, key)| {
                    let issue = &candidates[key];
                    let files = if issue
                        .user
                        .as_ref()
                        .is_some_and(|author| author.login.eq_ignore_ascii_case(user))
                        && (period.contains(issue.created_at)
                            || issue
                                .pull_request
                                .as_ref()
                                .and_then(|pull| pull.merged_at)
                                .is_some_and(|merged_at| period.contains(merged_at)))
                    {
                        FILES
                    } else {
                        ""
                    };
                    field(&key.0, key.1, index, &format!("{selection} {files}"))
                })
                .collect::<Vec<_>>()
                .join("\n");
            let response = api.graphql(
                "PullRequestActivity",
                &format!("query PullRequestActivity {{ {fields} }}"),
            );
            let mut data = match response {
                Ok(data) => data,
                Err(error) if super::api::is_rate_limit(error.as_ref()) => return Err(error),
                Err(error) => {
                    nodes
                        .warnings
                        .extend(batch.iter().map(|(repository, number)| {
                            recovery::warning(
                                repository,
                                WarningSubject::PullRequest { number: *number },
                                error.as_ref(),
                            )
                        }));
                    return Ok(nodes);
                }
            };
            for (index, (repository, number)) in batch.iter().enumerate() {
                let result = data.take(&format!("p{index}"), batch.len());
                let Some(value) = recovery::item(
                    result,
                    repository,
                    WarningSubject::PullRequest { number: *number },
                    &mut nodes.warnings,
                )?
                else {
                    continue;
                };
                let key = (repository.clone(), *number);
                nodes.items.insert(key, value);
            }
            Ok(nodes)
        },
    )?;
    let mut pending = Vec::new();
    for batch in fetched {
        snapshots.warnings.extend(batch.warnings);
        pending.extend(batch.items);
    }
    let loaded = super::parallel::collect(pending, |(key, value)| {
        let mut batch = Batch::empty();
        let Some(snapshot) = recovery::item(
            load_snapshot(api, &key.0, key.1, value),
            &key.0,
            WarningSubject::PullRequest { number: key.1 },
            &mut batch.warnings,
        )?
        else {
            return Ok(batch);
        };
        if let Some(PrefetchedFiles::Complete(files)) = &snapshot.files {
            cache_files(api, &key, &candidates[&key], files);
        }
        let (slot, version) = cache_key(api, &key, &candidates[&key], "activity");
        if let Some(cache) = &api.cache {
            cache.write(&slot, &version, &snapshot);
        }
        batch.items.insert(key, snapshot);
        Ok(batch)
    })?;
    for batch in loaded {
        snapshots.extend(batch);
    }
    Ok(snapshots)
}

#[derive(Deserialize)]
struct ChangedFile {
    #[serde(flatten)]
    detail: FileDetail,
    #[serde(rename = "changeType")]
    change_type: String,
}

#[derive(Deserialize)]
struct Files {
    files: Connection<ChangedFile>,
}

pub(super) fn files(
    api: &Api,
    keys: &[(String, u64)],
    candidates: &BTreeMap<(String, u64), Issue>,
    prefetched: &BTreeMap<(String, u64), PrefetchedFiles>,
) -> Result<PullRequestFiles, Box<dyn Error>> {
    let mut files = Batch::empty();
    let mut missing = Vec::new();
    for key in keys {
        let (slot, version) = cache_key(api, key, &candidates[key], "files");
        if let Some(page) = prefetched.get(key) {
            if let PrefetchedFiles::Complete(changes) = page {
                files.items.insert(key.clone(), changes.clone());
            }
            continue;
        }
        if let Some(cached) = api
            .cache
            .as_ref()
            .and_then(|cache| cache.read::<Vec<FileDetail>>(&slot, &version))
        {
            files.items.insert(key.clone(), cached);
        } else {
            missing.push(key.clone());
        }
    }
    let fetched = super::parallel::collect(
        missing.chunks(8).map(|chunk| chunk.to_vec()).collect(),
        |batch| {
            let mut files = Batch::empty();
            let fields = batch
                .iter()
                .enumerate()
                .map(|(index, (repository, number))| field(repository, *number, index, FILES))
                .collect::<Vec<_>>()
                .join("\n");
            let response = api.graphql(
                "PullRequestFiles",
                &format!("query PullRequestFiles {{ {fields} }}"),
            );
            let mut data = match response {
                Ok(data) => data,
                Err(error) if super::api::is_rate_limit(error.as_ref()) => return Err(error),
                Err(error) => {
                    files
                        .warnings
                        .extend(batch.iter().map(|(repository, number)| {
                            recovery::warning(
                                repository,
                                WarningSubject::PullRequest { number: *number },
                                error.as_ref(),
                            )
                        }));
                    return Ok(files);
                }
            };
            for (index, key) in batch.iter().enumerate() {
                let result = data
                    .take(&format!("p{index}"), batch.len())
                    .and_then(|value| {
                        serde_json::from_value::<Repository<Files>>(value)
                            .map(|repository| repository.pull_request.files)
                            .map_err(|_| "GitHub GraphQL : fichiers invalides".into())
                    });
                let Some(page) = recovery::item(
                    result,
                    &key.0,
                    WarningSubject::PullRequest { number: key.1 },
                    &mut files.warnings,
                )?
                else {
                    continue;
                };
                if let Some(changes) = complete_files(page) {
                    let (slot, version) = cache_key(api, key, &candidates[key], "files");
                    if let Some(cache) = &api.cache {
                        cache.write(&slot, &version, &changes);
                    }
                    files.items.insert(key.clone(), changes);
                }
            }
            Ok(files)
        },
    )?;
    for batch in fetched {
        files.extend(batch);
    }
    Ok(files)
}

pub(super) fn identifier<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Identifier {
        Number(u64),
        Text(String),
    }
    match Identifier::deserialize(deserializer)? {
        Identifier::Number(value) => Ok(value),
        Identifier::Text(value) => value.parse().map_err(serde::de::Error::custom),
    }
}

fn cache_key(api: &Api, key: &(String, u64), issue: &Issue, kind: &str) -> (String, String) {
    let slot = api.cache_key(&format!("{}/{}/{kind}", key.0, key.1));
    let version = format!("{slot}/{}", issue.updated_at);
    (slot, version)
}

pub(super) fn cache_files(api: &Api, key: &(String, u64), issue: &Issue, files: &[FileDetail]) {
    let (slot, version) = cache_key(api, key, issue, "files");
    if let Some(cache) = &api.cache {
        cache.write(&slot, &version, &files);
    }
}

fn load_snapshot(
    api: &Api,
    repository: &str,
    number: u64,
    value: serde_json::Value,
) -> Result<Snapshot, Box<dyn Error>> {
    let files = value
        .get("pullRequest")
        .cloned()
        .and_then(|value| serde_json::from_value::<Files>(value).ok())
        .map(|files| {
            complete_files(files.files)
                .map_or(PrefetchedFiles::RestRequired, PrefetchedFiles::Complete)
        });
    let pull = serde_json::from_value::<Repository<PullRequest>>(value)
        .map_err(|_| "Réponse GitHub GraphQL invalide pour la PR")?
        .pull_request;
    if pull.detail.number != number {
        return Err("GitHub GraphQL : numéro de PR incohérent".into());
    }
    let discussion = if pull.discussion.page_info.has_next {
        api.list(&format!("repos/{repository}/issues/{number}/comments"))?
    } else {
        pull.discussion.nodes
    };
    let incomplete_reviews = pull.reviews.page_info.has_next;
    let incomplete_inline = incomplete_reviews
        || pull
            .reviews
            .nodes
            .iter()
            .any(|review| review.comments.page_info.has_next);
    let inline = if incomplete_inline {
        api.list(&format!("repos/{repository}/pulls/{number}/comments"))?
    } else {
        pull.reviews
            .nodes
            .iter()
            .filter(|review| review.detail.state != crate::activity::ReviewState::Pending)
            .flat_map(|review| review.comments.nodes.clone())
            .collect()
    };
    let reviews = if incomplete_reviews {
        api.list(&format!("repos/{repository}/pulls/{number}/reviews"))?
    } else {
        pull.reviews
            .nodes
            .into_iter()
            .map(|review| review.detail)
            .collect()
    };
    if reviews.iter().any(|review| {
        review.state != crate::activity::ReviewState::Pending && review.submitted_at.is_none()
    }) {
        return Err("Review GitHub soumise sans timestamp".into());
    }
    let snapshot = Snapshot {
        detail: pull.detail,
        reviews,
        discussion,
        inline,
        files,
    };
    Ok(snapshot)
}

fn complete_files(page: Connection<ChangedFile>) -> Option<Vec<FileDetail>> {
    (!page.page_info.has_next && !page.nodes.iter().any(|file| file.change_type == "RENAMED"))
        .then(|| page.nodes.into_iter().map(|file| file.detail).collect())
}
