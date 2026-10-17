use super::{Comparison, Metrics, MetricsReport, WindowMetrics};

pub const REPORT_SCHEMA: &str = include_str!("../../schemas/report-v1.schema.json");
const LEGEND: &str =
    "Δ = courant − précédent ; — = non calculable. Part > 400 lignes : Δ en points.";

impl MetricsReport {
    pub fn json(&self) -> Result<String, serde_json::Error> {
        #[derive(serde::Serialize)]
        struct Report<'a> {
            schema_version: u32,
            #[serde(flatten)]
            metrics: &'a MetricsReport,
        }
        serde_json::to_string(&Report {
            schema_version: 1,
            metrics: self,
        })
    }

    pub fn table(&self) -> String {
        let mut sections = vec![table_section("Ensemble", &self.windows)];
        sections.extend(self.by_source.iter().map(|source| {
            table_section(
                &format!("Source : {}", source.source.as_str()),
                &source.windows,
            )
        }));
        sections.extend(self.by_repository.iter().map(|repository| {
            table_section(
                &format!(
                    "Dépôt : {}/{}",
                    repository.source.as_str(),
                    repository.repository
                ),
                &repository.windows,
            )
        }));
        sections.push(LEGEND.into());
        sections.join("\n\n")
    }

    pub fn markdown(&self) -> String {
        let mut lines = vec!["## Métriques de développement".into(), String::new()];
        lines.extend(
            self.windows
                .iter()
                .map(|window| format!("- {}", period_label(window))),
        );
        lines.push(String::new());
        lines.push(format!(
            "| Métrique | {} |",
            self.windows
                .iter()
                .map(|window| format!("{} j (Δ ; Δ %)", window.days))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
        lines.push(format!(
            "| --- | {} |",
            vec!["---:"; self.windows.len()].join(" | ")
        ));
        for row in metric_rows(&self.windows) {
            lines.push(format!("| {} |", row.join(" | ")));
        }
        let sources = self
            .by_source
            .iter()
            .map(|source| source.source.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(String::new());
        lines.push(format!(
            "Sources : {}. Dépôts : {}.",
            if sources.is_empty() {
                "aucune"
            } else {
                &sources
            },
            self.by_repository.len()
        ));
        lines.push(LEGEND.into());
        lines.join("\n")
    }
}

fn period_label(window: &WindowMetrics) -> String {
    format!(
        "{} j : {} → {} ; précédent : {} → {} (UTC)",
        window.days,
        window.current_period.from,
        window.current_period.to,
        window.previous_period.from,
        window.previous_period.to
    )
}

fn measures(metrics: &Metrics) -> [(&str, &Comparison, f64); 12] {
    [
        ("PRs ouvertes", &metrics.pull_requests_opened, 1.0),
        ("PRs mergées", &metrics.pull_requests_merged, 1.0),
        ("Commits", &metrics.commits, 1.0),
        ("Lignes ajoutées", &metrics.lines_added, 1.0),
        ("Lignes supprimées", &metrics.lines_deleted, 1.0),
        ("Médiane merge (h)", &metrics.median_merge_seconds, 3600.0),
        (
            "Médiane première review (h)",
            &metrics.median_first_review_seconds,
            3600.0,
        ),
        (
            "Médiane taille PR (lignes)",
            &metrics.median_pull_request_lines,
            1.0,
        ),
        (
            "PRs > 400 lignes (%)",
            &metrics.large_pull_request_percent,
            1.0,
        ),
        ("Reviews faites", &metrics.reviews_given, 1.0),
        ("Commentaires laissés", &metrics.comments_left, 1.0),
        ("Reviews reçues", &metrics.reviews_received, 1.0),
    ]
}

fn number(value: Option<f64>) -> String {
    value.map_or_else(
        || "—".into(),
        |value| {
            let rounded = if value.abs() < 0.005 { 0.0 } else { value };
            format!("{rounded:.2}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .into()
        },
    )
}

fn delta(value: Option<f64>) -> String {
    let text = number(value);
    if value.is_some_and(|value| value >= 0.005) {
        format!("+{text}")
    } else {
        text
    }
}

fn cells(comparison: &Comparison, divisor: f64) -> [String; 3] {
    [
        number(comparison.current.map(|value| value / divisor)),
        delta(comparison.delta.map(|value| value / divisor)),
        delta(comparison.delta_percent),
    ]
}

fn metric_rows(windows: &[WindowMetrics]) -> Vec<Vec<String>> {
    let Some(first) = windows.first() else {
        return Vec::new();
    };
    measures(&first.metrics)
        .iter()
        .enumerate()
        .map(|(index, (label, _, _))| {
            let mut row = vec![(*label).into()];
            row.extend(windows.iter().map(|window| {
                let (_, comparison, divisor) = measures(&window.metrics)[index];
                let [current, delta, percent] = cells(comparison, divisor);
                format!("{current} ({delta} ; {percent} %)")
            }));
            row
        })
        .collect()
}

fn table_section(title: &str, windows: &[WindowMetrics]) -> String {
    let mut lines = vec![title.into()];
    lines.extend(windows.iter().map(period_label));
    let mut header = vec!["Métrique".into()];
    for window in windows {
        header.extend([format!("{} j", window.days), "Δ".into(), "Δ %".into()]);
    }
    let mut rows = vec![header];
    if let Some(first) = windows.first() {
        for (index, (label, _, _)) in measures(&first.metrics).iter().enumerate() {
            let mut row = vec![(*label).into()];
            for window in windows {
                let (_, comparison, divisor) = measures(&window.metrics)[index];
                row.extend(cells(comparison, divisor));
            }
            rows.push(row);
        }
    }
    let widths: Vec<_> = (0..rows[0].len())
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for (index, row) in rows.iter().enumerate() {
        lines.push(
            row.iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:width$}"))
                .collect::<Vec<_>>()
                .join(" | ")
                .trim_end()
                .into(),
        );
        if index == 0 {
            lines.push(
                widths
                    .iter()
                    .map(|width| "-".repeat(*width))
                    .collect::<Vec<_>>()
                    .join("-+-"),
            );
        }
    }
    lines.join("\n")
}
