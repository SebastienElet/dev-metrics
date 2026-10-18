use super::*;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn render(terminal: bool, quiet: bool, verbose: bool) -> String {
    let buffer = Buffer::default();
    let progress =
        Progress::with_clock(buffer.clone(), terminal, quiet, verbose, || Duration::ZERO);
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
    String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap()
}

#[test]
fn stages_rewrite_one_line_without_leaving_a_transcript() {
    let buffer = Buffer::default();
    let progress = Progress::with_clock(buffer.clone(), true, false, false, || Duration::ZERO);
    progress.emit(Event::Source(Source::GitHub));
    progress.emit(Event::Repositories {
        source: Source::GitHub,
        stage: "PRs",
        current: 3,
        total: 12,
    });
    let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(!output.contains('\n'), "{output:?}");
    assert!(output.starts_with("\r\x1b[2K"), "{output:?}");
    assert!(
        output.contains("GitHub · PRs 3/12 dépôts · 0 s"),
        "{output:?}"
    );
}

#[test]
fn non_tty_and_quiet_disable_all_output_including_verbose() {
    for verbose in [false, true] {
        assert_eq!(render(false, false, verbose), "");
        assert_eq!(render(true, true, verbose), "");
    }
}

#[test]
fn controlled_ticks_animate_the_current_counter_and_elapsed_time() {
    let buffer = Buffer::default();
    let elapsed = Arc::new(Mutex::new(Duration::ZERO));
    let clock = elapsed.clone();
    let progress = Progress::with_clock(buffer.clone(), true, false, false, move || {
        *clock.lock().unwrap()
    });
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
    let mut times = [Duration::from_millis(90), Duration::from_secs(12)].into_iter();
    animate(&progress.session.as_ref().unwrap().display, |interval| {
        assert_eq!(interval, Duration::from_millis(90));
        let Some(time) = times.next() else {
            return false;
        };
        *elapsed.lock().unwrap() = time;
        true
    });
    assert_eq!(
        String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap(),
        concat!(
            "\r\x1b[2K⠋ GitHub · PRs 3/12 dépôts · 0 s",
            "\r\x1b[2K⠙ GitHub · PRs 3/12 dépôts · 0 s",
            "\r\x1b[2K⠹ GitHub · PRs 3/12 dépôts · 12 s",
        )
    );
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
        String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap(),
        "\r\x1b[2KÉchec après 2.00 s · 0 avertissement\n"
    );
}

#[test]
fn finishing_clears_the_spinner_once_and_ignores_late_events_and_ticks() {
    for (success, summary) in [(true, "Terminé en"), (false, "Échec après")] {
        let buffer = Buffer::default();
        let progress = Progress::with_clock(buffer.clone(), true, false, false, || Duration::ZERO);
        let clone = progress.clone();
        progress.emit(Event::Calculating);
        progress.emit(Event::Warnings { count: 2 });
        progress.emit(Event::Finished {
            elapsed: Duration::from_millis(1250),
            success,
        });
        clone.emit(Event::Source(Source::Bitbucket));
        clone.emit(Event::Finished {
            elapsed: Duration::from_secs(3),
            success: !success,
        });
        animate(&progress.session.as_ref().unwrap().display, |_| true);
        drop(clone);
        drop(progress);
        assert_eq!(
            String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap(),
            format!(
                "\r\x1b[2K⠋ Calcul des métriques · 0 s\r\x1b[2K{summary} 1.25 s · 2 avertissements\n"
            )
        );
    }
}

#[test]
fn dropping_the_last_handle_clears_an_unfinished_spinner() {
    let buffer = Buffer::default();
    let progress = Progress::new(buffer.clone(), true, false, false);
    let clone = progress.clone();
    progress.emit(Event::Calculating);
    drop(clone);
    drop(progress);
    let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(output.ends_with("\r\x1b[2K"), "{output:?}");
    assert!(!output.contains('\n'));
}

#[test]
fn stopping_prevents_any_redraw_before_the_final_summary() {
    let buffer = Buffer::default();
    let progress = Progress::with_clock(buffer.clone(), true, false, false, || Duration::ZERO);
    progress.emit(Event::Calculating);
    progress.emit(Event::Warnings { count: 1 });
    progress.stop();
    progress.stop();
    progress.emit(Event::Calculating);
    animate(&progress.session.as_ref().unwrap().display, |_| true);
    let mut writer = buffer.clone();
    writeln!(writer, "avertissement").unwrap();
    progress.emit(Event::Finished {
        elapsed: Duration::from_secs(2),
        success: false,
    });
    drop(progress);
    assert_eq!(
        String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap(),
        concat!(
            "\r\x1b[2K⠋ Calcul des métriques · 0 s\r\x1b[2Kavertissement\n",
            "Échec après 2.00 s · 1 avertissement\n",
        )
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

#[test]
fn retry_restores_the_repository_counter_after_waiting() {
    for verbose in [false, true] {
        let buffer = Buffer::default();
        let progress = Progress::with_clock(buffer.clone(), true, false, verbose, || {
            Duration::from_secs(12)
        });
        progress.emit(Event::Repositories {
            source: Source::GitHub,
            stage: "PRs",
            current: 3,
            total: 12,
        });
        for seconds in [2, 1, 0] {
            progress.emit(Event::Waiting {
                source: Source::GitHub,
                seconds,
            });
        }
        progress.emit(Event::Request {
            source: Source::GitHub,
            operation: "pull request",
        });
        progress.emit(Event::Page {
            source: Source::GitHub,
            operation: "files",
            number: 2,
        });
        let mut ticks = 1;
        animate(&progress.session.as_ref().unwrap().display, |_| {
            let tick = ticks > 0;
            ticks = 0;
            tick
        });
        let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        assert!(output.contains("reprise dans 1 s"), "{output:?}");
        assert!(
            output.ends_with("⠙ GitHub · PRs 3/12 dépôts · 12 s"),
            "{output:?}"
        );
    }
}
