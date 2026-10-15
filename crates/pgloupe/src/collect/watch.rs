//! The loop behind `watch`: every interval take a snapshot, fetch new log text, ingest it into a
//! store, rebuild the report, and publish it.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

use jiff::Timestamp;

/// How often the served dashboard asks for news, in seconds.
const LIVE_POLL_SECS: u32 = 15;

use super::{
    conn::ConnSpec,
    fetch::DirSink,
    provider::{LogProvider, Request},
    serve::Shared,
    sql,
};
use crate::{
    analyze::{
        pipeline,
        report::{ReportOptions, SortBy, all_statements},
    },
    combined::{self, LogData},
    stats::{
        delta,
        snapshot::{Info, Snapshot},
    },
    store::{self, Status, Store, report as store_report},
};

/// A report ready to publish.
#[derive(Debug, Clone)]
pub struct Built {
    /// The dashboard served by `watch`: it polls the server for newer reports.
    pub html: String,
    /// The same report as a plain file, for `--out`: it never polls.
    pub page: String,
    /// The report as JSON.
    pub json: String,
    /// Steps that failed but did not stop the report (shown on the page and in the log).
    pub notes: Vec<String>,
}

/// A cycle that could not produce a report.
#[derive(Debug, Clone)]
pub struct CycleError {
    /// Which step.
    pub step: &'static str,
    /// What went wrong, without secrets.
    pub message: String,
}

/// One pass of the loop.
pub trait Cycle {
    /// Collects, ingests and builds the report.
    ///
    /// # Errors
    ///
    /// Fails when no report could be built.
    fn run(&mut self) -> Result<Built, CycleError>;
}

/// Time, so tests need not wait.
pub trait Clock {
    /// The current time.
    fn now(&self) -> Timestamp;
    /// Waits.
    fn sleep(&self, duration: Duration);
}

/// The real clock.
#[derive(Debug, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

/// How the loop runs.
#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// Time between cycles.
    pub interval: Duration,
    /// Stop after this many cycles (`None`: run until killed).
    pub max_cycles: Option<u32>,
}

/// The wait after `failures` failed cycles in a row: a quick retry first, up to the interval.
#[must_use]
pub fn backoff(interval: Duration, failures: u32) -> Duration {
    if failures == 0 {
        return interval;
    }
    let quick = Duration::from_secs(60).saturating_mul(1 << failures.saturating_sub(1).min(10));
    quick.min(interval)
}

fn stamp(at: Timestamp) -> String {
    at.to_string()
}

/// Runs cycles, publishing each result into `shared`, and returns after `max_cycles`.
pub fn run(
    settings: Settings,
    cycle: &mut dyn Cycle,
    clock: &dyn Clock,
    shared: &Shared,
    log: &mut dyn FnMut(&str),
) {
    let mut failures = 0_u32;
    let mut done = 0_u32;
    loop {
        let result = cycle.run();
        let now = clock.now();
        let wait = match &result {
            Ok(_) => backoff(settings.interval, 0),
            Err(_) => backoff(settings.interval, failures + 1),
        };
        let next = now
            .checked_add(jiff::SignedDuration::from_secs(
                i64::try_from(wait.as_secs()).unwrap_or(0),
            ))
            .unwrap_or(now);
        {
            let mut published = match shared.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            published.interval_secs = settings.interval.as_secs();
            published.next_at = Some(stamp(next));
            match result {
                Ok(built) => {
                    failures = 0;
                    for note in &built.notes {
                        log(&format!("warning: {note}"));
                    }
                    published.generation += 1;
                    published.updated_at = Some(stamp(now));
                    published.html = Arc::new(built.html);
                    published.json = Arc::new(built.json);
                    published.last_error =
                        (!built.notes.is_empty()).then(|| built.notes.join("; "));
                    log(&format!("updated (report {})", published.generation));
                }
                Err(error) => {
                    failures += 1;
                    let message = format!("{}: {}", error.step, error.message);
                    log(&format!("error: {message}"));
                    published.last_error = Some(message);
                }
            }
        }
        done += 1;
        if settings.max_cycles.is_some_and(|max| done >= max) {
            return;
        }
        clock.sleep(wait);
    }
}

/// The collection steps against a real server and provider.
pub struct Live {
    spec: ConnSpec,
    provider: Box<dyn LogProvider>,
    dir: PathBuf,
    store: PathBuf,
    window: Duration,
    keep: usize,
    options: pipeline::Options,
    checked: bool,
}

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl Live {
    /// Steps that keep their files under `dir` and their store at `store`.
    #[must_use]
    pub fn new(
        spec: ConnSpec,
        provider: Box<dyn LogProvider>,
        dir: PathBuf,
        store: PathBuf,
        window: Duration,
        keep: usize,
        options: pipeline::Options,
    ) -> Self {
        Live {
            spec,
            provider,
            dir,
            store,
            window,
            keep: keep.max(2),
            options,
            checked: false,
        }
    }

    fn snapshots_dir(&self) -> PathBuf {
        self.dir.join("snapshots")
    }

    fn logs_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }

    fn snapshot(&self) -> Result<(), String> {
        let mut client = self.spec.connect().map_err(|e| e.to_string())?;
        sql::take(&mut client, &self.snapshots_dir()).map_err(|e| e.to_string())?;
        prune_snapshots(&self.snapshots_dir(), self.keep);
        Ok(())
    }

    fn fetch(&mut self) -> Result<(), String> {
        if !self.checked {
            self.provider.check().map_err(|e| e.to_string())?;
            self.checked = true;
        }
        let mut sink =
            DirSink::open(&self.logs_dir(), self.provider.name()).map_err(|e| e.to_string())?;
        let mut state = sink.state();
        let since = Timestamp::now()
            .checked_sub(jiff::SignedDuration::from_secs(
                i64::try_from(self.window.as_secs()).unwrap_or(0),
            ))
            .map_err(|e| e.to_string())?;
        self.provider
            .fetch(&Request { since, until: None }, &mut sink, &mut state)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn ingest(&self) -> Result<(), String> {
        let paths: Vec<PathBuf> = fs::read_dir(self.logs_dir())
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && !p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            })
            .collect();
        let mut store = Store::create(&self.store).map_err(|e| e.to_string())?;
        let mut failed = Vec::new();
        for outcome in store::ingest(&mut store, &paths, &self.options, false) {
            if let Status::Failed(reason) = outcome.status {
                failed.push(format!("{}: {reason}", outcome.path.display()));
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(failed.join("; "))
        }
    }

    fn report(&self, mut notes: Vec<String>) -> Result<Built, CycleError> {
        let fail = |step: &'static str, message: String| CycleError { step, message };
        let options = ReportOptions {
            by: SortBy::Total,
            top: 100,
        };
        let since = Timestamp::now()
            .checked_sub(jiff::SignedDuration::from_secs(
                i64::try_from(self.window.as_secs()).unwrap_or(0),
            ))
            .map_err(|e| fail("report", e.to_string()))?;
        let logs = match Store::open_existing(&self.store) {
            Ok(store) => {
                let (report, acc) =
                    store_report::build_parts(&store, &self.store, Some(since), None, &options, 60)
                        .map_err(|e| fail("report", e.to_string()))?;
                Some(LogData {
                    rows: all_statements(&acc),
                    inputs: report.files.len(),
                    report,
                    source: "store",
                    store: Some(self.store.display().to_string()),
                })
            }
            Err(e) => {
                notes.push(format!("logs: nothing ingested yet ({e})"));
                None
            }
        };
        let delta = match statistics(&self.snapshots_dir(), since) {
            Ok(delta) => delta,
            Err(note) => {
                notes.push(note);
                None
            }
        };
        if logs.is_none() && delta.is_none() {
            let why = notes.join("; ");
            return Err(fail("report", format!("nothing to report yet: {why}")));
        }
        let combined = combined::build(logs, delta.as_ref(), &options);
        let html = combined::html_live(&combined, LIVE_POLL_SECS)
            .map_err(|e| fail("report", e.to_string()))?;
        let page = combined::html(&combined).map_err(|e| fail("report", e.to_string()))?;
        let json = combined
            .to_json()
            .map_err(|e| fail("report", e.to_string()))?;
        Ok(Built {
            html,
            page,
            json,
            notes,
        })
    }
}

/// The statistics delta between the oldest snapshot inside the window and the newest.
fn statistics(dir: &Path, since: Timestamp) -> Result<Option<delta::Delta>, String> {
    let files = snapshot_files(dir);
    let Some(after) = files.last() else {
        return Ok(None);
    };
    let before = files
        .iter()
        .find(|p| snapshot_time(p).is_some_and(|t| t >= since))
        .or_else(|| files.first());
    let Some(before) = before.filter(|b| *b != after) else {
        return Ok(None);
    };
    let load = |path: &Path| -> Result<Snapshot, String> {
        let mut snapshot = Snapshot::from_path(path).map_err(|e| e.to_string())?;
        let info = info_path(path);
        if info.exists() {
            snapshot = snapshot.with_info(Info::from_path(&info).map_err(|e| e.to_string())?);
        }
        Ok(snapshot)
    };
    delta::diff(&load(before)?, &load(after)?)
        .map(Some)
        .map_err(|e| format!("statistics: {e}"))
}

/// Deletes the oldest snapshots (and their info files) beyond `keep`.
fn prune_snapshots(dir: &Path, keep: usize) {
    let files = snapshot_files(dir);
    for old in files.iter().take(files.len().saturating_sub(keep)) {
        drop(fs::remove_file(old));
        drop(fs::remove_file(info_path(old)));
    }
}

/// Deletes fetched log files not written since `cutoff`; the report only reads its window.
fn prune_logs(dir: &Path, cutoff: std::time::SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        let old = entry
            .metadata()
            .ok()
            .filter(fs::Metadata::is_file)
            .and_then(|m| m.modified().ok())
            .is_some_and(|t| t < cutoff);
        if old && !hidden {
            drop(fs::remove_file(entry.path()));
        }
    }
}

impl Cycle for Live {
    fn run(&mut self) -> Result<Built, CycleError> {
        let mut notes = Vec::new();
        if let Err(e) = self.snapshot() {
            notes.push(format!("snapshot: {e}"));
        }
        if let Err(e) = self.fetch() {
            notes.push(format!("fetch: {e}"));
        }
        if let Err(e) = self.ingest() {
            notes.push(format!("ingest: {e}"));
        }
        // Keep a margin past the window so a file still being filled is never cut from under it.
        let keep_for = self.window + std::time::Duration::from_secs(2 * 3600);
        if let Some(cutoff) = std::time::SystemTime::now().checked_sub(keep_for) {
            prune_logs(&self.logs_dir(), cutoff);
        }
        self.report(notes)
    }
}

/// The `pgss_<time>.csv` files in `dir`, oldest first.
#[must_use]
pub fn snapshot_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("pgss_2") && p.extension().is_some_and(|e| e == "csv")
            })
        })
        .collect();
    files.sort();
    files
}

/// The info file that goes with a snapshot file.
#[must_use]
pub fn info_path(snapshot: &Path) -> PathBuf {
    snapshot.with_file_name(snapshot.file_name().map_or_else(String::new, |n| {
        n.to_string_lossy().replacen("pgss_", "pgss_info_", 1)
    }))
}

/// The time in a `pgss_20261014T102009Z.csv` file name.
#[must_use]
pub fn snapshot_time(path: &Path) -> Option<Timestamp> {
    let name = path.file_name()?.to_str()?;
    let stamp = name.strip_prefix("pgss_")?.strip_suffix(".csv")?;
    if stamp.len() != 16 {
        return None;
    }
    let iso = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &stamp[0..4],
        &stamp[4..6],
        &stamp[6..8],
        &stamp[9..11],
        &stamp[11..13],
        &stamp[13..15]
    );
    iso.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::collect::serve::Published;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pgloupe-watch-{}-{name}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Snapshot files named by time, with fixture contents stamped to match.
    fn snapshots(dir: &Path) {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/stats");
        let before = fs::read_to_string(fixtures.join("basic_before.csv")).unwrap();
        let after = fs::read_to_string(fixtures.join("basic_after.csv")).unwrap();
        let early = before.replace("2026-10-14 10:00:00", "2026-10-14 09:00:00");
        fs::write(dir.join("pgss_20261014T090000Z.csv"), early).unwrap();
        fs::write(dir.join("pgss_20261014T100000Z.csv"), before).unwrap();
        fs::write(dir.join("pgss_20261014T101000Z.csv"), after).unwrap();
    }

    #[test]
    fn the_statistics_start_at_the_oldest_snapshot_inside_the_window() {
        let dir = scratch("window");
        snapshots(&dir);
        let at = |s: &str| s.parse::<Timestamp>().unwrap();
        let wide = statistics(&dir, at("2026-10-14T08:00:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(wide.before_at, at("2026-10-14T09:00:00Z"));
        let narrow = statistics(&dir, at("2026-10-14T09:30:00Z"))
            .unwrap()
            .unwrap();
        assert_eq!(narrow.before_at, at("2026-10-14T10:00:00Z"));
    }

    #[test]
    fn one_snapshot_or_none_gives_no_statistics_side() {
        let dir = scratch("single");
        assert!(statistics(&dir, Timestamp::UNIX_EPOCH).unwrap().is_none());
        fs::write(dir.join("pgss_20261014T100000Z.csv"), "x").unwrap();
        assert!(statistics(&dir, Timestamp::UNIX_EPOCH).unwrap().is_none());
    }

    #[test]
    fn only_the_newest_snapshots_are_kept_with_their_info_files() {
        let dir = scratch("prune");
        snapshots(&dir);
        for stamp in ["20261014T090000Z", "20261014T100000Z"] {
            fs::write(dir.join(format!("pgss_info_{stamp}.csv")), "i").unwrap();
        }
        prune_snapshots(&dir, 2);
        let left: Vec<String> = snapshot_files(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            left,
            ["pgss_20261014T100000Z.csv", "pgss_20261014T101000Z.csv"]
        );
        assert!(!dir.join("pgss_info_20261014T090000Z.csv").exists());
        assert!(dir.join("pgss_info_20261014T100000Z.csv").exists());
    }

    #[test]
    fn old_log_files_go_but_hidden_state_stays() {
        let dir = scratch("logs");
        fs::write(dir.join("a.log"), "x").unwrap();
        fs::write(dir.join(".fetch.json"), "{}").unwrap();
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        prune_logs(&dir, future);
        assert!(!dir.join("a.log").exists());
        assert!(dir.join(".fetch.json").exists());
        fs::write(dir.join("b.log"), "x").unwrap();
        prune_logs(&dir, std::time::SystemTime::UNIX_EPOCH);
        assert!(dir.join("b.log").exists());
    }

    struct Scripted(Vec<Result<Built, CycleError>>);

    impl Cycle for Scripted {
        fn run(&mut self) -> Result<Built, CycleError> {
            if self.0.is_empty() {
                return Err(CycleError {
                    step: "script",
                    message: "ran out".to_owned(),
                });
            }
            self.0.remove(0)
        }
    }

    struct FakeClock {
        now: RefCell<Timestamp>,
        slept: RefCell<Vec<Duration>>,
    }

    impl Clock for FakeClock {
        fn now(&self) -> Timestamp {
            *self.now.borrow()
        }

        fn sleep(&self, duration: Duration) {
            self.slept.borrow_mut().push(duration);
            let next = *self.now.borrow()
                + jiff::SignedDuration::from_secs(i64::try_from(duration.as_secs()).unwrap());
            *self.now.borrow_mut() = next;
        }
    }

    fn built(tag: &str) -> Built {
        Built {
            html: format!("<html>{tag}</html>"),
            page: format!("<html>{tag}</html>"),
            json: format!("{{\"tag\":\"{tag}\"}}"),
            notes: Vec::new(),
        }
    }

    fn failing(step: &'static str) -> Result<Built, CycleError> {
        Err(CycleError {
            step,
            message: "boom".to_owned(),
        })
    }

    fn clock() -> FakeClock {
        FakeClock {
            now: RefCell::new("2026-10-14T12:00:00Z".parse().unwrap()),
            slept: RefCell::new(Vec::new()),
        }
    }

    fn settings(cycles: u32) -> Settings {
        Settings {
            interval: Duration::from_secs(300),
            max_cycles: Some(cycles),
        }
    }

    #[test]
    fn each_good_cycle_publishes_a_new_generation() {
        let shared: Shared = Arc::default();
        let mut cycle = Scripted(vec![Ok(built("a")), Ok(built("b"))]);
        let clock = clock();
        run(settings(2), &mut cycle, &clock, &shared, &mut |_| {});
        let p = shared.read().unwrap().clone();
        assert_eq!(p.generation, 2);
        assert_eq!(p.html.as_str(), "<html>b</html>");
        assert_eq!(p.interval_secs, 300);
        assert!(p.last_error.is_none());
        assert_eq!(*clock.slept.borrow(), [Duration::from_secs(300)]);
    }

    #[test]
    fn a_failed_cycle_keeps_the_last_good_report_and_says_what_failed() {
        let shared: Shared = Arc::default();
        let mut cycle = Scripted(vec![
            Ok(built("good")),
            failing("fetch"),
            failing("fetch"),
            failing("fetch"),
            Ok(built("again")),
        ]);
        let clock = clock();
        let mut log = Vec::new();
        run(settings(5), &mut cycle, &clock, &shared, &mut |line| {
            log.push(line.to_owned());
        });
        let p = shared.read().unwrap().clone();
        assert_eq!(
            p.generation, 2,
            "one good report before the failures, one after"
        );
        assert_eq!(p.html.as_str(), "<html>again</html>");
        assert!(p.last_error.is_none(), "cleared by the good cycle");
        assert!(
            log.iter()
                .filter(|l| l.contains("error: fetch: boom"))
                .count()
                == 3,
            "{log:?}"
        );
        // After failures the wait is a quick retry that grows up to the interval.
        assert_eq!(
            *clock.slept.borrow(),
            [
                Duration::from_secs(300),
                Duration::from_secs(60),
                Duration::from_secs(120),
                Duration::from_secs(240)
            ]
        );
    }

    #[test]
    fn the_report_stays_available_while_a_failure_is_reported() {
        let shared: Shared = Arc::new(std::sync::RwLock::new(Published::default()));
        let mut cycle = Scripted(vec![Ok(built("good")), failing("report")]);
        run(settings(2), &mut cycle, &clock(), &shared, &mut |_| {});
        let p = shared.read().unwrap().clone();
        assert_eq!(p.generation, 1);
        assert_eq!(p.html.as_str(), "<html>good</html>");
        assert!(p.last_error.as_deref().unwrap().contains("report: boom"));
    }

    #[test]
    fn notes_from_partial_failures_are_published_with_the_report() {
        let shared: Shared = Arc::default();
        let mut cycle = Scripted(vec![Ok(Built {
            html: "h".to_owned(),
            page: "h".to_owned(),
            json: "{}".to_owned(),
            notes: vec!["snapshot: refused".to_owned()],
        })]);
        run(settings(1), &mut cycle, &clock(), &shared, &mut |_| {});
        let p = shared.read().unwrap().clone();
        assert_eq!(p.generation, 1);
        assert_eq!(p.last_error.as_deref(), Some("snapshot: refused"));
    }

    #[test]
    fn backoff_never_exceeds_the_interval() {
        let interval = Duration::from_secs(300);
        assert_eq!(backoff(interval, 0), interval);
        assert_eq!(backoff(interval, 1), Duration::from_secs(60));
        assert_eq!(backoff(interval, 3), Duration::from_secs(240));
        assert_eq!(backoff(interval, 4), interval);
        assert_eq!(backoff(interval, 40), interval);
        assert_eq!(backoff(Duration::from_secs(30), 1), Duration::from_secs(30));
    }

    #[test]
    fn snapshot_names_carry_their_time() {
        let path = Path::new("/x/pgss_20261014T102009Z.csv");
        assert_eq!(
            snapshot_time(path).unwrap().to_string(),
            "2026-10-14T10:20:09Z"
        );
        assert_eq!(
            info_path(path),
            Path::new("/x/pgss_info_20261014T102009Z.csv")
        );
        assert!(snapshot_time(Path::new("pgss_info_20261014T102009Z.csv")).is_none());
        assert!(snapshot_time(Path::new("other.csv")).is_none());
    }
}
