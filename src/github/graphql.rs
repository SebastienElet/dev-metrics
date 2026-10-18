use super::api::{Api, CommentDetail, FileDetail, Issue, PullRequestDetail, ReviewDetail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;

type PullRequestFiles = BTreeMap<(String, u64), Vec<FileDetail>>;

const COMMENT: &str = "id:fullDatabaseId user:author { login } created_at:createdAt html_url:url";

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Snapshot {
    pub detail: PullRequestDetail,
    pub reviews: Vec<ReviewDetail>,
    pub discussion: Vec<CommentDetail>,
    pub inline: Vec<CommentDetail>,
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
) -> Result<BTreeMap<(String, u64), Snapshot>, Box<dyn Error>> {
    let mut snapshots = BTreeMap::new();
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
        {
            snapshots.insert(key.clone(), snapshot);
        } else {
            keys.push(key.clone());
        }
    }
    let fetched = super::parallel::collect(
        keys.chunks(8).map(|chunk| chunk.to_vec()).collect(),
        |batch| {
            let mut snapshots = BTreeMap::new();
            let selection = format!(
                "number user:author {{ login }} title html_url:url created_at:createdAt merged_at:mergedAt additions deletions changed_files:changedFiles discussion:comments(first:100) {{ nodes {{ {COMMENT} }} pageInfo {{ hasNextPage }} }} reviews(first:100) {{ nodes {{ id:fullDatabaseId user:author {{ login }} submitted_at:submittedAt state html_url:url comments(first:100) {{ nodes {{ {COMMENT} }} pageInfo {{ hasNextPage }} }} }} pageInfo {{ hasNextPage }} }}"
            );
            let fields = batch
                .iter()
                .enumerate()
                .map(|(index, (repository, number))| field(repository, *number, index, &selection))
                .collect::<Vec<_>>()
                .join("\n");
            let mut data: BTreeMap<String, Repository<PullRequest>> = api.graphql(
                "PullRequestActivity",
                &format!("query PullRequestActivity {{ {fields} }}"),
            )?;
            for (index, (repository, number)) in batch.iter().enumerate() {
                let pull = data
                    .remove(&format!("p{index}"))
                    .ok_or("GitHub GraphQL : PR absente")?
                    .pull_request;
                if pull.detail.number != *number {
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
                        .filter(|review| {
                            review.detail.state != crate::activity::ReviewState::Pending
                        })
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
                    review.state != crate::activity::ReviewState::Pending
                        && review.submitted_at.is_none()
                }) {
                    return Err("Review GitHub soumise sans timestamp".into());
                }
                let key = (repository.clone(), *number);
                let snapshot = Snapshot {
                    detail: pull.detail,
                    reviews,
                    discussion,
                    inline,
                };
                let (slot, version) = cache_key(api, &key, &candidates[&key], "activity");
                if let Some(cache) = &api.cache {
                    cache.write(&slot, &version, &snapshot);
                }
                snapshots.insert(key, snapshot);
            }
            Ok(snapshots)
        },
    )?;
    snapshots.extend(fetched.into_iter().flatten());
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
) -> Result<PullRequestFiles, Box<dyn Error>> {
    let mut files = BTreeMap::new();
    let mut missing = Vec::new();
    for key in keys {
        let (slot, version) = cache_key(api, key, &candidates[key], "files");
        if let Some(cached) = api
            .cache
            .as_ref()
            .and_then(|cache| cache.read::<Vec<FileDetail>>(&slot, &version))
        {
            files.insert(key.clone(), cached);
        } else {
            missing.push(key.clone());
        }
    }
    let fetched = super::parallel::collect(
        missing.chunks(8).map(|chunk| chunk.to_vec()).collect(),
        |batch| {
            let mut files = BTreeMap::new();
            let fields = batch.iter().enumerate().map(|(index, (repository, number))| field(repository, *number, index, "files(first:100) { nodes { filename:path additions deletions changeType } pageInfo { hasNextPage } }")).collect::<Vec<_>>().join("\n");
            let mut data: BTreeMap<String, Repository<Files>> = api.graphql(
                "PullRequestFiles",
                &format!("query PullRequestFiles {{ {fields} }}"),
            )?;
            for (index, key) in batch.iter().enumerate() {
                let page = data
                    .remove(&format!("p{index}"))
                    .ok_or("GitHub GraphQL : fichiers absents")?
                    .pull_request
                    .files;
                if !page.page_info.has_next
                    && !page.nodes.iter().any(|file| file.change_type == "RENAMED")
                {
                    let changes = page
                        .nodes
                        .into_iter()
                        .map(|file| file.detail)
                        .collect::<Vec<_>>();
                    let (slot, version) = cache_key(api, key, &candidates[key], "files");
                    if let Some(cache) = &api.cache {
                        cache.write(&slot, &version, &changes);
                    }
                    files.insert(key.clone(), changes);
                }
            }
            Ok(files)
        },
    )?;
    files.extend(fetched.into_iter().flatten());
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
