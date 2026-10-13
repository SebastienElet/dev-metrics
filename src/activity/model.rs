use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::error::Error;

#[derive(Debug, Clone, Serialize)]
pub struct Period {
    pub(crate) from: NaiveDate,
    pub(crate) to: NaiveDate,
}

impl Period {
    pub fn parse(from: &str, to: &str) -> Result<Self, Box<dyn Error>> {
        let parse = |value: &str| -> Result<NaiveDate, Box<dyn Error>> {
            if value.len() != 10 {
                return Err("Date invalide : format YYYY-MM-DD attendu".into());
            }
            let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| "Date invalide : format YYYY-MM-DD attendu")?;
            if date.format("%Y-%m-%d").to_string() != value {
                return Err("Date invalide : format YYYY-MM-DD attendu".into());
            }
            Ok(date)
        };
        let from = parse(from)?;
        let to = parse(to)?;
        if from > to {
            return Err("La date de début doit précéder ou égaler la date de fin".into());
        }
        Ok(Self { from, to })
    }

    pub fn contains(&self, timestamp: DateTime<Utc>) -> bool {
        (self.from..=self.to).contains(&timestamp.date_naive())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    GitHub,
    Bitbucket,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileChange {
    #[serde(rename(deserialize = "filename"))]
    pub path: String,
    #[serde(rename(deserialize = "previous_filename"))]
    pub previous_path: Option<String>,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRequest {
    pub source: Source,
    pub repository: String,
    pub number: u64,
    pub author: Option<String>,
    pub title: String,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
    pub first_review_received_at: Option<DateTime<Utc>>,
    pub additions: u64,
    pub deletions: u64,
    pub files: Vec<FileChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    Pending,
}

#[derive(Debug, Serialize)]
pub struct Review {
    pub source: Source,
    pub repository: String,
    pub pull_request_number: u64,
    pub id: u64,
    pub author: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub state: ReviewState,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentKind {
    Discussion,
    Review,
}

#[derive(Debug, Serialize)]
pub struct Comment {
    pub source: Source,
    pub repository: String,
    pub issue_number: u64,
    pub id: u64,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub url: String,
    pub kind: CommentKind,
}

#[derive(Debug, Serialize)]
pub struct Commit {
    pub source: Source,
    pub repository: String,
    pub sha: String,
    pub message: String,
    pub url: String,
    pub authored_at: DateTime<Utc>,
    pub additions: u64,
    pub deletions: u64,
    pub files: Vec<FileChange>,
}

#[derive(Debug, Serialize)]
pub struct Activity {
    pub schema_version: u32,
    pub source: Source,
    pub user: String,
    pub period: Period,
    pub pull_requests: Vec<PullRequest>,
    pub reviews_given: Vec<Review>,
    pub reviews_received: Vec<Review>,
    pub comments: Vec<Comment>,
    pub commits: Vec<Commit>,
}

impl Activity {
    pub(crate) fn empty(source: Source, user: &str, period: Period) -> Self {
        Self {
            schema_version: 1,
            source,
            user: user.into(),
            period,
            pull_requests: Vec::new(),
            reviews_given: Vec::new(),
            reviews_received: Vec::new(),
            comments: Vec::new(),
            commits: Vec::new(),
        }
    }
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::Bitbucket => "bitbucket",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::Bitbucket => "Bitbucket",
        }
    }
}
