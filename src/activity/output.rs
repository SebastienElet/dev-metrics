use super::{Activity, CommentKind, Source};
use chrono::{DateTime, SecondsFormat, Utc};

impl Activity {
    pub fn table(&self) -> String {
        let mut rows = vec![
            format!(
                "{} · {} · {} → {} (UTC)",
                self.source.label(),
                self.user,
                self.period.from,
                self.period.to
            ),
            "Source | Dépôt | Activité | Référence | Date UTC | Ajouts | Suppressions".into(),
        ];
        for pr in &self.pull_requests {
            rows.push(row(
                self.source,
                &pr.repository,
                "PR",
                &format!("#{}", pr.number),
                pr.created_at,
                Some((pr.additions, pr.deletions)),
            ));
            if let Some(merged) = pr.merged_at {
                rows.push(row(
                    self.source,
                    &pr.repository,
                    "merge",
                    &format!("#{}", pr.number),
                    merged,
                    None,
                ));
            }
            if let Some(review) = pr.first_review_received_at {
                rows.push(row(
                    self.source,
                    &pr.repository,
                    "première review",
                    &format!("#{}", pr.number),
                    review,
                    None,
                ));
            }
        }
        for (reviews, label) in [
            (&self.reviews_given, "review faite"),
            (&self.reviews_received, "review reçue"),
        ] {
            for review in reviews {
                rows.push(row(
                    self.source,
                    &review.repository,
                    label,
                    &format!("#{}/review:{}", review.pull_request_number, review.id),
                    review.submitted_at,
                    None,
                ));
            }
        }
        for comment in &self.comments {
            let label = match comment.kind {
                CommentKind::Discussion => "commentaire",
                CommentKind::Review => "commentaire review",
            };
            rows.push(row(
                self.source,
                &comment.repository,
                label,
                &format!("#{}/comment:{}", comment.issue_number, comment.id),
                comment.created_at,
                None,
            ));
        }
        for commit in &self.commits {
            rows.push(row(
                self.source,
                &commit.repository,
                "commit",
                &commit.sha,
                commit.authored_at,
                Some((commit.additions, commit.deletions)),
            ));
        }
        if rows.len() == 2 {
            rows.push("Aucune activité.".into());
        }
        rows.join("\n")
    }
}

fn row(
    source: Source,
    repository: &str,
    activity: &str,
    reference: &str,
    date: DateTime<Utc>,
    statistics: Option<(u64, u64)>,
) -> String {
    let (added, deleted) = statistics.map_or(("—".into(), "—".into()), |(a, d)| {
        (a.to_string(), d.to_string())
    });
    format!(
        "{} | {repository} | {activity} | {reference} | {} | {added} | {deleted}",
        source.as_str(),
        date.to_rfc3339_opts(SecondsFormat::AutoSi, true)
    )
}
