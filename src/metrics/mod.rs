use crate::activity::{Activity, ActivitySource, FileChange, Period, Source};
use chrono::{DateTime, Days, NaiveDate, Utc};
use std::collections::BTreeSet;
use std::error::Error;

#[derive(Debug, PartialEq)]
pub struct Comparison {
    pub current: Option<f64>,
    pub previous: Option<f64>,
    pub delta: Option<f64>,
    pub delta_percent: Option<f64>,
}

impl Comparison {
    fn new(current: Option<f64>, previous: Option<f64>) -> Self {
        let delta = current
            .zip(previous)
            .map(|(current, previous)| current - previous);
        let delta_percent = current.zip(previous).and_then(|(current, previous)| {
            if previous != 0.0 {
                Some((current - previous) / previous * 100.0)
            } else if current == 0.0 {
                Some(0.0)
            } else {
                None
            }
        });
        Self {
            current,
            previous,
            delta,
            delta_percent,
        }
    }
}

#[derive(Debug)]
pub struct Metrics {
    pub pull_requests_opened: Comparison,
    pub pull_requests_merged: Comparison,
    pub commits: Comparison,
    pub lines_added: Comparison,
    pub lines_deleted: Comparison,
    pub median_merge_seconds: Comparison,
    pub median_first_review_seconds: Comparison,
    pub median_pull_request_lines: Comparison,
    pub large_pull_request_percent: Comparison,
    pub reviews_given: Comparison,
    pub comments_left: Comparison,
    pub reviews_received: Comparison,
}

#[derive(Debug)]
pub struct WindowMetrics {
    pub days: u64,
    pub current_period: Period,
    pub previous_period: Period,
    pub metrics: Metrics,
}

#[derive(Debug)]
pub struct SourceMetrics {
    pub source: Source,
    pub windows: [WindowMetrics; 2],
}

#[derive(Debug)]
pub struct RepositoryMetrics {
    pub source: Source,
    pub repository: String,
    pub windows: [WindowMetrics; 2],
}

#[derive(Debug)]
pub struct MetricsReport {
    pub windows: [WindowMetrics; 2],
    pub by_source: Vec<SourceMetrics>,
    pub by_repository: Vec<RepositoryMetrics>,
}

pub struct SourceRequest<'a> {
    pub source: &'a dyn ActivitySource,
    pub user: Option<&'a str>,
}

pub fn collect(
    sources: &[SourceRequest<'_>],
    through: NaiveDate,
    config: &MetricsConfig,
) -> Result<MetricsReport, Box<dyn Error>> {
    let from = through
        .checked_sub_days(Days::new(59))
        .ok_or("Date hors limites")?;
    let period = Period { from, to: through };
    let activities = sources
        .iter()
        .map(|request| {
            request
                .source
                .collect_activity(request.user, period.clone())
        })
        .collect::<Result<Vec<_>, _>>()?;
    calculate(&activities, through, config)
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsConfig {
    pub excluded_files: Vec<String>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            excluded_files: [
                "Cargo.lock",
                "package-lock.json",
                "npm-shrinkwrap.json",
                "yarn.lock",
                "pnpm-lock.yaml",
                "bun.lock",
                "bun.lockb",
                "composer.lock",
                "Pipfile.lock",
                "poetry.lock",
                "uv.lock",
                "Gemfile.lock",
                "go.sum",
                "packages.lock.json",
                "*.min.js",
                "*.min.css",
                "*.generated.*",
                "generated/*",
                "*/generated/*",
                "dist/*",
                "*/dist/*",
                "build/*",
                "*/build/*",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}

impl MetricsConfig {
    fn excludes(&self, file: &FileChange) -> bool {
        let matches = |path: &str| {
            self.excluded_files.iter().any(|pattern| {
                let candidate = if pattern.contains('/') {
                    path
                } else {
                    path.rsplit('/').next().unwrap_or(path)
                };
                wildcard_matches(pattern, candidate)
            })
        };
        matches(&file.path) || file.previous_path.as_deref().is_some_and(matches)
    }
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let value: Vec<_> = value.chars().collect();
    let mut matched = vec![false; value.len() + 1];
    matched[0] = true;
    for character in pattern.chars() {
        if character == '*' {
            for index in 1..matched.len() {
                matched[index] |= matched[index - 1];
            }
        } else {
            for index in (1..matched.len()).rev() {
                matched[index] =
                    matched[index - 1] && (character == '?' || character == value[index - 1]);
            }
            matched[0] = false;
        }
    }
    matched[value.len()]
}

pub fn calculate(
    activities: &[Activity],
    through: NaiveDate,
    config: &MetricsConfig,
) -> Result<MetricsReport, Box<dyn Error>> {
    let from = through
        .checked_sub_days(Days::new(59))
        .ok_or("Date hors limites")?;
    let mut sources = BTreeSet::new();
    for activity in activities {
        if activity.period.from > from || activity.period.to < through {
            return Err("La collecte doit couvrir les 60 jours comparés".into());
        }
        if !sources.insert(activity.source.as_str()) {
            return Err("Une seule collecte par source est acceptée".into());
        }
    }
    let mut by_source = Vec::new();
    let mut by_repository = Vec::new();
    for source in [Source::GitHub, Source::Bitbucket] {
        let source_activities: Vec<_> = activities
            .iter()
            .filter(|activity| activity.source == source)
            .collect();
        if source_activities.is_empty() {
            continue;
        }
        by_source.push(SourceMetrics {
            source,
            windows: windows(activities, through, config, Some(source), None)?,
        });
        let repositories: BTreeSet<_> = source_activities
            .iter()
            .flat_map(|activity| {
                activity
                    .pull_requests
                    .iter()
                    .map(|pr| pr.repository.as_str())
                    .chain(
                        activity
                            .commits
                            .iter()
                            .map(|commit| commit.repository.as_str()),
                    )
                    .chain(
                        activity
                            .reviews_given
                            .iter()
                            .map(|review| review.repository.as_str()),
                    )
                    .chain(
                        activity
                            .reviews_received
                            .iter()
                            .map(|review| review.repository.as_str()),
                    )
                    .chain(
                        activity
                            .comments
                            .iter()
                            .map(|comment| comment.repository.as_str()),
                    )
            })
            .collect();
        for repository in repositories {
            by_repository.push(RepositoryMetrics {
                source,
                repository: repository.into(),
                windows: windows(activities, through, config, Some(source), Some(repository))?,
            });
        }
    }
    Ok(MetricsReport {
        windows: windows(activities, through, config, None, None)?,
        by_source,
        by_repository,
    })
}

fn windows(
    activities: &[Activity],
    through: NaiveDate,
    config: &MetricsConfig,
    source: Option<Source>,
    repository: Option<&str>,
) -> Result<[WindowMetrics; 2], Box<dyn Error>> {
    let window = |days| -> Result<WindowMetrics, Box<dyn Error>> {
        let current_start = through
            .checked_sub_days(Days::new(days - 1))
            .ok_or("Date hors limites")?;
        let previous_end = current_start.pred_opt().ok_or("Date hors limites")?;
        let previous_start = previous_end
            .checked_sub_days(Days::new(days - 1))
            .ok_or("Date hors limites")?;
        let current_period = Period {
            from: current_start,
            to: through,
        };
        let previous_period = Period {
            from: previous_start,
            to: previous_end,
        };
        let current = snapshot(activities, &current_period, config, source, repository)?;
        let previous = snapshot(activities, &previous_period, config, source, repository)?;
        Ok(WindowMetrics {
            days,
            current_period,
            previous_period,
            metrics: compare(current, previous),
        })
    };
    Ok([window(7)?, window(30)?])
}

#[derive(Default)]
struct Snapshot {
    opened: u64,
    merged: u64,
    commits: u64,
    additions: u64,
    deletions: u64,
    merge_seconds: Vec<f64>,
    first_review_seconds: Vec<f64>,
    pull_request_lines: Vec<f64>,
    large_pull_requests: u64,
    reviews_given: u64,
    reviews_received: u64,
    comments: u64,
}

fn snapshot(
    activities: &[Activity],
    period: &Period,
    config: &MetricsConfig,
    source: Option<Source>,
    repository: Option<&str>,
) -> Result<Snapshot, Box<dyn Error>> {
    let mut result = Snapshot::default();
    let includes = |value: &str| repository.is_none_or(|repository| repository == value);
    for activity in activities
        .iter()
        .filter(|activity| source.is_none_or(|source| source == activity.source))
    {
        result.reviews_given += activity
            .reviews_given
            .iter()
            .filter(|review| includes(&review.repository) && period.contains(review.submitted_at))
            .count() as u64;
        result.reviews_received += activity
            .reviews_received
            .iter()
            .filter(|review| includes(&review.repository) && period.contains(review.submitted_at))
            .count() as u64;
        result.comments += activity
            .comments
            .iter()
            .filter(|comment| includes(&comment.repository) && period.contains(comment.created_at))
            .count() as u64;
        for pr in &activity.pull_requests {
            if !includes(&pr.repository)
                || !pr
                    .author
                    .as_deref()
                    .is_some_and(|author| same_user(activity.source, author, &activity.user))
            {
                continue;
            }
            if period.contains(pr.created_at) {
                result.opened += 1;
                let lines = pr
                    .files
                    .iter()
                    .filter(|file| !config.excludes(file))
                    .try_fold(0_u64, |total, file| {
                        total
                            .checked_add(file.additions)?
                            .checked_add(file.deletions)
                    })
                    .ok_or("Statistiques trop grandes")?;
                result.pull_request_lines.push(lines as f64);
                result.large_pull_requests += u64::from(lines > 400);
            }
            if let Some(at) = pr.merged_at.filter(|at| period.contains(*at)) {
                result.merged += 1;
                result
                    .merge_seconds
                    .push(elapsed_seconds(pr.created_at, at)?);
            }
            if let Some(at) = pr
                .first_review_received_at
                .filter(|at| period.contains(*at))
            {
                result
                    .first_review_seconds
                    .push(elapsed_seconds(pr.created_at, at)?);
            }
        }
        for commit in &activity.commits {
            if !includes(&commit.repository) || !period.contains(commit.authored_at) {
                continue;
            }
            result.commits += 1;
            for file in commit.files.iter().filter(|file| !config.excludes(file)) {
                result.additions = result
                    .additions
                    .checked_add(file.additions)
                    .ok_or("Statistiques trop grandes")?;
                result.deletions = result
                    .deletions
                    .checked_add(file.deletions)
                    .ok_or("Statistiques trop grandes")?;
            }
        }
    }
    Ok(result)
}

fn same_user(source: Source, left: &str, right: &str) -> bool {
    match source {
        Source::GitHub => left.eq_ignore_ascii_case(right),
        Source::Bitbucket => left == right,
    }
}

fn compare(current: Snapshot, previous: Snapshot) -> Metrics {
    let count = |current, previous| Comparison::new(Some(current as f64), Some(previous as f64));
    let large_percent = |snapshot: &Snapshot| {
        (snapshot.opened != 0)
            .then(|| snapshot.large_pull_requests as f64 / snapshot.opened as f64 * 100.0)
    };
    let large = Comparison::new(large_percent(&current), large_percent(&previous));
    Metrics {
        pull_requests_opened: count(current.opened, previous.opened),
        pull_requests_merged: count(current.merged, previous.merged),
        commits: count(current.commits, previous.commits),
        lines_added: count(current.additions, previous.additions),
        lines_deleted: count(current.deletions, previous.deletions),
        median_merge_seconds: Comparison::new(
            median(current.merge_seconds),
            median(previous.merge_seconds),
        ),
        median_first_review_seconds: Comparison::new(
            median(current.first_review_seconds),
            median(previous.first_review_seconds),
        ),
        median_pull_request_lines: Comparison::new(
            median(current.pull_request_lines),
            median(previous.pull_request_lines),
        ),
        large_pull_request_percent: large,
        reviews_given: count(current.reviews_given, previous.reviews_given),
        comments_left: count(current.comments, previous.comments),
        reviews_received: count(current.reviews_received, previous.reviews_received),
    }
}

fn elapsed_seconds(from: DateTime<Utc>, to: DateTime<Utc>) -> Result<f64, Box<dyn Error>> {
    let elapsed = to.signed_duration_since(from);
    if elapsed < chrono::Duration::zero() {
        return Err("Timestamp de merge ou de review antérieur à l'ouverture de la PR".into());
    }
    Ok(elapsed.num_milliseconds() as f64 / 1000.0)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        Some(values[middle - 1] / 2.0 + values[middle] / 2.0)
    } else {
        Some(values[middle])
    }
}

#[cfg(test)]
mod tests;
