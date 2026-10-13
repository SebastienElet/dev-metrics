use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct User {
    pub uuid: String,
}

#[derive(Deserialize)]
pub(super) struct Repository {
    pub full_name: String,
}

#[derive(Deserialize)]
pub(super) struct Links {
    pub html: Link,
}
#[derive(Deserialize)]
pub(super) struct Link {
    pub href: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum PullRequestState {
    Open,
    Merged,
    Declined,
    Superseded,
}

#[derive(Deserialize)]
pub(super) struct PullRequestDetail {
    pub id: u64,
    pub author: Option<User>,
    pub title: String,
    pub state: PullRequestState,
    pub created_on: DateTime<Utc>,
    pub links: Links,
}

#[derive(Deserialize)]
pub(super) struct Event {
    pub approval: Option<ReviewAction>,
    pub changes_requested: Option<ReviewAction>,
    pub update: Option<Update>,
    pub comment: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub(super) struct ReviewAction {
    pub date: DateTime<Utc>,
    pub user: Option<User>,
}

#[derive(Deserialize)]
pub(super) struct Update {
    pub date: DateTime<Utc>,
    pub state: PullRequestState,
}

#[derive(Deserialize)]
pub(super) struct CommentDetail {
    pub id: u64,
    pub user: Option<User>,
    pub created_on: DateTime<Utc>,
    pub deleted: bool,
    #[serde(default)]
    pub pending: bool,
    pub inline: Option<serde_json::Value>,
    pub links: Links,
}

#[derive(Deserialize)]
pub(super) struct DiffStat {
    pub old: Option<File>,
    pub new: Option<File>,
    pub lines_added: u64,
    pub lines_removed: u64,
}

#[derive(Deserialize)]
pub(super) struct File {
    pub path: String,
}

#[derive(Deserialize)]
pub(super) struct CommitDetail {
    pub hash: String,
    pub author: CommitAuthor,
    pub date: DateTime<Utc>,
    pub message: String,
    pub links: Links,
}

#[derive(Deserialize)]
pub(super) struct CommitAuthor {
    pub user: Option<User>,
}
