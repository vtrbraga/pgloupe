//! Amazon RDS and Aurora PostgreSQL, through the `aws` CLI.
//!
//! Supported: this is the connector tested against a real account.

use std::{
    io::Write as _,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

use jiff::Timestamp;
use serde::Deserialize;

use super::{
    FetchError, LogProvider, LogSink, ProviderState, Request, Summary,
    cmd::{Output, Tool},
};

/// Calls retried when AWS says to slow down.
const RETRIES: u32 = 4;
/// Portions fetched for one file before giving up (a guard against a marker that never ends).
const MAX_PORTIONS: u32 = 1_000_000;

/// Files downloaded at the same time unless told otherwise.
pub const DEFAULT_JOBS: usize = 4;
/// The most files downloaded at the same time.
pub const MAX_JOBS: usize = 16;

const INSTALL_HINT: &str =
    "install the AWS CLI (https://docs.aws.amazon.com/cli/) or pass its path with --bin";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LogFiles {
    #[serde(default, rename = "DescribeDBLogFiles")]
    describe_db_log_files: Vec<LogFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LogFile {
    #[serde(rename = "LogFileName")]
    name: String,
    #[serde(default)]
    last_written: i64,
    #[serde(default)]
    size: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Portion {
    #[serde(default)]
    log_file_data: Option<String>,
    #[serde(default)]
    marker: Option<String>,
    #[serde(default)]
    additional_data_pending: bool,
}

/// What to try for the failures AWS reports most.
pub(super) fn explain(stderr: &str) -> &'static str {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("unable to locate credentials") || lower.contains("no credentials") {
        "no AWS credentials found: run `aws configure`, `aws sso login`, or pass --profile"
    } else if lower.contains("expiredtoken")
        || lower.contains("token has expired")
        || lower.contains("sso session")
    {
        "the AWS session expired: sign in again (`aws sso login` or refresh the credentials)"
    } else if lower.contains("accessdenied") || lower.contains("not authorized") {
        "the role needs rds:DescribeDBLogFiles and rds:DownloadDBLogFilePortion on the instance"
    } else if lower.contains("dbinstancenotfound") {
        "no such instance: check --instance and --region"
    } else if lower.contains("could not connect to the endpoint")
        || lower.contains("could not resolve")
    {
        "cannot reach AWS: check the network and --region"
    } else {
        ""
    }
}

fn is_throttle(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("throttl")
        || lower.contains("rate exceeded")
        || lower.contains("too many requests")
}

/// A file name that is safe as a local path component.
#[must_use]
pub fn safe_name(remote: &str) -> String {
    let flat: String = remote
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if flat.is_empty() || flat.chars().all(|c| c == '.') {
        format!("_{flat}")
    } else {
        flat
    }
}

/// The `aws` CLI with the account options every call carries; shared by the AWS connectors.
#[derive(Debug, Clone)]
pub struct AwsCli {
    tool: Tool,
    region: Option<String>,
    profile: Option<String>,
    retry_delay: Duration,
}

impl AwsCli {
    /// The CLI at `program` with `--region` and `--profile` as given.
    #[must_use]
    pub fn new(program: PathBuf, region: Option<String>, profile: Option<String>) -> Self {
        AwsCli {
            tool: Tool {
                program,
                timeout_secs: 120,
                install_hint: INSTALL_HINT,
                env: vec![("AWS_PAGER", "")],
            },
            region,
            profile,
            retry_delay: Duration::from_secs(1),
        }
    }

    /// Waits this long (doubling) before retrying a throttled call.
    #[must_use]
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// `service operation --region … --profile … --output json`.
    #[must_use]
    pub fn base(&self, service: &str, operation: &str) -> Vec<String> {
        let mut args = vec![service.to_owned(), operation.to_owned()];
        if let Some(region) = &self.region {
            args.push("--region".to_owned());
            args.push(region.clone());
        }
        if let Some(profile) = &self.profile {
            args.push("--profile".to_owned());
            args.push(profile.clone());
        }
        args.push("--output".to_owned());
        args.push("json".to_owned());
        args
    }

    /// Runs one call, retrying while AWS throttles.
    ///
    /// # Errors
    ///
    /// Fails with the fix for the failures AWS reports most.
    pub fn call(&self, args: &[String], command: &str) -> Result<Vec<u8>, FetchError> {
        let mut delay = self.retry_delay;
        for attempt in 0..=RETRIES {
            let output: Output = self.tool.run(args)?;
            if output.code == Some(0) {
                return Ok(output.stdout);
            }
            if attempt < RETRIES && is_throttle(&output.stderr) {
                thread::sleep(delay);
                delay *= 2;
                continue;
            }
            let hint = explain(&output.stderr);
            return Err(FetchError::Command {
                program: self.tool.program.display().to_string(),
                command: command.to_owned(),
                status: output
                    .code
                    .map_or_else(|| "killed".to_owned(), |c| format!("exit {c}")),
                stderr: output.stderr,
                hint: if hint.is_empty() {
                    String::new()
                } else {
                    format!(" — {hint}")
                },
            });
        }
        unreachable_attempts(command)
    }

    /// The CLI is installed and its credentials work.
    ///
    /// # Errors
    ///
    /// Fails with the fix when it is missing or not signed in.
    pub fn check(&self) -> Result<(), FetchError> {
        self.tool
            .run_ok(&["--version".to_owned()], "--version", |_| "")?;
        let mut args = self.base("sts", "get-caller-identity");
        args.retain(|a| a != "json" && a != "--output");
        self.call(&args, "sts get-caller-identity").map(|_| ())
    }
}

/// Where RDS/Aurora logs are read from.
#[derive(Debug, Clone)]
pub struct AwsRds {
    cli: AwsCli,
    instance: String,
    jobs: usize,
    progress: bool,
}

/// One file to download, after the checks that need the sink and the saved state.
#[derive(Debug)]
struct Job {
    name: String,
    local: String,
    size: u64,
    marker: String,
}

/// Tells workers to stop: everything after a failed file, or everything once the sink fails.
/// Files before the failed one keep going so the error reported is the first in the list.
struct Halt<'a> {
    stop: &'a AtomicBool,
    failed: &'a AtomicUsize,
}

impl Halt<'_> {
    fn stops(&self, job: usize) -> bool {
        self.stop.load(Ordering::Relaxed) || job > self.failed.load(Ordering::Relaxed)
    }
}

/// What a download worker tells the thread that owns the sink.
enum Event {
    Data {
        job: usize,
        text: String,
        marker: String,
    },
    Done {
        job: usize,
        took: Duration,
    },
    Failed {
        job: usize,
        error: FetchError,
    },
}

impl AwsRds {
    /// A provider for `instance`. `program` is the `aws` executable.
    #[must_use]
    pub fn new(
        program: PathBuf,
        instance: String,
        region: Option<String>,
        profile: Option<String>,
    ) -> Self {
        AwsRds {
            cli: AwsCli::new(program, region, profile),
            instance,
            jobs: DEFAULT_JOBS,
            progress: false,
        }
    }

    /// Downloads up to `jobs` files at the same time (at least 1, at most [`MAX_JOBS`]).
    #[must_use]
    pub fn with_jobs(mut self, jobs: usize) -> Self {
        self.jobs = jobs.clamp(1, MAX_JOBS);
        self
    }

    /// Prints one line on stderr as each file finishes.
    #[must_use]
    pub fn with_progress(mut self, progress: bool) -> Self {
        self.progress = progress;
        self
    }

    /// Waits this long (doubling) before retrying a throttled call.
    #[must_use]
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.cli = self.cli.with_retry_delay(delay);
        self
    }

    fn base(&self, service: &str, operation: &str) -> Vec<String> {
        self.cli.base(service, operation)
    }

    fn call(&self, args: &[String], command: &str) -> Result<Vec<u8>, FetchError> {
        self.cli.call(args, command)
    }

    fn list(&self, since: Timestamp) -> Result<Vec<LogFile>, FetchError> {
        let mut args = self.base("rds", "describe-db-log-files");
        args.extend([
            "--db-instance-identifier".to_owned(),
            self.instance.clone(),
            "--file-last-written".to_owned(),
            since.as_millisecond().to_string(),
        ]);
        let out = self.call(&args, "rds describe-db-log-files")?;
        let mut files = serde_json::from_slice::<LogFiles>(&out)
            .map_err(|e| FetchError::Output {
                provider: "aws-rds",
                what: "describe-db-log-files",
                detail: e.to_string(),
            })?
            .describe_db_log_files;
        files.sort_by(|a, b| {
            a.last_written
                .cmp(&b.last_written)
                .then(a.name.cmp(&b.name))
        });
        Ok(files)
    }

    fn portion(&self, file: &str, marker: &str) -> Result<Portion, FetchError> {
        let mut args = self.base("rds", "download-db-log-file-portion");
        args.extend([
            "--db-instance-identifier".to_owned(),
            self.instance.clone(),
            "--log-file-name".to_owned(),
            file.to_owned(),
            "--marker".to_owned(),
            marker.to_owned(),
            "--no-paginate".to_owned(),
        ]);
        let out = self.call(&args, "rds download-db-log-file-portion")?;
        serde_json::from_slice(&out).map_err(|e| FetchError::Output {
            provider: "aws-rds",
            what: "download-db-log-file-portion",
            detail: e.to_string(),
        })
    }
}

fn unreachable_attempts(command: &str) -> Result<Vec<u8>, FetchError> {
    Err(FetchError::Output {
        provider: "aws-rds",
        what: "retry loop",
        detail: format!("{command} gave no result"),
    })
}

impl LogProvider for AwsRds {
    fn name(&self) -> &'static str {
        "aws-rds"
    }

    fn experimental(&self) -> bool {
        false
    }

    fn check(&self) -> Result<(), FetchError> {
        self.cli.check()
    }

    fn fetch(
        &self,
        request: &Request,
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
    ) -> Result<Summary, FetchError> {
        let mut summary = Summary::default();
        let jobs = self.plan(request, sink, state, &mut summary)?;
        self.run(&jobs, sink, state, &mut summary)?;
        Ok(summary)
    }
}

impl AwsRds {
    /// The files that still need bytes, in the order RDS wrote them.
    fn plan(
        &self,
        request: &Request,
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
        summary: &mut Summary,
    ) -> Result<Vec<Job>, FetchError> {
        let mut jobs = Vec::new();
        for file in self.list(request.since)? {
            if ends_before(&file.name, request.since) {
                continue;
            }
            let local = safe_name(&file.name);
            let key = format!("marker:{}", file.name);
            let size = u64::try_from(file.size).unwrap_or(0);
            let size_key = format!("size:{}", file.name);
            // The size RDS reported last time, not the bytes we wrote: those can differ when the
            // log is not valid UTF-8, and would look like a rotation on every run.
            let last_size = state.get(&size_key).and_then(|v| v.parse::<u64>().ok());
            let stored = sink.stored(&local);
            let shrunk = last_size.map_or(stored > size, |last| size < last);
            if shrunk {
                // The instance's copy is smaller than before: it was rotated or recreated.
                sink.reset(&local)?;
                state.remove(&key);
                summary.restarted.push(local.clone());
            }
            if !shrunk && last_size == Some(size) && state.contains_key(&key) {
                continue;
            }
            let marker = state.get(&key).cloned().unwrap_or_else(|| "0".to_owned());
            jobs.push(Job {
                name: file.name,
                local,
                size,
                marker,
            });
        }
        Ok(jobs)
    }

    /// Downloads `jobs` on up to `self.jobs` threads. The calling thread alone writes to the sink
    /// and the state, so a checkpoint always matches the bytes on disk.
    fn run(
        &self,
        jobs: &[Job],
        sink: &mut dyn LogSink,
        state: &mut ProviderState,
        summary: &mut Summary,
    ) -> Result<(), FetchError> {
        let workers = self.jobs.clamp(1, jobs.len().max(1));
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let failed = AtomicUsize::new(usize::MAX);
        let (tx, rx) = mpsc::sync_channel::<Event>(workers * 2);
        let mut wrote = vec![false; jobs.len()];
        let mut finished = 0;
        let mut sink_failed = false;
        let mut failure: Option<(usize, FetchError)> = None;
        thread::scope(|scope| {
            for _ in 0..workers {
                let tx = tx.clone();
                let (next, stop, failed) = (&next, &stop, &failed);
                scope.spawn(move || {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(index) else { break };
                        let halt = Halt { stop, failed };
                        if halt.stops(index) {
                            break;
                        }
                        let started = Instant::now();
                        match self.download(index, job, &tx, &halt) {
                            Ok(true) => {
                                let took = started.elapsed();
                                if tx.send(Event::Done { job: index, took }).is_err() {
                                    break;
                                }
                            }
                            Ok(false) => break,
                            Err(error) => {
                                failed.fetch_min(index, Ordering::Relaxed);
                                drop(tx.send(Event::Failed { job: index, error }));
                                break;
                            }
                        }
                    }
                });
            }
            drop(tx);
            for event in rx {
                match event {
                    Event::Data { job, text, marker } => {
                        if sink_failed {
                            continue;
                        }
                        let spec = &jobs[job];
                        if !text.is_empty() {
                            if let Err(error) = sink.append(&spec.local, text.as_bytes()) {
                                stop.store(true, Ordering::Relaxed);
                                sink_failed = true;
                                keep_first(&mut failure, job, error);
                                continue;
                            }
                            summary.bytes += text.len() as u64;
                            wrote[job] = true;
                        }
                        state.insert(format!("marker:{}", spec.name), marker);
                    }
                    Event::Done { job, took } => {
                        let spec = &jobs[job];
                        state.insert(format!("size:{}", spec.name), spec.size.to_string());
                        if sink_failed {
                            continue;
                        }
                        if let Err(error) = sink.checkpoint(state) {
                            stop.store(true, Ordering::Relaxed);
                            sink_failed = true;
                            keep_first(&mut failure, job, error);
                            continue;
                        }
                        finished += 1;
                        if self.progress {
                            say(&format!(
                                "[{finished}/{}] {}  {}  {}",
                                jobs.len(),
                                spec.name,
                                human_bytes(spec.size),
                                human_time(took)
                            ));
                        }
                    }
                    Event::Failed { job, error } => keep_first(&mut failure, job, error),
                }
            }
        });
        for (job, wrote) in jobs.iter().zip(wrote) {
            if wrote && !summary.files.contains(&job.local) {
                summary.files.push(job.local.clone());
            }
        }
        failure.map_or(Ok(()), |(_, error)| Err(error))
    }

    /// Downloads one file portion by portion. `Ok(false)` means it stopped early because another
    /// file failed.
    fn download(
        &self,
        index: usize,
        job: &Job,
        tx: &SyncSender<Event>,
        halt: &Halt<'_>,
    ) -> Result<bool, FetchError> {
        let mut marker = job.marker.clone();
        for _ in 0..MAX_PORTIONS {
            if halt.stops(index) {
                return Ok(false);
            }
            let portion = self.portion(&job.name, &marker)?;
            let text = portion.log_file_data.unwrap_or_default();
            let next = portion.marker.unwrap_or_else(|| marker.clone());
            if portion.additional_data_pending && next == marker {
                // More data is promised but the marker stands still: stop before repeating text.
                return Err(FetchError::Output {
                    provider: "aws-rds",
                    what: "download-db-log-file-portion",
                    detail: format!("the marker {marker:?} does not advance for {}", job.name),
                });
            }
            marker = next;
            let event = Event::Data {
                job: index,
                text,
                marker: marker.clone(),
            };
            if tx.send(event).is_err() {
                return Ok(false);
            }
            if !portion.additional_data_pending {
                break;
            }
        }
        Ok(true)
    }
}

/// Keeps the failure of the file that comes first in the list.
fn keep_first(slot: &mut Option<(usize, FetchError)>, job: usize, error: FetchError) {
    if slot.as_ref().is_none_or(|(first, _)| job < *first) {
        *slot = Some((job, error));
    }
}

/// A line of progress on stderr; there is nowhere to report a failure to write it.
fn say(line: &str) {
    drop(writeln!(std::io::stderr(), "{line}"));
}

#[expect(clippy::cast_precision_loss, reason = "a display figure")]
fn human_bytes(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / f64::from(1_u32 << 20))
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

fn human_time(took: Duration) -> String {
    let secs = took.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// Whether the hour in a name like `error/postgresql.log.2026-10-16-20` ended at or before
/// `since`. RDS closes a file when the hour turns, so its `LastWritten` falls just after the end
/// of that hour and the file would be listed although it holds only older lines. A name without
/// an hour is never skipped here.
fn ends_before(name: &str, since: Timestamp) -> bool {
    let Some((_, stamp)) = name.rsplit_once('.') else {
        return false;
    };
    let mut parts = stamp.split('-');
    let (Some(y), Some(mo), Some(d), Some(h), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    if y.len() != 4 || [mo, d, h].iter().any(|p| p.len() != 2) {
        return false;
    }
    format!("{y}-{mo}-{d}T{h}:00:00Z")
        .parse::<Timestamp>()
        .ok()
        .and_then(|start| start.checked_add(jiff::SignedDuration::from_hours(1)).ok())
        .is_some_and(|end| end <= since)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_names_become_safe_local_ones() {
        assert_eq!(
            safe_name("error/postgresql.log.2026-10-14-10"),
            "error_postgresql.log.2026-10-14-10"
        );
        assert_eq!(safe_name("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(safe_name(".."), "_..");
        assert_eq!(safe_name(""), "_");
        assert!(!safe_name("a/b\\c").contains(['/', '\\']));
    }

    #[test]
    fn failures_get_the_fix_that_matches() {
        assert!(explain("Unable to locate credentials. You can configure credentials by running \"aws configure\".").contains("aws configure"));
        assert!(explain("An error occurred (ExpiredToken) when calling").contains("sign in"));
        assert!(explain("An error occurred (AccessDenied)").contains("rds:DescribeDBLogFiles"));
        assert!(explain("An error occurred (DBInstanceNotFound)").contains("--instance"));
        assert_eq!(explain("something else"), "");
        assert!(is_throttle(
            "An error occurred (Throttling) when calling the X operation: Rate exceeded"
        ));
    }

    #[test]
    fn a_file_whose_hour_ended_before_since_is_skipped() {
        let since = "2026-10-16T21:00:00Z".parse::<Timestamp>().unwrap();
        assert!(ends_before("error/postgresql.log.2026-10-16-20", since));
        assert!(ends_before("error/postgresql.log.2026-10-15-23", since));
        assert!(!ends_before("error/postgresql.log.2026-10-16-21", since));
        let mid = "2026-10-16T21:30:00Z".parse::<Timestamp>().unwrap();
        assert!(!ends_before("error/postgresql.log.2026-10-16-21", mid));
        assert!(!ends_before("error/postgresql.log", since));
        assert!(!ends_before("error/postgresql.log.2026-10-16", since));
        assert!(!ends_before("error/postgresql.log.20261016-20", since));
        assert!(!ends_before("error/postgresql.log.2026-13-16-20", since));
    }
}
