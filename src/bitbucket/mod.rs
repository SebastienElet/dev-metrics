mod api;
mod auth;
mod model;
pub use auth::{resolve_token, resolve_workspace};

use crate::activity::{
    Activity, ActivitySource, Comment, CommentKind, Commit, FileChange, Period, PullRequest,
    Review, ReviewState, Source,
};
use chrono::{DateTime, Utc};
use model::{
    CommentDetail, CommitDetail, DiffStat, Event, PullRequestDetail, PullRequestState, Repository,
    User,
};
use reqwest::Url;
use std::collections::BTreeSet;
use std::error::Error;

pub struct BitbucketSource {
    api: api::Api,
    workspace: String,
}

impl ActivitySource for BitbucketSource {
    fn collect_activity(
        &self,
        user: Option<&str>,
        period: Period,
    ) -> Result<Activity, Box<dyn Error>> {
        self.activity(user, period)
    }
}

impl BitbucketSource {
    pub fn new(api_base: Url, token: &str, workspace: &str) -> Result<Self, Box<dyn Error>> {
        validate_workspace(workspace)?;
        Ok(Self {
            api: api::Api::new(api_base, token)?,
            workspace: workspace.into(),
        })
    }

    pub fn new_with_email(
        api_base: Url,
        token: &str,
        email: &str,
        workspace: &str,
    ) -> Result<Self, Box<dyn Error>> {
        validate_workspace(workspace)?;
        Ok(Self {
            api: api::Api::new_with_email(api_base, token, email)?,
            workspace: workspace.into(),
        })
    }

    pub fn activity(&self, user: Option<&str>, period: Period) -> Result<Activity, Box<dyn Error>> {
        if user.is_some_and(|value| !valid_user_identifier(value)) {
            return Err("Utilisateur Bitbucket invalide".into());
        }
        let endpoint = user.map_or_else(|| "user".into(), |user| format!("users/{user}"));
        let identity: User = self.api.get(&endpoint)?;
        if identity.uuid.is_empty() {
            return Err("Identité Bitbucket sans UUID".into());
        }
        let mut activity = Activity::empty(Source::Bitbucket, &identity.uuid, period);
        let mut repositories = BTreeSet::new();
        for repository in self
            .api
            .list::<Repository>(&format!("repositories/{}", self.workspace), &[])?
        {
            let Some((workspace, slug)) = repository.full_name.split_once('/') else {
                return Err("Dépôt Bitbucket invalide".into());
            };
            if workspace != self.workspace
                || slug.is_empty()
                || matches!(slug, "." | "..")
                || !slug
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
            {
                return Err("Dépôt Bitbucket invalide ou hors workspace".into());
            }
            if !repositories.insert(repository.full_name.clone()) {
                return Err("Dépôt Bitbucket dupliqué".into());
            }
            let endpoint = format!("repositories/{}", repository.full_name);
            let mut pull_requests = BTreeSet::new();
            for detail in self.api.list::<PullRequestDetail>(
                &format!("{endpoint}/pullrequests"),
                &[
                    ("state", "OPEN"),
                    ("state", "MERGED"),
                    ("state", "DECLINED"),
                    ("state", "SUPERSEDED"),
                ],
            )? {
                if !pull_requests.insert(detail.id) {
                    return Err("PR Bitbucket dupliquée".into());
                }
                self.pull_request(&mut activity, &repository.full_name, detail)?;
            }
            let mut commits = BTreeSet::new();
            for commit in self
                .api
                .list::<CommitDetail>(&format!("{endpoint}/commits"), &[])?
            {
                if commit.hash.is_empty() || !commit.hash.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return Err("Hash Bitbucket invalide".into());
                }
                if !commits.insert(commit.hash.clone()) {
                    return Err("Commit Bitbucket dupliqué".into());
                }
                if commit.author.user.as_ref().map(|u| u.uuid.as_str())
                    != Some(activity.user.as_str())
                    || !activity.period.contains(commit.date)
                {
                    continue;
                }
                let (files, additions, deletions) =
                    self.files(&format!("{endpoint}/diffstat/{}", commit.hash))?;
                activity.commits.push(Commit {
                    source: Source::Bitbucket,
                    repository: repository.full_name.clone(),
                    sha: commit.hash,
                    message: commit.message,
                    url: commit.links.html.href,
                    authored_at: commit.date,
                    additions,
                    deletions,
                    files,
                });
            }
        }
        Ok(activity)
    }

    fn pull_request(
        &self,
        activity: &mut Activity,
        repository: &str,
        detail: PullRequestDetail,
    ) -> Result<(), Box<dyn Error>> {
        let endpoint = format!("repositories/{repository}/pullrequests/{}", detail.id);
        let author = detail.author.map(|a| a.uuid);
        let owned = author.as_deref() == Some(activity.user.as_str());
        let mut merged_at = None;
        let mut reviews = Vec::new();
        for event in self
            .api
            .list::<Event>(&format!("{endpoint}/activity"), &[])?
        {
            let kinds = [
                event.approval.is_some(),
                event.changes_requested.is_some(),
                event.update.is_some(),
                event.comment.is_some(),
            ];
            if kinds.into_iter().filter(|known| *known).count() != 1 {
                return Err("Événement Bitbucket inconnu ou ambigu".into());
            }
            if let Some(update) = event.update
                && matches!(update.state, PullRequestState::Merged)
            {
                merged_at = Some(merged_at.map_or(update.date, |previous: DateTime<Utc>| {
                    previous.min(update.date)
                }));
            }
            for (action, state) in [
                (event.approval, ReviewState::Approved),
                (event.changes_requested, ReviewState::ChangesRequested),
            ] {
                if let Some(action) = action {
                    reviews.push((
                        action.date,
                        action.user.map(|u| u.uuid),
                        state,
                        detail.links.html.href.clone(),
                    ));
                }
            }
        }
        if matches!(detail.state, PullRequestState::Merged) && merged_at.is_none() {
            return Err("PR Bitbucket mergée sans timestamp de merge dans son historique".into());
        }
        let mut commented_in_period = false;
        let mut comments = BTreeSet::new();
        for comment in self
            .api
            .list::<CommentDetail>(&format!("{endpoint}/comments"), &[])?
        {
            if !comments.insert(comment.id) {
                return Err("Commentaire Bitbucket dupliqué".into());
            }
            if comment.deleted || comment.pending {
                continue;
            }
            let commenter = comment.user.map(|u| u.uuid);
            reviews.push((
                comment.created_on,
                commenter.clone(),
                ReviewState::Commented,
                comment.links.html.href.clone(),
            ));
            if commenter.as_deref() == Some(activity.user.as_str())
                && activity.period.contains(comment.created_on)
            {
                commented_in_period = true;
                activity.comments.push(Comment {
                    source: Source::Bitbucket,
                    repository: repository.into(),
                    issue_number: detail.id,
                    id: comment.id,
                    author: activity.user.clone(),
                    created_at: comment.created_on,
                    url: comment.links.html.href,
                    kind: if comment.inline.is_some() {
                        CommentKind::Review
                    } else {
                        CommentKind::Discussion
                    },
                });
            }
        }
        reviews.sort_by(|a, b| (a.0, &a.1, a.2 as u8, &a.3).cmp(&(b.0, &b.1, b.2 as u8, &b.3)));
        let mut review_keys = BTreeSet::new();
        for review in &reviews {
            if !review_keys.insert((review.0, &review.1, review.2 as u8, &review.3)) {
                return Err("Review Bitbucket dupliquée".into());
            }
        }
        let mut first_review_received_at = None;
        let mut reviewed_in_period = false;
        for (index, (submitted_at, reviewer, state, url)) in reviews.into_iter().enumerate() {
            if author
                .as_deref()
                .zip(reviewer.as_deref())
                .is_some_and(|(a, r)| a == r)
            {
                continue;
            }
            first_review_received_at = Some(
                first_review_received_at.map_or(submitted_at, |previous: DateTime<Utc>| {
                    previous.min(submitted_at)
                }),
            );
            if !activity.period.contains(submitted_at) {
                continue;
            }
            let given = !owned && reviewer.as_deref() == Some(activity.user.as_str());
            if !given && !owned {
                continue;
            }
            reviewed_in_period = true;
            let review = Review {
                source: Source::Bitbucket,
                repository: repository.into(),
                pull_request_number: detail.id,
                id: u64::try_from(index)? + 1,
                author: reviewer,
                submitted_at,
                state,
                url,
            };
            if owned {
                activity.reviews_received.push(review);
            } else {
                activity.reviews_given.push(review);
            }
        }
        let authored_in_period = owned
            && (activity.period.contains(detail.created_on)
                || merged_at.is_some_and(|t| activity.period.contains(t)));
        if authored_in_period || reviewed_in_period || commented_in_period {
            let (files, additions, deletions) = self.files(&format!("{endpoint}/diffstat"))?;
            activity.pull_requests.push(PullRequest {
                source: Source::Bitbucket,
                repository: repository.into(),
                number: detail.id,
                author,
                title: detail.title,
                url: detail.links.html.href,
                created_at: detail.created_on,
                merged_at,
                first_review_received_at,
                additions,
                deletions,
                files,
            });
        }
        Ok(())
    }

    fn files(&self, endpoint: &str) -> Result<(Vec<FileChange>, u64, u64), Box<dyn Error>> {
        let mut paths = BTreeSet::new();
        let mut files = Vec::new();
        let (mut additions, mut deletions) = (0_u64, 0_u64);
        for entry in self.api.list::<DiffStat>(endpoint, &[])? {
            let old = entry.old.map(|f| f.path);
            let path = entry
                .new
                .map(|f| f.path)
                .or_else(|| old.clone())
                .ok_or("Diff Bitbucket sans chemin")?;
            if path.is_empty() || !paths.insert(path.clone()) {
                return Err("Chemin de diff Bitbucket vide ou dupliqué".into());
            }
            let previous_path = old.filter(|old| old != &path);
            additions = additions
                .checked_add(entry.lines_added)
                .ok_or("Statistiques Bitbucket trop grandes")?;
            deletions = deletions
                .checked_add(entry.lines_removed)
                .ok_or("Statistiques Bitbucket trop grandes")?;
            files.push(FileChange {
                path,
                previous_path,
                additions: entry.lines_added,
                deletions: entry.lines_removed,
            });
        }
        Ok((files, additions, deletions))
    }
}

pub(crate) fn valid_user_identifier(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'{' | b'}' | b':')
        })
}

fn validate_workspace(workspace: &str) -> Result<(), Box<dyn Error>> {
    if workspace.is_empty()
        || !workspace
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return Err("Workspace Bitbucket invalide".into());
    }
    Ok(())
}
