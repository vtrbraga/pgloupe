//! `bench verify`: pgloupe's report against the server's own counters.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use anyhow::{Context as _, bail};
use serde_json::Value;

use super::{pgbench::TPCB_STATEMENTS, truth::Truth};
use crate::cli::BenchVerifyArgs;

/// Allowed relative gap between statements in the log and statements pgbench says it sent.
const STATEMENT_TOLERANCE: f64 = 0.01;

/// Errors the log may differ from the workers' count by (a statement cut off at the end).
const ERROR_SLACK: u64 = 2;

/// Rollbacks the server counter may differ from the workers' count by, besides 1%.
const ROLLBACK_SLACK: u64 = 5;

/// Allowed relative gap between pgbench's transactions and the server's commit counter.
const COMMIT_TOLERANCE: f64 = 0.005;

/// One comparison and its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Short name.
    pub name: String,
    /// Whether it held.
    pub ok: bool,
    /// The numbers behind it.
    pub detail: String,
}

/// pgloupe reports (JSON) of one run.
#[derive(Debug, Default)]
pub struct Reports {
    /// Whole log, one report per format.
    pub per_format: BTreeMap<String, Value>,
    /// Snapshots `first` to `after_reset`: the window that contains the reset.
    pub window_reset: Option<Value>,
    /// Snapshots `after_reset` to `last`: the window where evictions are visible.
    pub window_evictions: Option<Value>,
    /// The stderr log restricted to the two pgbench applications.
    pub pgbench_only: Option<Value>,
}

fn skipped(report: &Value) -> Option<u64> {
    report.pointer("/logs/diagnostics/skipped")?.as_u64()
}

fn calls(report: &Value) -> Option<u64> {
    let kinds = report.pointer("/logs/kinds")?.as_array()?;
    kinds.iter().map(|k| k.get("calls")?.as_u64()).sum()
}

fn number(report: &Value, pointer: &str) -> Option<u64> {
    report.pointer(pointer)?.as_u64()
}

/// Errors (level `ERROR`) in the report, summed over message kinds.
fn error_count(report: &Value) -> Option<u64> {
    report
        .pointer("/logs/errors")?
        .as_array()?
        .iter()
        .filter(|row| row.get("level").and_then(Value::as_str) == Some("ERROR"))
        .map(|row| row.get("count")?.as_u64())
        .sum()
}

/// The report the single-valued checks read: stderr if present, else the first format.
fn reference(reports: &Reports) -> Option<&Value> {
    reports
        .per_format
        .get("stderr")
        .or_else(|| reports.per_format.values().next())
}

fn finding_codes(report: &Value) -> Vec<&str> {
    report
        .get("findings")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r.get("code").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "counts far below 2^52; only a ratio is computed"
)]
fn within(actual: u64, expected: u64, tolerance: f64) -> bool {
    if expected == 0 {
        return actual == 0;
    }
    (actual as f64 - expected as f64).abs() / expected as f64 <= tolerance
}

fn check(name: &str, ok: bool, detail: String) -> Check {
    Check {
        name: name.to_owned(),
        ok,
        detail,
    }
}

/// Compares `reports` with `truth`.
#[must_use]
pub fn evaluate(truth: &Truth, reports: &Reports) -> Vec<Check> {
    let mut out = Vec::new();

    for (format, report) in &reports.per_format {
        let n = skipped(report);
        out.push(check(
            &format!("skipped lines ({format})"),
            n == Some(0),
            format!("{n:?}"),
        ));
    }

    let counts: BTreeMap<&str, Option<u64>> = reports
        .per_format
        .iter()
        .map(|(f, r)| (f.as_str(), calls(r)))
        .collect();
    let mut values = counts.values();
    let agree = values
        .next()
        .is_some_and(|first| first.is_some() && values.all(|v| v == first));
    out.push(check(
        "statement calls agree across formats",
        agree,
        format!("{counts:?}"),
    ));

    let expected =
        truth.transactions_of("tpcb") * TPCB_STATEMENTS + truth.transactions_of("literal");
    let actual = reports
        .pgbench_only
        .as_ref()
        .or_else(|| reference(reports))
        .and_then(calls);
    out.push(check(
        "statement calls match pgbench",
        actual.is_some_and(|a| within(a, expected, STATEMENT_TOLERANCE)),
        format!("log {actual:?}, pgbench {expected}"),
    ));

    let locks_committed = truth.event("locks", "transactions").saturating_sub(
        truth.event("locks", "explicit_rollbacks") + truth.event("locks", "deadlocks"),
    );
    let committed = truth.transactions() + locks_committed;
    let commits = u64::try_from(truth.delta.commits).unwrap_or(0);
    out.push(check(
        "server commits match the workers",
        within(commits, committed, COMMIT_TOLERANCE),
        format!("xact_commit {commits}, workers {committed}"),
    ));
    out.extend(worker_checks(truth, reports));

    let reset_seen = reports
        .window_reset
        .as_ref()
        .is_some_and(|r| finding_codes(r).contains(&"stats_reset"));
    out.push(check(
        "reset is reported",
        truth.snapshots.after_reset.is_some() && reset_seen,
        format!("reset snapshot {:?}", truth.snapshots.after_reset),
    ));

    let evictions_seen = reports
        .window_evictions
        .as_ref()
        .is_some_and(|r| finding_codes(r).contains(&"stats_evictions"));
    out.push(check(
        "evictions are reported",
        truth.dealloc_at_end > 0 && evictions_seen,
        format!("dealloc at end {}", truth.dealloc_at_end),
    ));
    out
}

/// Checks that tie the lock, temp file, error and connection workers to the server's counters.
fn worker_checks(truth: &Truth, reports: &Reports) -> Vec<Check> {
    let mut out = Vec::new();
    let Some(log) = reference(reports) else {
        return out;
    };
    let server_deadlocks = u64::try_from(truth.delta.deadlocks).unwrap_or(0);
    let seen = truth.event("locks", "deadlocks");
    let logged = number(log, "/logs/locks/deadlocks");
    out.push(check(
        "deadlocks: log, server and sessions agree",
        logged == Some(server_deadlocks) && seen == server_deadlocks,
        format!("log {logged:?}, server {server_deadlocks}, sessions {seen}"),
    ));
    let waits = number(log, "/logs/locks/waits");
    out.push(check(
        "lock waits are logged",
        server_deadlocks == 0 || waits.is_some_and(|w| w > 0),
        format!("waits {waits:?}"),
    ));

    let temp_files = u64::try_from(truth.delta.temp_files).unwrap_or(0);
    let temp_bytes = u64::try_from(truth.delta.temp_bytes).unwrap_or(0);
    let (files, bytes) = (
        number(log, "/logs/temp_files/files"),
        number(log, "/logs/temp_files/bytes"),
    );
    out.push(check(
        "temp files match the server",
        files == Some(temp_files) && bytes == Some(temp_bytes),
        format!(
            "log {files:?} files {bytes:?} bytes, server {temp_files} files {temp_bytes} bytes"
        ),
    ));

    let injected = truth.event("errors", "errors") + seen;
    let logged_errors = error_count(log);
    out.push(check(
        "errors match the workers",
        logged_errors.is_some_and(|n| n.abs_diff(injected) <= ERROR_SLACK),
        format!("log {logged_errors:?}, workers {injected}"),
    ));

    let rollbacks = u64::try_from(truth.delta.rollbacks).unwrap_or(0);
    let expected = truth.event("locks", "explicit_rollbacks") + injected;
    out.push(check(
        "server rollbacks match the workers",
        rollbacks.abs_diff(expected) <= ROLLBACK_SLACK.max(expected / 100),
        format!("xact_rollback {rollbacks}, workers {expected}"),
    ));

    let connects = truth.event("churn", "connects") + truth.event("churn", "abrupt");
    let authorized = number(log, "/logs/connections/authorized");
    out.push(check(
        "connections are logged",
        authorized.is_some_and(|n| n >= connects),
        format!("authorized {authorized:?}, churn {connects}"),
    ));
    let reasons = log
        .pointer("/logs/disconnects/reasons")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    out.push(check(
        "abrupt hang-ups are reported",
        truth.event("churn", "abrupt") == 0 || reasons > 0,
        format!("{reasons} disconnect reasons"),
    ));
    out
}

/// Runs `pgloupe report … --output json` and parses its output.
fn report(pgloupe: &Path, files: &[PathBuf], extra: &[String]) -> anyhow::Result<Value> {
    let output = Command::new(pgloupe)
        .arg("report")
        .args(files)
        .args(["--output", "json"])
        .args(extra)
        .output()
        .with_context(|| format!("running {}", pgloupe.display()))?;
    if !output.status.success() {
        bail!(
            "pgloupe failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout).context("pgloupe printed invalid JSON")
}

/// `log`, `csv` or `json` for `postgresql-….log`, `….csv.gz` and so on.
fn format_of(path: &Path) -> Option<&str> {
    let plain = if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        Path::new(path.file_stem()?)
    } else {
        path
    };
    plain.extension()?.to_str()
}

/// Log files with `ext`, in name (= time) order.
fn files_with(dir: &Path, ext: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| format_of(p) == Some(ext))
        .collect();
    files.sort();
    Ok(files)
}

/// Snapshot flags for a window between snapshot numbers `before` and `after`.
fn window_args(snaps: &Path, before: u8, after: u8) -> Vec<String> {
    let path = |stem: &str, n: u8| snaps.join(format!("{stem}_{n}.csv")).display().to_string();
    vec![
        "--before".to_owned(),
        path("pgss", before),
        "--after".to_owned(),
        path("pgss", after),
        "--before-info".to_owned(),
        path("pgss_info", before),
        "--after-info".to_owned(),
        path("pgss_info", after),
        "--before-db".to_owned(),
        path("pg_stat_database", before),
        "--after-db".to_owned(),
        path("pg_stat_database", after),
    ]
}

/// One timed run: wall seconds, peak resident memory in MiB, and output sizes in KiB.
struct Timing {
    threads: u32,
    secs: f64,
    rss_mib: u64,
    html_kib: u64,
    json_kib: u64,
}

fn time_run(pgloupe: &Path, files: &[PathBuf], dir: &Path, threads: u32) -> anyhow::Result<Timing> {
    let prefix = dir.join(format!("verify-report-{threads}"));
    let started = Instant::now();
    let output = Command::new("/usr/bin/time")
        .arg("-v")
        .arg(pgloupe)
        .arg("report")
        .args(files)
        .args([
            "--output",
            "html,json",
            "--threads",
            &threads.to_string(),
            "--out",
        ])
        .arg(&prefix)
        .output()
        .context("running /usr/bin/time")?;
    let secs = started.elapsed().as_secs_f64();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "timed pgloupe run failed ({}): {}",
            output.status,
            stderr.trim()
        );
    }
    let rss_kib: u64 = stderr
        .lines()
        .find_map(|l| l.trim().strip_prefix("Maximum resident set size (kbytes):"))
        .and_then(|v| v.trim().parse().ok())
        .context("no peak memory line in /usr/bin/time output")?;
    let size_of = |ext: &str| -> u64 {
        fs::metadata(prefix.with_extension(ext)).map_or(0, |m| m.len() >> 10)
    };
    Ok(Timing {
        threads,
        secs,
        rss_mib: rss_kib >> 10,
        html_kib: size_of("html"),
        json_kib: size_of("json"),
    })
}

/// Runs everything and prints the result.
///
/// Returns whether every check held.
///
/// # Errors
///
/// Fails if the run directory is incomplete or pgloupe cannot be run.
#[expect(clippy::print_stdout, reason = "the table is the output")]
pub fn run(args: &BenchVerifyArgs) -> anyhow::Result<bool> {
    let truth = Truth::read(&args.run_dir)?;
    let log_dir = args.run_dir.join("log");
    let snaps = args.run_dir.join("snapshots");

    let mut reports = Reports::default();
    for (format, ext) in [("stderr", "log"), ("csv", "csv"), ("json", "json")] {
        let files = files_with(&log_dir, ext)?;
        if files.is_empty() {
            continue;
        }
        reports
            .per_format
            .insert(format.to_owned(), report(&args.pgloupe, &files, &[])?);
    }
    let stderr_files = files_with(&log_dir, "log")?;
    let only = [
        "--application",
        "bench:tpcb",
        "--application",
        "bench:literal",
    ]
    .map(str::to_owned);
    reports.pgbench_only = Some(report(&args.pgloupe, &stderr_files, &only)?);
    if let Some(after_reset) = truth.snapshots.after_reset {
        let a = window_args(&snaps, truth.snapshots.first, after_reset);
        reports.window_reset = Some(report(&args.pgloupe, &stderr_files, &a)?);
        let b = window_args(&snaps, after_reset, truth.snapshots.last);
        reports.window_evictions = Some(report(&args.pgloupe, &stderr_files, &b)?);
    }

    let checks = evaluate(&truth, &reports);
    for c in &checks {
        println!(
            "{}  {}  ({})",
            if c.ok { "ok  " } else { "FAIL" },
            c.name,
            c.detail
        );
    }

    println!("\nthreads  seconds  MB/s  peak MiB  html KiB  json KiB");
    // The timed runs read the stderr files.
    let stderr_bytes = truth.formats.get("log").map_or(0, |f| f.bytes);
    let mib = f64::from(u32::try_from(stderr_bytes >> 20).unwrap_or(u32::MAX));
    for threads in &args.threads {
        let t = time_run(&args.pgloupe, &stderr_files, &args.run_dir, *threads)?;
        println!(
            "{:>7}  {:>7.1}  {:>4.0}  {:>8}  {:>8}  {:>8}",
            t.threads,
            t.secs,
            mib / t.secs,
            t.rss_mib,
            t.html_kib,
            t.json_kib
        );
    }
    Ok(checks.iter().all(|c| c.ok))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::bench::truth::{Counters, Snapshots, WorkerStats};

    fn truth() -> Truth {
        Truth {
            pg_major: 17,
            seed: 42,
            scale: 20,
            target_bytes: 1 << 30,
            reached_bytes: 1 << 30,
            formats: BTreeMap::new(),
            wall_secs: 60,
            delta: Counters {
                commits: 1_045,
                rollbacks: 15,
                deadlocks: 3,
                temp_files: 4,
                temp_bytes: 4_096,
                ..Counters::default()
            },
            dealloc_at_end: 4,
            workers: vec![
                WorkerStats {
                    name: "tpcb".to_owned(),
                    rounds: 1,
                    transactions: 100,
                    events: BTreeMap::new(),
                },
                WorkerStats {
                    name: "literal".to_owned(),
                    rounds: 1,
                    transactions: 900,
                    events: BTreeMap::new(),
                },
                worker(
                    "locks",
                    &[
                        ("transactions", 50),
                        ("explicit_rollbacks", 2),
                        ("deadlocks", 3),
                    ],
                ),
                worker("errors", &[("errors", 10)]),
                worker("churn", &[("connects", 8), ("abrupt", 2)]),
            ],
            snapshots: Snapshots {
                first: 0,
                after_reset: Some(2),
                last: 3,
            },
            gzipped: Vec::new(),
        }
    }

    fn worker(name: &str, events: &[(&str, u64)]) -> WorkerStats {
        WorkerStats {
            name: name.to_owned(),
            rounds: 0,
            transactions: 0,
            events: events.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
        }
    }

    /// 100 × 7 + 900 = 1600 statements.
    fn report_with(calls: u64, skipped: u64, advice: &[&str]) -> Value {
        json!({
            "findings": advice.iter().map(|c| json!({"code": c})).collect::<Vec<_>>(),
            "logs": {
                "diagnostics": {"skipped": skipped},
                "locks": {"deadlocks": 3, "waits": 7},
                "temp_files": {"files": 4, "bytes": 4096},
                "errors": [{"level": "ERROR", "count": 13}, {"level": "FATAL", "count": 5}],
                "connections": {"authorized": 20},
                "disconnects": {"reasons": [{"reason": "client_gone", "count": 2}]},
                "kinds": [{"calls": calls / 2}, {"calls": calls - calls / 2}],
            }
        })
    }

    fn good() -> Reports {
        Reports {
            per_format: BTreeMap::from([
                ("stderr".to_owned(), report_with(1_600, 0, &[])),
                ("csv".to_owned(), report_with(1_600, 0, &[])),
                ("json".to_owned(), report_with(1_600, 0, &[])),
            ]),
            window_reset: Some(report_with(1, 0, &["stats_reset"])),
            window_evictions: Some(report_with(1, 0, &["stats_evictions"])),
            pgbench_only: Some(report_with(1_600, 0, &[])),
        }
    }

    fn failed(truth: &Truth, reports: &Reports) -> Vec<String> {
        evaluate(truth, reports)
            .into_iter()
            .filter(|c| !c.ok)
            .map(|c| c.name)
            .collect()
    }

    #[test]
    fn a_consistent_run_passes() {
        assert_eq!(failed(&truth(), &good()), Vec::<String>::new());
    }

    #[test]
    fn a_format_with_other_counts_fails() {
        let mut r = good();
        r.per_format
            .insert("csv".to_owned(), report_with(1_599, 0, &[]));
        assert_eq!(
            failed(&truth(), &r),
            ["statement calls agree across formats"]
        );
    }

    #[test]
    fn skipped_lines_fail() {
        let mut r = good();
        r.per_format
            .insert("json".to_owned(), report_with(1_600, 3, &[]));
        assert_eq!(failed(&truth(), &r), ["skipped lines (json)"]);
    }

    #[test]
    fn statements_far_from_pgbench_fail() {
        let mut t = truth();
        t.workers[1].transactions = 1_000;
        assert_eq!(
            failed(&t, &good()),
            [
                "statement calls match pgbench",
                "server commits match the workers"
            ]
        );
    }

    #[test]
    fn commits_far_from_pgbench_fail() {
        let mut t = truth();
        t.delta.commits = 1_200;
        assert_eq!(failed(&t, &good()), ["server commits match the workers"]);
    }

    #[test]
    fn a_missing_reset_fails() {
        let mut r = good();
        r.window_reset = Some(report_with(1, 0, &[]));
        assert_eq!(failed(&truth(), &r), ["reset is reported"]);
    }

    #[test]
    fn missing_evictions_fail() {
        let mut r = good();
        r.window_evictions = Some(report_with(1, 0, &[]));
        assert_eq!(failed(&truth(), &r), ["evictions are reported"]);
    }

    fn mutated(f: impl FnOnce(&mut Truth)) -> Vec<String> {
        let mut t = truth();
        f(&mut t);
        failed(&t, &good())
    }

    #[test]
    fn a_deadlock_the_log_does_not_show_fails() {
        let failures = mutated(|t| t.delta.deadlocks = 4);
        assert!(failures.contains(&"deadlocks: log, server and sessions agree".to_owned()));
    }

    #[test]
    fn temp_files_the_log_does_not_show_fail() {
        assert_eq!(
            mutated(|t| t.delta.temp_files = 5),
            ["temp files match the server"]
        );
    }

    #[test]
    fn errors_the_log_does_not_show_fail() {
        let failures = mutated(|t| {
            t.workers[3].events.insert("errors".to_owned(), 20);
        });
        assert!(failures.contains(&"errors match the workers".to_owned()));
    }

    #[test]
    fn rollbacks_far_from_the_workers_fail() {
        assert_eq!(
            mutated(|t| t.delta.rollbacks = 40),
            ["server rollbacks match the workers"]
        );
    }

    #[test]
    fn fewer_logged_connections_than_made_fail() {
        assert_eq!(
            mutated(|t| {
                t.workers[4].events.insert("connects".to_owned(), 30);
            }),
            ["connections are logged"]
        );
    }

    #[test]
    fn gzip_files_count_as_their_format() {
        assert_eq!(format_of(Path::new("postgresql-1.log")), Some("log"));
        assert_eq!(format_of(Path::new("postgresql-1.csv.gz")), Some("csv"));
        assert_eq!(format_of(Path::new("postgresql-1.json.gz")), Some("json"));
        assert_eq!(format_of(Path::new("gz")), None);
    }

    #[test]
    fn window_flags_name_every_snapshot_file() {
        let args = window_args(Path::new("/s"), 0, 3).join(" ");
        assert!(args.contains("--before /s/pgss_0.csv"), "{args}");
        assert!(args.contains("--after-info /s/pgss_info_3.csv"), "{args}");
        assert!(
            args.contains("--after-db /s/pg_stat_database_3.csv"),
            "{args}"
        );
    }
}
