mod api;
mod auth;
mod cache;
mod graphql;
mod parallel;
mod recovery;

pub use crate::activity::*;
use crate::progress::{Event, Progress};
use api::{Api, CommentDetail, CommitDetail, CommitReference, FileDetail, Issue};
pub use auth::resolve_token;
use reqwest::Url;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;

pub struct GitHubSource {
    api: Api,
    progress: Progress,
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
    pub fn with_cache(mut self, directory: std::path::PathBuf) -> Self {
        self.api.cache = Some(cache::Cache { directory });
        self
    }

    pub fn with_progress(mut self, progress: Progress) -> Self {
        self.progress = progress;
        self
    }

    pub fn new(api_base: Url, token: &str) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            api: Api::new(api_base, token)?,
            progress: Progress::default(),
        })
    }

    fn run<T: Send>(
        &self,
        collect: impl FnOnce(Collector) -> Result<T, Box<dyn Error>> + Send,
    ) -> Result<T, Box<dyn Error>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut api = self.api.clone();
        api.events = Some(sender);
        std::thread::scope(|scope| {
            let task =
                scope.spawn(move || collect(Collector { api }).map_err(|error| error.to_string()));
            let mut calls = BTreeMap::new();
            let mut waited = std::time::Duration::ZERO;
            for event in receiver {
                match &event {
                    Event::Request { operation, .. } => *calls.entry(*operation).or_default() += 1,
                    Event::Waited { elapsed, .. } => waited += *elapsed,
                    _ => {}
                }
                self.progress.emit(event);
            }
            self.progress.emit(Event::Statistics {
                source: Source::GitHub,
                calls,
                waited,
            });
            task.join()
                .map_err(|_| "Collecte GitHub interrompue")?
                .map_err(Into::into)
        })
    }

    pub fn authenticated_user(&self) -> Result<String, Box<dyn Error>> {
        self.run(|collector| collector.authenticated_user())
    }

    pub fn activity(&self, user: &str, period: Period) -> Result<Activity, Box<dyn Error>> {
        self.run(|collector| collector.activity(user, period))
    }
}

struct Collector {
    api: Api,
}

impl Collector {
    fn authenticated_user(&self) -> Result<String, Box<dyn Error>> {
        self.api.emit(Event::Identifying(Source::GitHub));
        let (user, _): (api::User, _) = self.api.get("user", &[])?;
        Ok(user.login)
    }

    pub fn activity(&self, user: &str, period: Period) -> Result<Activity, Box<dyn Error>> {
        self.api.emit(Event::Source(Source::GitHub));
        if user.is_empty()
            || user.len() > 39
            || !user.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err("Utilisateur GitHub invalide".into());
        }
        let mut activity = Activity::empty(Source::GitHub, user, period);
        let mut candidates = BTreeMap::new();
        let queries = [
            format!("is:pr author:{user}"),
            format!("is:pr reviewed-by:{user}"),
            format!("is:pr commenter:{user}"),
            format!("is:issue commenter:{user}"),
        ]
        .map(|query| {
            format!(
                "{query} updated:>={} created:<={}",
                activity.period.from, activity.period.to
            )
        });
        let searches = parallel::collect(queries.into(), |query| {
            self.api.search::<Issue>("search/issues", &query)
        })?;
        for issues in searches {
            let mut query_keys = BTreeSet::new();
            for issue in issues {
                let repository = issue.repository()?;
                if !query_keys.insert((repository.clone(), issue.number)) {
                    return Err(
                        "Recherche GitHub avec résultats dupliqués ; relancez la collecte".into(),
                    );
                }
                candidates.insert((repository, issue.number), issue);
            }
        }
        self.api.emit(Event::Candidates {
            source: Source::GitHub,
            kind: "PRs et issues",
            count: candidates.len(),
        });
        let snapshots = graphql::snapshots(&self.api, &candidates)?;
        activity.warnings.extend(snapshots.warnings);
        let mut snapshots = snapshots.items;
        let discussions = candidates
            .iter()
            .filter(|(_, issue)| issue.pull_request.is_none() && issue.comments > 0)
            .collect::<Vec<_>>();
        let discussions = parallel::collect(discussions, |(key, issue)| {
            let mut warnings = Vec::new();
            let result = (|| {
                let slot = self
                    .api
                    .cache_key(&format!("{}/{}/discussion", key.0, key.1));
                let version = format!("{slot}/{}", issue.updated_at);
                let comments = match self
                    .api
                    .cache
                    .as_ref()
                    .and_then(|cache| cache.read::<Vec<CommentDetail>>(&slot, &version))
                {
                    Some(comments) => comments,
                    None => {
                        let comments: Vec<CommentDetail> = self
                            .api
                            .list(&format!("repos/{}/issues/{}/comments", key.0, key.1))?;
                        if let Some(cache) = &self.api.cache {
                            cache.write(&slot, &version, &comments);
                        }
                        comments
                    }
                };
                Ok(comments)
            })();
            let comments = recovery::item(
                result,
                &key.0,
                WarningSubject::Issue { number: key.1 },
                &mut warnings,
            )?;
            Ok((key.clone(), comments, warnings))
        })?;
        let mut discussion_items = BTreeMap::new();
        for (key, comments, warnings) in discussions {
            activity.warnings.extend(warnings);
            if let Some(comments) = comments {
                discussion_items.insert(key, comments);
            }
        }
        let mut relevant = Vec::new();
        let total = candidates
            .keys()
            .map(|(repository, _)| repository)
            .collect::<BTreeSet<_>>()
            .len();
        let mut visited = BTreeSet::new();
        self.api.emit(Event::Repositories {
            source: Source::GitHub,
            stage: "PRs et commentaires",
            current: 0,
            total,
        });
        for ((repository, number), issue) in &candidates {
            let repository = repository.clone();
            let number = *number;
            if visited.insert(repository.clone()) {
                self.api.emit(Event::Repositories {
                    source: Source::GitHub,
                    stage: "PRs et commentaires",
                    current: visited.len(),
                    total,
                });
            }
            if issue.pull_request.is_none() {
                if let Some(comments) = discussion_items.remove(&(repository.clone(), number)) {
                    self.record_comments(
                        &mut activity,
                        &repository,
                        number,
                        CommentKind::Discussion,
                        comments,
                    );
                }
                continue;
            }
            let Some(snapshot) = snapshots.remove(&(repository.clone(), number)) else {
                continue;
            };
            self.record_comments(
                &mut activity,
                &repository,
                number,
                CommentKind::Discussion,
                snapshot.discussion,
            );
            self.record_comments(
                &mut activity,
                &repository,
                number,
                CommentKind::Review,
                snapshot.inline,
            );
            let detail = snapshot.detail;
            let author = detail.user.as_ref().map(|u| u.login.clone());
            let owned = author
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(user));
            let mut first_review_received_at = None;
            let mut reviewed_in_period = false;
            let mut received_in_period = false;
            for review in snapshot.reviews {
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
                relevant.push((repository, number, detail, author, first_review_received_at));
            }
        }
        let keys = relevant
            .iter()
            .map(|(repository, number, ..)| (repository.clone(), *number))
            .collect::<Vec<_>>();
        let graph_files = graphql::files(&self.api, &keys, &candidates)?;
        let unavailable = graph_files
            .warnings
            .iter()
            .filter_map(|warning| match warning.subject {
                WarningSubject::PullRequest { number } => {
                    Some((warning.repository.clone(), number))
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        activity.warnings.extend(graph_files.warnings);
        relevant.retain(|(repository, number, ..)| {
            !unavailable.contains(&(repository.clone(), *number))
        });
        let pulls = parallel::collect(
            relevant,
            |(repository, number, detail, author, first_review_received_at)| {
                let warning_repository = repository.clone();
                let mut warnings = Vec::new();
                let result = (|| {
                    let mut collected =
                        Activity::empty(Source::GitHub, user, activity.period.clone());
                    let endpoint = format!("repos/{repository}/pulls/{number}");
                    let subject = WarningSubject::PullRequest { number };
                    if detail.changed_files > 3000 {
                        collected.warnings.push(warning(
                            &repository,
                            &subject,
                            WarningCode::FileLimitReached,
                            format!(
                                "{} fichiers annoncés ; GitHub limite le détail à 3000 fichiers",
                                detail.changed_files
                            ),
                        ));
                    }
                    let mut files = graph_files
                        .items
                        .get(&(repository.clone(), number))
                        .cloned();
                    if files.is_none() {
                        let warnings_before = collected.warnings.len();
                        let mut pages = Vec::new();
                        for page_number in 1..=30 {
                            let (page, next): (Vec<FileDetail>, _) = self.api.get(
                                &format!("{endpoint}/files"),
                                &[
                                    ("per_page", "100".into()),
                                    ("page", page_number.to_string()),
                                ],
                            )?;
                            let empty = page.is_empty();
                            pages.extend(page);
                            if !next {
                                break;
                            }
                            if empty || pages.len() >= 3000 || page_number == 30 {
                                collected.warnings.push(warning(
                                &repository,
                                &subject,
                                WarningCode::FilePaginationIncomplete,
                                "Pagination des fichiers de PR interrompue avant la dernière page",
                            ));
                                break;
                            }
                        }
                        if collected.warnings.len() == warnings_before
                            && detail.changed_files == pages.len()
                        {
                            let key = (repository.clone(), number);
                            graphql::cache_files(&self.api, &key, &candidates[&key], &pages);
                        }
                        files = Some(pages);
                    }
                    let files = file_changes(
                        &mut collected,
                        &repository,
                        &subject,
                        files.unwrap(),
                        detail.additions,
                        detail.deletions,
                    );
                    if detail.changed_files != files.len() {
                        collected.warnings.push(warning(&repository, &subject, WarningCode::FileCountMismatch,
                        format!("{} fichiers annoncés, {} fichiers disponibles ; exclusions limitées aux fichiers disponibles", detail.changed_files, files.len())));
                    }
                    collected.pull_requests.push(PullRequest {
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
                    Ok(collected)
                })();
                let collected = recovery::item(
                    result,
                    &warning_repository,
                    WarningSubject::PullRequest { number },
                    &mut warnings,
                )?;
                Ok((collected, warnings))
            },
        )?;
        for (collected, warnings) in pulls {
            activity.warnings.extend(warnings);
            if let Some(collected) = collected {
                activity.pull_requests.extend(collected.pull_requests);
                activity.warnings.extend(collected.warnings);
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
        self.api.emit(Event::Candidates {
            source: Source::GitHub,
            kind: "commits",
            count: commit_keys.len(),
        });
        let total = commit_keys
            .iter()
            .map(|(repository, _)| repository)
            .collect::<BTreeSet<_>>()
            .len();
        self.api.emit(Event::Repositories {
            source: Source::GitHub,
            stage: "commits",
            current: 0,
            total,
        });
        for (repository, sha) in &commit_keys {
            api::validate_repository(repository)?;
            if sha.is_empty() || !sha.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err("SHA GitHub invalide".into());
            }
        }
        let commits = parallel::collect(commit_keys.into_iter().collect(), |(repository, sha)| {
            let mut warnings = Vec::new();
            let result = self.commit(user, &activity.period, repository.clone(), sha.clone());
            let collected = recovery::item(
                result,
                &repository,
                WarningSubject::Commit { sha },
                &mut warnings,
            )?;
            Ok((collected, warnings))
        })?;
        self.api.emit(Event::Repositories {
            source: Source::GitHub,
            stage: "commits",
            current: total,
            total,
        });
        for (collected, warnings) in commits {
            activity.warnings.extend(warnings);
            if let Some(collected) = collected {
                activity.commits.extend(collected.commits);
                activity.warnings.extend(collected.warnings);
            }
        }
        let unavailable = activity
            .warnings
            .iter()
            .filter(|warning| warning.code == WarningCode::ItemUnavailable)
            .filter_map(|warning| match warning.subject {
                WarningSubject::PullRequest { number } => {
                    Some((warning.repository.clone(), number))
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        activity.comments.retain(|comment| {
            !unavailable.contains(&(comment.repository.clone(), comment.issue_number))
        });
        activity.reviews_given.retain(|review| {
            !unavailable.contains(&(review.repository.clone(), review.pull_request_number))
        });
        activity.reviews_received.retain(|review| {
            !unavailable.contains(&(review.repository.clone(), review.pull_request_number))
        });
        Ok(activity)
    }

    fn commit(
        &self,
        user: &str,
        period: &Period,
        repository: String,
        sha: String,
    ) -> Result<Activity, Box<dyn Error>> {
        let mut activity = Activity::empty(Source::GitHub, user, period.clone());
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
        Ok(activity)
    }

    fn record_comments(
        &self,
        activity: &mut Activity,
        repository: &str,
        number: u64,
        kind: CommentKind,
        comments: Vec<CommentDetail>,
    ) {
        for comment in comments {
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
