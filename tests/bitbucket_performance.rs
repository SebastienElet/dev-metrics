use chrono::{Days, NaiveDate};
use dev_metrics::activity::Period;
use dev_metrics::bitbucket::BitbucketSource;
use dev_metrics::metrics::{MetricsConfig, SourceRequest, collect_period};
use mockito::{Matcher, Mock, Server};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
struct Measurements(Arc<Mutex<BTreeMap<&'static str, (usize, Duration)>>>);

impl Measurements {
    fn endpoint(
        &self,
        server: &mut Server,
        path: &str,
        query: Matcher,
        kind: &'static str,
        body: Value,
    ) -> Mock {
        let measurements = self.clone();
        let body = body.to_string();
        server
            .mock("GET", path)
            .match_query(query)
            .with_header("content-type", "application/json")
            .with_chunked_body(move |writer| {
                let started = Instant::now();
                std::thread::sleep(Duration::from_millis(50));
                writer.write_all(body.as_bytes())?;
                let mut counts = measurements.0.lock().unwrap();
                let entry = counts.entry(kind).or_default();
                entry.0 += 1;
                entry.1 += started.elapsed();
                Ok(())
            })
            .create()
    }

    fn pages(
        &self,
        server: &mut Server,
        path: &str,
        values: &[Value],
        first_query: Matcher,
        cursor: &str,
        page_size: usize,
    ) {
        let kind = if path.ends_with("/commits") {
            "commits"
        } else {
            "pullrequests"
        };
        let pages = values.len().max(1).div_ceil(page_size);
        for page in 0..pages {
            let mut body = json!({
                "values": &values[page * page_size..values.len().min((page + 1) * page_size)],
                "size": values.len()
            });
            if page + 1 < pages {
                body["next"] = json!(format!("?cursor={cursor}-{}", page + 1));
            }
            self.endpoint(
                server,
                path,
                if page == 0 {
                    first_query.clone()
                } else {
                    Matcher::UrlEncoded("cursor".into(), format!("{cursor}-{page}"))
                },
                kind,
                body,
            );
        }
    }
}

fn cold_report(days: u64) {
    let mut server = Server::new();
    let measurements = Measurements::default();
    let through = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
    let from = through.checked_sub_days(Days::new(days - 1)).unwrap();
    let collection_from = through.checked_sub_days(Days::new(days * 2 - 1)).unwrap();
    let timestamp = |age| {
        format!(
            "{}T12:00:00Z",
            through.checked_sub_days(Days::new(age)).unwrap()
        )
    };
    measurements.endpoint(
        &mut server,
        "/2.0/user",
        Matcher::Any,
        "identity",
        json!({"uuid":"{alice}"}),
    );
    measurements.endpoint(
        &mut server,
        "/2.0/repositories/team",
        Matcher::Any,
        "repositories",
        json!({"values":(0..3).map(|n|json!({"full_name":format!("team/project{n}")})).collect::<Vec<_>>(),"size":3}),
    );
    for repository in 0..3 {
        let base = format!("/2.0/repositories/team/project{repository}");
        let prs = (0..100)
            .map(|id| {
                let date = timestamp(id * 3);
                json!({
                    "id":id+1,"author":{"uuid":if id%4==0 {"{alice}"}else{"{bob}"}},
                    "title":format!("Change {id}"),"state":"OPEN","created_on":date,"updated_on":date,
                    "links":{"html":{"href":format!("https://bitbucket.org/team/project{repository}/pull-requests/{}",id+1)}}
                })
            })
            .collect::<Vec<_>>();
        measurements.pages(
            &mut server,
            &format!("{base}/pullrequests"),
            &prs,
            Matcher::UrlEncoded("pagelen".into(), "100".into()),
            "all-prs",
            50,
        );
        let filtered = prs
            .iter()
            .filter(|pr| {
                pr["updated_on"].as_str().unwrap()
                    >= format!("{collection_from}T00:00:00Z").as_str()
            })
            .cloned()
            .collect::<Vec<_>>();
        measurements.pages(
            &mut server,
            &format!("{base}/pullrequests"),
            &filtered,
            Matcher::AllOf(vec![
                Matcher::UrlEncoded("pagelen".into(), "50".into()),
                Matcher::UrlEncoded(
                    "q".into(),
                    format!("updated_on>=\"{collection_from}T00:00:00Z\""),
                ),
            ]),
            "recent-prs",
            50,
        );
        for (id, pr) in prs.iter().enumerate() {
            let endpoint = format!("{base}/pullrequests/{}", id + 1);
            measurements.endpoint(
                &mut server,
                &format!("{endpoint}/activity"),
                Matcher::Any,
                "activity",
                json!({"values":[{"approval":{"date":pr["created_on"],"user":{"uuid":if id%4==0 {"{bob}"}else{"{alice}"}}}}]}),
            );
            measurements.endpoint(
                &mut server,
                &format!("{endpoint}/comments"),
                Matcher::Any,
                "comments",
                json!({"values":[]}),
            );
            measurements.endpoint(
                &mut server,
                &format!("{endpoint}/diffstat"),
                Matcher::Any,
                "pr_diffstat",
                json!({"values":[{"new":{"path":"src/lib.rs"},"lines_added":10,"lines_removed":2}]}),
            );
        }
        let commits = (0..500)
            .map(|id| json!({
                "hash":format!("{id:08x}"),"date":timestamp(id/5),"message":"Change",
                "author":{"user":{"uuid":if id%5==0 {"{alice}"}else{"{bob}"}}},
                "links":{"html":{"href":format!("https://bitbucket.org/team/project{repository}/commits/{id:08x}")}}
            }))
            .collect::<Vec<_>>();
        measurements.pages(
            &mut server,
            &format!("{base}/commits"),
            &commits,
            Matcher::UrlEncoded("pagelen".into(), "100".into()),
            "commits",
            100,
        );
        for id in (0..500).step_by(5) {
            measurements.endpoint(
                &mut server,
                &format!("{base}/diffstat/{id:08x}"),
                Matcher::Any,
                "commit_diffstat",
                json!({"values":[{"new":{"path":"src/lib.rs"},"lines_added":10,"lines_removed":2}]}),
            );
        }
    }
    let source = BitbucketSource::new(
        format!("{}/2.0/", server.url()).parse().unwrap(),
        "benchmark-token",
        "team",
    )
    .unwrap();
    let started = Instant::now();
    let report = collect_period(
        &[SourceRequest {
            source: &source,
            user: None,
        }],
        Period::parse(&from.to_string(), &through.to_string()).unwrap(),
        &MetricsConfig::default(),
    )
    .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(report.by_repository.len(), 3);
    assert_eq!(
        report.windows[0].metrics.lines_added.current,
        Some(30.0 * days as f64)
    );
    let counts = measurements.0.lock().unwrap();
    eprintln!(
        "Bitbucket cold report: {days} days (+ previous period), {:.3}s",
        elapsed.as_secs_f64()
    );
    for (kind, (requests, duration)) in counts.iter() {
        eprintln!(
            "{kind}: {requests} requests, {:.3}s cumulative mock latency",
            duration.as_secs_f64()
        );
    }
    let (histories, commit_diffs, seconds) = if days == 7 {
        (15, 42, 10)
    } else {
        (60, 180, 30)
    };
    for (kind, expected) in [
        ("identity", 1),
        ("repositories", 1),
        ("pullrequests", 3),
        ("activity", histories),
        ("comments", histories),
        ("pr_diffstat", histories),
        ("commits", 15),
        ("commit_diffstat", commit_diffs),
    ] {
        assert_eq!(counts[kind].0, expected, "{kind}");
    }
    assert!(elapsed < Duration::from_secs(seconds), "{elapsed:?}");
}

#[test]
fn cold_seven_day_report() {
    cold_report(7);
}

#[test]
fn cold_thirty_day_report() {
    cold_report(30);
}
