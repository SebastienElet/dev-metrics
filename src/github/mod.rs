mod api;
mod auth;

pub use crate::activity::*;
use crate::progress::{Event, Progress};
use api::{
    Api, CommentDetail, CommitDetail, CommitReference, FileDetail, Issue, PullRequestDetail,
    ReviewDetail,
};
pub use auth::resolve_token;
use reqwest::Url;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;

pub struct GitHubSource {
    api: Api,
}

impl ActivitySource for GitHubSource {
    fn collect_activity(
        &self,
        user: Option<&str>,
        period: Period,
    ) -> Result<Activity, Box<dyn Error>> {
        let authenticated;
        let user = match user {
            Some(user) => user,
            None => {
                authenticated = self.authenticated_user()?;
                &authenticated
            }
        };
        self.activity(user, period)
    }
}

impl GitHubSource {
    pub fn with_progress(mut self, progress: Progress) -> Self {
        self.api.progress = progress;
        self
    }

    pub fn new(api_base: Url, token: &str) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            api: Api::new(api_base, token)?,
        })
    }

    pub fn authenticated_user(&self) -> Result<String, Box<dyn Error>> {
        self.api.progress.emit(Event::Identifying(Source::GitHub));
        let (user, _): (api::User, _) = self.api.get("user", &[])?;
        Ok(user.login)
    }

    pub fn activity(&self, user: &str, period: Period) -> Result<Activity, Box<dyn Error>> {
        self.api.progress.emit(Event::Source(Source::GitHub));
        if user.is_empty()
            || user.len() > 39
            || !user.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err("Utilisateur GitHub invalide".into());
        }
        let mut activity = Activity::empty(Source::GitHub, user, period);
        let mut candidates = BTreeMap::new();
        for query in [
            format!("is:pr author:{user} updated:>={}", activity.period.from),
            format!(
                "is:pr reviewed-by:{user} updated:>={}",
                activity.period.from
            ),
            format!("is:pr commenter:{user} updated:>={}", activity.period.from),
            format!(
                "is:issue commenter:{user} updated:>={}",
                activity.period.from
            ),
        ] {
            let mut query_keys = BTreeSet::new();
            for issue in self.api.search::<Issue>("search/issues", &query)? {
                let repository = issue.repository()?;
                if !query_keys.insert((repository.clone(), issue.number)) {
                    return Err(
                        "Recherche GitHub avec résultats dupliqués ; relancez la collecte".into(),
                    );
                }
                candidates.insert((repository, issue.number), issue.pull_request.is_some());
            }
        }
        let total = candidates
            .keys()
            .map(|(repository, _)| repository)
            .collect::<BTreeSet<_>>()
            .len();
        let mut visited = BTreeSet::new();
        self.api.progress.emit(Event::Repositories {
            source: Source::GitHub,
            stage: "PRs et commentaires",
            current: 0,
            total,
        });
        for ((repository, number), is_pr) in candidates {
            if visited.insert(repository.clone()) {
                self.api.progress.emit(Event::Repositories {
                    source: Source::GitHub,
                    stage: "PRs et commentaires",
                    current: visited.len(),
                    total,
                });
            }
            self.comments(&mut activity, &repository, number, CommentKind::Discussion)?;
            if !is_pr {
                continue;
            }
            self.comments(&mut activity, &repository, number, CommentKind::Review)?;
            let endpoint = format!("repos/{repository}/pulls/{number}");
            let (detail, _): (PullRequestDetail, _) = self.api.get(&endpoint, &[])?;
            let author = detail.user.map(|u| u.login);
            let owned = author
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(user));
            let mut first_review_received_at = None;
            let mut reviewed_in_period = false;
            let mut received_in_period = false;
            for review in self
                .api
                .list::<ReviewDetail>(&format!("{endpoint}/reviews"))?
            {
                if review.state == ReviewState::Pending {
                    continue;
                }
                let submitted_at = review
                    .submitted_at
                    .ok_or("Review GitHub soumise sans timestamp")?;
                let reviewer = review.user.map(|u| u.login);
                if reviewer
                    .as_deref()
                    .zip(author.as_deref())
                    .is_some_and(|(r, a)| r.eq_ignore_ascii_case(a))
                {
                    continue;
                }
                first_review_received_at = Some(
                    first_review_received_at
                        .map_or(submitted_at, |previous: chrono::DateTime<chrono::Utc>| {
                            previous.min(submitted_at)
                        }),
                );
                if !activity.period.contains(submitted_at) {
                    continue;
                }
                let given = reviewer
                    .as_deref()
                    .is_some_and(|r| r.eq_ignore_ascii_case(user))
                    && !owned;
                if !given && !owned {
                    continue;
                }
                let record = Review {
                    source: Source::GitHub,
                    repository: repository.clone(),
                    pull_request_number: number,
                    id: review.id,
                    author: reviewer,
                    submitted_at,
                    state: review.state,
                    url: review.html_url,
                };
                if owned {
                    received_in_period = true;
                    activity.reviews_received.push(record);
                } else {
                    reviewed_in_period = true;
                    activity.reviews_given.push(record);
                }
            }
            let authored_in_period = owned
                && (activity.period.contains(detail.created_at)
                    || detail
                        .merged_at
                        .is_some_and(|t| activity.period.contains(t)));
            let commented_in_period = activity
                .comments
                .iter()
                .any(|c| c.repository == repository && c.issue_number == number);
            if authored_in_period || reviewed_in_period || received_in_period || commented_in_period
            {
                let subject = WarningSubject::PullRequest { number };
                if detail.changed_files > 3000 {
                    activity.warnings.push(warning(
                        &repository,
                        &subject,
                        WarningCode::FileLimitReached,
                        format!(
                            "{} fichiers annoncés ; GitHub limite le détail à 3000 fichiers",
                            detail.changed_files
                        ),
                    ));
                }
                let mut files = Vec::new();
                for page_number in 1..=30 {
                    let (page, next): (Vec<FileDetail>, _) = self.api.get(
                        &format!("{endpoint}/files"),
                        &[
                            ("per_page", "100".into()),
                            ("page", page_number.to_string()),
                        ],
                    )?;
                    let empty = page.is_empty();
                    files.extend(page);
                    if !next {
                        break;
                    }
                    if empty || files.len() >= 3000 || page_number == 30 {
                        activity.warnings.push(warning(
                            &repository,
                            &subject,
                            WarningCode::FilePaginationIncomplete,
                            "Pagination des fichiers de PR interrompue avant la dernière page",
                        ));
                        break;
                    }
                }
                let files = file_changes(
                    &mut activity,
                    &repository,
                    &subject,
                    files,
                    detail.additions,
                    detail.deletions,
                );
                if detail.changed_files != files.len() {
                    activity.warnings.push(warning(&repository, &subject, WarningCode::FileCountMismatch,
                        format!("{} fichiers annoncés, {} fichiers disponibles ; exclusions limitées aux fichiers disponibles", detail.changed_files, files.len())));
                }
                activity.pull_requests.push(PullRequest {
                    source: Source::GitHub,
                    repository,
                    number: detail.number,
                    author,
                    title: detail.title,
                    url: detail.html_url,
                    created_at: detail.created_at,
                    merged_at: detail.merged_at,
                    first_review_received_at,
                    additions: detail.additions,
                    deletions: detail.deletions,
                    files,
                });
            }
        }
        let query = format!(
            "author:{user} author-date:{}..{}",
            activity.period.from, activity.period.to
        );
        let references = self
            .api
            .search::<CommitReference>("search/commits", &query)?;
        let mut commit_keys = BTreeSet::new();
        for reference in references {
            if !commit_keys.insert((reference.repository.full_name, reference.sha)) {
                return Err(
                    "Recherche GitHub avec résultats dupliqués ; relancez la collecte".into(),
                );
            }
        }
        let total = commit_keys
            .iter()
            .map(|(repository, _)| repository)
            .collect::<BTreeSet<_>>()
            .len();
        let mut visited = BTreeSet::new();
        self.api.progress.emit(Event::Repositories {
            source: Source::GitHub,
            stage: "commits",
            current: 0,
            total,
        });
        for (repository, sha) in commit_keys {
            if visited.insert(repository.clone()) {
                self.api.progress.emit(Event::Repositories {
                    source: Source::GitHub,
                    stage: "commits",
                    current: visited.len(),
                    total,
                });
            }
            api::validate_repository(&repository)?;
            if sha.is_empty() || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err("SHA GitHub invalide".into());
            }
            let endpoint = format!("repos/{repository}/commits/{sha}");
            let (mut detail, mut next): (CommitDetail, _) = self.api.get(
                &endpoint,
                &[("per_page", "100".into()), ("page", "1".into())],
            )?;
            let subject = WarningSubject::Commit { sha: sha.clone() };
            let mut number = 2;
            while next {
                if detail.files.len() >= 3000 {
                    break;
                }
                if detail.files.is_empty() || number > 30 {
                    activity.warnings.push(warning(
                        &repository,
                        &subject,
                        WarningCode::FilePaginationIncomplete,
                        "Pagination des fichiers de commit interrompue avant la dernière page",
                    ));
                    break;
                }
                let (page, more): (CommitDetail, _) = self.api.get(
                    &endpoint,
                    &[("per_page", "100".into()), ("page", number.to_string())],
                )?;
                if page.sha != detail.sha
                    || page.stats.additions != detail.stats.additions
                    || page.stats.deletions != detail.stats.deletions
                {
                    activity.warnings.push(warning(&repository, &subject, WarningCode::FilePaginationInconsistent,
                        "Métadonnées de commit différentes entre les pages ; totaux de la première page conservés"));
                }
                if page.sha != detail.sha {
                    break;
                }
                if page.files.is_empty() {
                    activity.warnings.push(warning(
                        &repository,
                        &subject,
                        WarningCode::FilePaginationIncomplete,
                        "Page de fichiers de commit vide après une page annonçant une suite",
                    ));
                    break;
                }
                detail.files.extend(page.files);
                next = more;
                number += 1;
            }
            if detail.files.len() >= 3000 {
                activity.warnings.push(warning(
                    &repository,
                    &subject,
                    WarningCode::FileLimitReached,
                    "Limite GitHub de 3000 fichiers atteinte ; le diff peut être tronqué",
                ));
            }
            let files = file_changes(
                &mut activity,
                &repository,
                &subject,
                detail.files,
                detail.stats.additions,
                detail.stats.deletions,
            );
            activity.commits.push(Commit {
                source: Source::GitHub,
                repository,
                sha: detail.sha,
                message: detail.commit.message,
                url: detail.html_url,
                authored_at: detail.commit.author.date,
                additions: detail.stats.additions,
                deletions: detail.stats.deletions,
                files,
            });
        }
        Ok(activity)
    }

    fn comments(
        &self,
        activity: &mut Activity,
        repository: &str,
        number: u64,
        kind: CommentKind,
    ) -> Result<(), Box<dyn Error>> {
        let segment = match kind {
            CommentKind::Discussion => "issues",
            CommentKind::Review => "pulls",
        };
        let endpoint = format!("repos/{repository}/{segment}/{number}/comments");
        for comment in self.api.list::<CommentDetail>(&endpoint)? {
            let Some(author) = comment.user.map(|u| u.login) else {
                continue;
            };
            if !author.eq_ignore_ascii_case(&activity.user)
                || !activity.period.contains(comment.created_at)
            {
                continue;
            }
            activity.comments.push(Comment {
                source: Source::GitHub,
                repository: repository.into(),
                issue_number: number,
                id: comment.id,
                author,
                created_at: comment.created_at,
                url: comment.html_url,
                kind,
            });
        }
        Ok(())
    }
}

fn warning(
    repository: &str,
    subject: &WarningSubject,
    code: WarningCode,
    message: impl Into<String>,
) -> ActivityWarning {
    ActivityWarning {
        source: Source::GitHub,
        repository: repository.into(),
        subject: subject.clone(),
        code,
        message: message.into(),
    }
}

fn file_changes(
    activity: &mut Activity,
    repository: &str,
    subject: &WarningSubject,
    files: Vec<FileDetail>,
    additions: u64,
    deletions: u64,
) -> Vec<FileChange> {
    let mut paths = BTreeSet::new();
    let mut missing = 0;
    let mut duplicates = 0;
    let files: Vec<_> = files
        .into_iter()
        .filter_map(|file| {
            if !paths.insert(file.filename.clone()) {
                duplicates += 1;
                return None;
            }
            if file.additions.is_none() || file.deletions.is_none() {
                missing += 1;
            }
            Some(FileChange {
                path: file.filename,
                previous_path: file.previous_filename,
                additions: file.additions.unwrap_or(0),
                deletions: file.deletions.unwrap_or(0),
            })
        })
        .collect();
    if duplicates > 0 {
        activity.warnings.push(warning(
            repository,
            subject,
            WarningCode::DuplicateFiles,
            format!("{duplicates} fichiers dupliqués ignorés ; première occurrence conservée"),
        ));
    }
    if missing > 0 {
        activity.warnings.push(warning(repository, subject, WarningCode::FileStatisticsMissing,
            format!("Statistiques absentes pour {missing} fichiers ; totaux API conservés, exclusions limitées aux statistiques disponibles")));
    }
    let (added, deleted) = files
        .iter()
        .fold((0_u128, 0_u128), |(added, deleted), file| {
            (
                added + u128::from(file.additions),
                deleted + u128::from(file.deletions),
            )
        });
    if (added, deleted) != (u128::from(additions), u128::from(deletions)) {
        activity.warnings.push(warning(repository, subject, WarningCode::FileStatisticsMismatch,
            format!("Totaux API +{additions}/-{deletions}, détail disponible +{added}/-{deleted} ; totaux API conservés, exclusions limitées aux statistiques disponibles")));
    }
    files
}
