use crate::activity::Source;
use std::io::Write;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TICK_INTERVAL: Duration = Duration::from_millis(90);
const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const CLEAR_LINE: &str = "\r\x1b[2K";

#[derive(Clone, Default)]
pub struct Progress {
    session: Option<Arc<Session>>,
}

struct Session {
    display: Arc<Mutex<Display>>,
    ticker: Mutex<Option<Ticker>>,
}

struct Ticker {
    stop: mpsc::Sender<()>,
    worker: JoinHandle<()>,
}

struct Display {
    writer: Box<dyn Write + Send>,
    clock: Box<dyn Fn() -> Duration + Send>,
    verbose: bool,
    message: Option<String>,
    waiting: Option<String>,
    frame: usize,
    state: DisplayState,
    warnings: usize,
}

#[derive(PartialEq, Eq)]
enum DisplayState {
    Running,
    Stopped,
    Finished,
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
    EndpointSummary {
        source: Source,
        operation: &'static str,
        requests: usize,
        pages: usize,
        elapsed: Duration,
    },
    CollectionSummary {
        source: Source,
        elapsed: Duration,
        cache_hits: usize,
        cache_misses: usize,
    },
    Calculating,
    Warnings {
        count: usize,
    },
    Finished {
        elapsed: Duration,
        success: bool,
    },
}

impl Progress {
    pub fn new(
        writer: impl Write + Send + 'static,
        terminal: bool,
        quiet: bool,
        verbose: bool,
    ) -> Self {
        let started = Instant::now();
        let progress =
            Self::with_clock(writer, terminal, quiet, verbose, move || started.elapsed());
        if let Some(session) = &progress.session {
            let display = session.display.clone();
            let (stop, receiver) = mpsc::channel();
            let worker = thread::Builder::new()
                .name("progress".into())
                .spawn(move || {
                    animate(&display, |interval| {
                        matches!(
                            receiver.recv_timeout(interval),
                            Err(mpsc::RecvTimeoutError::Timeout)
                        )
                    });
                });
            if let Ok(worker) = worker {
                *session.ticker.lock().unwrap() = Some(Ticker { stop, worker });
            }
        }
        progress
    }

    fn with_clock(
        writer: impl Write + Send + 'static,
        terminal: bool,
        quiet: bool,
        verbose: bool,
        clock: impl Fn() -> Duration + Send + 'static,
    ) -> Self {
        Self {
            session: (terminal && !quiet).then(|| {
                Arc::new(Session {
                    display: Arc::new(Mutex::new(Display {
                        writer: Box::new(writer),
                        clock: Box::new(clock),
                        verbose,
                        message: None,
                        waiting: None,
                        frame: 0,
                        state: DisplayState::Running,
                        warnings: 0,
                    })),
                    ticker: Mutex::new(None),
                })
            }),
        }
    }

    pub fn stop(&self) {
        let Some(session) = &self.session else { return };
        session.stop();
        let mut display = session.display.lock().unwrap();
        if display.state == DisplayState::Running {
            if display.message.is_some() || display.waiting.is_some() {
                display.clear();
                let _ = display.writer.flush();
            }
            display.state = DisplayState::Stopped;
        }
    }

    pub fn emit(&self, event: Event) {
        let Some(session) = &self.session else {
            return;
        };
        if matches!(event, Event::Finished { .. }) {
            session.stop();
        }
        session.display.lock().unwrap().emit(event);
    }
}

impl Session {
    fn stop(&self) {
        if let Some(ticker) = self.ticker.lock().unwrap().take() {
            let _ = ticker.stop.send(());
            let _ = ticker.worker.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
        let mut display = self.display.lock().unwrap();
        if display.state == DisplayState::Running
            && (display.message.is_some() || display.waiting.is_some())
        {
            display.clear();
            let _ = display.writer.flush();
        }
    }
}

fn animate(display: &Mutex<Display>, mut next_tick: impl FnMut(Duration) -> bool) {
    while next_tick(TICK_INTERVAL) {
        let mut display = display.lock().unwrap();
        if display.state != DisplayState::Running {
            break;
        }
        display.tick();
    }
}

impl Display {
    fn clear(&mut self) {
        let _ = write!(self.writer, "{CLEAR_LINE}");
    }

    fn render(&mut self) {
        if self.state != DisplayState::Running {
            return;
        }
        if let Some(message) = self.waiting.as_ref().or(self.message.as_ref()) {
            let message = message.replacen(" : ", " · ", 1);
            let _ = write!(
                self.writer,
                "{CLEAR_LINE}{} {message} · {} s",
                FRAMES[self.frame],
                (self.clock)().as_secs()
            );
            let _ = self.writer.flush();
        }
    }

    fn tick(&mut self) {
        self.frame = (self.frame + 1) % FRAMES.len();
        self.render();
    }

    fn emit(&mut self, event: Event) {
        if self.state == DisplayState::Finished {
            return;
        }
        if let Event::Finished { elapsed, success } = event {
            if self.state == DisplayState::Running {
                self.clear();
            }
            let _ = writeln!(
                self.writer,
                "{} {:.2} s · {} avertissement{}",
                if success {
                    "Terminé en"
                } else {
                    "Échec après"
                },
                elapsed.as_secs_f64(),
                self.warnings,
                if self.warnings > 1 { "s" } else { "" }
            );
            let _ = self.writer.flush();
            self.state = DisplayState::Finished;
            return;
        }
        if self.state == DisplayState::Stopped {
            return;
        }
        if let Event::Warnings { count } = event {
            self.warnings = count;
            return;
        }
        if matches!(event, Event::Request { .. }) && self.waiting.take().is_some() {
            self.clear();
            self.render();
            let _ = self.writer.flush();
        }
        let waiting = matches!(event, Event::Waiting { .. });
        let detail = matches!(
            event,
            Event::Page { .. }
                | Event::Request { .. }
                | Event::RateLimit { .. }
                | Event::EndpointSummary { .. }
                | Event::CollectionSummary { .. }
        );
        if detail && !self.verbose {
            return;
        }
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
            Event::Request { source, operation } => format!("{} : GET {operation}", source.label()),
            Event::RateLimit {
                source,
                remaining,
                limit,
                near_limit,
            } => {
                let remaining = remaining.map_or_else(
                    || "restant inconnu".into(),
                    |value| format!("{value} restantes"),
                );
                let limit = limit.map_or_else(
                    || "limite inconnue".into(),
                    |value| format!("limite {value}"),
                );
                let near_limit = if near_limit == Some(true) {
                    ", quota presque épuisé"
                } else {
                    ""
                };
                format!(
                    "{} : rate limit, {remaining}, {limit}{near_limit}",
                    source.label()
                )
            }
            Event::Waiting { source, seconds } => {
                format!("{} : rate limit, reprise dans {seconds} s", source.label())
            }
            Event::EndpointSummary {
                source,
                operation,
                requests,
                pages,
                elapsed,
            } => format!(
                "{} : {operation} : {requests} requêtes, {pages} pages, {:.3} s",
                source.label(),
                elapsed.as_secs_f64()
            ),
            Event::CollectionSummary {
                source,
                elapsed,
                cache_hits,
                cache_misses,
            } => format!(
                "{} : collecte en {:.3} s ; cache : {cache_hits} hits, {cache_misses} miss",
                source.label(),
                elapsed.as_secs_f64()
            ),
            Event::Calculating => "Calcul des métriques".into(),
            Event::Finished { .. } | Event::Warnings { .. } => unreachable!(),
        };
        if self.verbose {
            self.clear();
            let _ = writeln!(self.writer, "{message}");
            let _ = self.writer.flush();
        }
        if waiting {
            self.waiting = Some(message);
        } else if !detail {
            self.message = Some(message);
            self.waiting = None;
        }
        self.render();
    }
}

#[cfg(test)]
mod tests;
