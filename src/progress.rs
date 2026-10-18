use crate::activity::Source;
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Progress {
    output: Option<Rc<RefCell<Box<dyn Write>>>>,
    verbose: bool,
}

pub enum Event {
    Identifying(Source),
    Source(Source),
    Repositories {
        source: Source,
        stage: &'static str,
        current: usize,
        total: usize,
    },
    Page {
        source: Source,
        operation: &'static str,
        number: usize,
    },
    Request {
        method: &'static str,
        source: Source,
        operation: &'static str,
    },
    RateLimit {
        source: Source,
        remaining: Option<u64>,
        limit: Option<u64>,
        near_limit: Option<bool>,
    },
    Waiting {
        source: Source,
        seconds: u64,
    },
    Candidates {
        source: Source,
        kind: &'static str,
        count: usize,
    },
    Waited {
        source: Source,
        elapsed: Duration,
    },
    Statistics {
        source: Source,
        calls: std::collections::BTreeMap<&'static str, usize>,
        waited: Duration,
    },
    Calculating,
    Finished {
        elapsed: Duration,
        success: bool,
    },
}

impl Progress {
    pub fn new(writer: impl Write + 'static, terminal: bool, quiet: bool, verbose: bool) -> Self {
        Self {
            output: (terminal && !quiet)
                .then(|| Rc::new(RefCell::new(Box::new(writer) as Box<dyn Write>))),
            verbose,
        }
    }

    pub fn emit(&self, event: Event) {
        let Some(output) = &self.output else {
            return;
        };
        let message = match event {
            Event::Identifying(source) => format!("{} : identification", source.label()),
            Event::Source(source) => format!("{} : collecte", source.label()),
            Event::Repositories {
                source,
                stage,
                current,
                total,
            } => format!("{} : {stage} {current}/{total} dépôts", source.label()),
            Event::Page {
                source,
                operation,
                number,
            } => format!("{} : {operation}, page {number}", source.label()),
            Event::Request {
                source,
                operation,
                method,
            } if self.verbose => {
                format!("{} : {method} {operation}", source.label())
            }
            Event::RateLimit {
                source,
                remaining,
                limit,
                near_limit,
            } if self.verbose => {
                let remaining = remaining.map_or_else(
                    || "restant inconnu".into(),
                    |value| format!("{value} restantes"),
                );
                let limit = limit.map_or_else(
                    || "limite inconnue".into(),
                    |value| format!("limite {value}"),
                );
                let near_limit = match near_limit {
                    Some(true) => ", quota presque épuisé",
                    _ => "",
                };
                format!(
                    "{} : rate limit, {remaining}, {limit}{near_limit}",
                    source.label()
                )
            }
            Event::Request { .. } | Event::RateLimit { .. } => return,
            Event::Waiting { source, seconds } => {
                format!("{} : rate limit, reprise dans {seconds} s", source.label())
            }
            Event::Candidates {
                source,
                kind,
                count,
            } if self.verbose => {
                format!("{} : {kind} : {count} candidats", source.label())
            }
            Event::Statistics {
                source,
                calls,
                waited,
            } if self.verbose => {
                let calls = calls
                    .iter()
                    .map(|(endpoint, count)| format!("{endpoint}={count}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{} : appels [{calls}], attente rate limit={:.2} s",
                    source.label(),
                    waited.as_secs_f64()
                )
            }
            Event::Candidates { .. } | Event::Waited { .. } | Event::Statistics { .. } => return,
            Event::Calculating => "Calcul des métriques".into(),
            Event::Finished { elapsed, success } => format!(
                "{} {:.2} s",
                if success {
                    "Terminé en"
                } else {
                    "Échec après"
                },
                elapsed.as_secs_f64()
            ),
        };
        let mut output = output.borrow_mut();
        let _ = writeln!(output, "{message}");
        let _ = output.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct Buffer(Rc<RefCell<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn render(terminal: bool, quiet: bool, verbose: bool) -> String {
        let buffer = Buffer::default();
        let progress = Progress::new(buffer.clone(), terminal, quiet, verbose);
        progress.emit(Event::Source(Source::GitHub));
        progress.emit(Event::Repositories {
            source: Source::GitHub,
            stage: "PRs",
            current: 3,
            total: 12,
        });
        progress.emit(Event::Page {
            source: Source::GitHub,
            operation: "recherche",
            number: 2,
        });
        progress.emit(Event::Request {
            method: "GET",
            source: Source::GitHub,
            operation: "recherche",
        });
        progress.emit(Event::RateLimit {
            source: Source::GitHub,
            remaining: Some(42),
            limit: Some(5000),
            near_limit: None,
        });
        progress.emit(Event::Waiting {
            source: Source::GitHub,
            seconds: 42,
        });
        progress.emit(Event::Calculating);
        progress.emit(Event::Finished {
            elapsed: Duration::from_millis(1250),
            success: true,
        });
        String::from_utf8(buffer.0.borrow().clone()).unwrap()
    }

    #[test]
    fn non_tty_and_quiet_disable_all_output_including_verbose() {
        for verbose in [false, true] {
            assert_eq!(render(false, false, verbose), "");
            assert_eq!(render(true, true, verbose), "");
        }
    }

    #[test]
    fn terminal_shows_stages_pages_waits_and_elapsed_summary() {
        let output = render(true, false, false);
        for expected in [
            "GitHub : collecte",
            "GitHub : PRs 3/12 dépôts",
            "recherche, page 2",
            "rate limit, reprise dans 42 s",
            "Calcul des métriques",
            "Terminé en 1.25 s",
        ] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
        assert!(!output.contains("GET"));
        assert!(!output.contains("restant"));
    }

    #[test]
    fn verbose_adds_api_calls_and_numeric_rate_limit_details() {
        let output = render(true, false, true);
        assert!(output.contains("GitHub : GET recherche"), "{output}");
        assert!(output.contains("42 restantes"), "{output}");
        assert!(output.contains("limite 5000"), "{output}");
    }

    #[test]
    fn failure_summary_does_not_claim_success() {
        let buffer = Buffer::default();
        Progress::new(buffer.clone(), true, false, false).emit(Event::Finished {
            elapsed: Duration::from_secs(2),
            success: false,
        });
        assert_eq!(
            String::from_utf8(buffer.0.borrow().clone()).unwrap(),
            "Échec après 2.00 s\n"
        );
    }

    #[test]
    fn unavailable_progress_output_does_not_abort_the_command() {
        struct BrokenOutput;
        impl Write for BrokenOutput {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        Progress::new(BrokenOutput, true, false, true).emit(Event::Calculating);
    }
}
