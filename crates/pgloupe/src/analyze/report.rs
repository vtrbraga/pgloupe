//! The versioned report: statistics shaped for display and for JSON consumers.

use std::{cmp::Reverse, collections::BTreeMap};

use jiff::Timestamp;
use serde::Serialize;

use super::{
    acc::{Accumulators, QueryStats, to_micros},
    diagnostics::{self, Diagnostics},
    pipeline::{FileSummary, Output, PrefixSource},
    sketch::Sketch,
};
use crate::log::Format;

/// Version of the JSON layout; bump when a field changes meaning.
const SCHEMA_VERSION: u32 = 1;
/// Tables listed in the autovacuum section.
const TOP_TABLES: usize = 10;

/// What orders the statement table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Total,
    Calls,
    P95,
    Max,
}

/// How much of the statement table to keep.
#[derive(Debug, Clone, Copy)]
pub struct ReportOptions {
    pub by: SortBy,
    pub top: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub files: Vec<FileReport>,
    pub window: Window,
    pub diagnostics: Diagnostics,
    pub overview: Overview,
    /// What ranks `statements`: `total`, `calls`, `p95` or `max`.
    pub statements_by: &'static str,
    pub statements: Vec<StatementRow>,
    pub errors: Vec<ErrorRow>,
    pub locks: LocksReport,
    pub temp_files: TempReport,
    pub connections: ConnectionsReport,
    pub checkpoints: Vec<CheckpointRow>,
    pub autovacuum: Vec<TableRow>,
    pub timeline: TimelineReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileReport {
    pub path: String,
    pub bytes: u64,
    /// `csv`, `json` or `stderr`.
    pub format: &'static str,
    pub prefix: Option<String>,
    /// `given`, `inferred` or `n/a`.
    pub prefix_source: &'static str,
    /// Percent of the inference sample the inferred prefix matched.
    pub prefix_match: Option<f64>,
    pub lines: u64,
    pub records: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Window {
    pub first: Option<String>,
    pub last: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub records: u64,
    pub events: BTreeMap<String, u64>,
    pub levels: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatementRow {
    /// 16 hex digits; `0000000000000000` is the overflow row.
    pub fingerprint: String,
    /// The `query_id` the log carried, as text (64-bit ids do not survive JSON numbers).
    pub query_id: Option<String>,
    pub text: String,
    pub parsed: bool,
    pub calls: u64,
    pub timed_calls: u64,
    pub total_ms: f64,
    pub mean_ms: Option<f64>,
    pub min_ms: Option<f64>,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub p99_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub share_pct: f64,
    pub errors: u64,
    pub lock_waits: u64,
    pub lock_wait_ms: f64,
    pub temp_files: u64,
    pub temp_bytes: u64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorRow {
    pub level: String,
    pub sqlstate: Option<String>,
    pub count: u64,
    pub first_message: String,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocksReport {
    pub waits: u64,
    pub acquired: u64,
    pub total_wait_ms: f64,
    pub max_wait_ms: f64,
    pub deadlocks: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TempReport {
    pub files: u64,
    pub bytes: u64,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Quantiles {
    pub count: u64,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionsReport {
    pub received: u64,
    pub authenticated: u64,
    pub authorized: u64,
    pub disconnections: u64,
    pub session_ms: Quantiles,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointRow {
    /// `checkpoint` or `restartpoint`.
    pub kind: &'static str,
    pub started: u64,
    pub completed: u64,
    pub reasons: BTreeMap<String, u64>,
    pub buffers: u64,
    pub write_ms: Quantiles,
    pub sync_ms: Quantiles,
    pub total_ms: Quantiles,
}

#[derive(Debug, Clone, Serialize)]
pub struct TableRow {
    pub table: String,
    pub vacuums: u64,
    pub analyzes: u64,
    pub total_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineRow {
    pub start: String,
    pub statements: u64,
    pub errors: u64,
    pub connections: u64,
    pub lock_waits: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineReport {
    pub bucket_secs: i64,
    pub buckets: Vec<TimelineRow>,
}

#[expect(
    clippy::cast_precision_loss,
    reason = "microsecond sums stay far below 2^53"
)]
fn ms(us: u64) -> f64 {
    us as f64 / 1_000.0
}

#[expect(clippy::cast_precision_loss, reason = "counts stay far below 2^53")]
fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn time(ts: Option<Timestamp>) -> Option<String> {
    ts.map(|t| t.to_string())
}

fn quantiles(sketch: &Sketch) -> Quantiles {
    Quantiles {
        count: sketch.count(),
        p50: sketch.quantile(0.5),
        p95: sketch.quantile(0.95),
        max: sketch.max(),
    }
}

impl Report {
    /// Shapes the statistics of a finished run.
    #[must_use]
    pub fn build(output: &Output, options: &ReportOptions) -> Report {
        let acc = &output.acc;
        let totals = &acc.totals;
        Report {
            schema_version: SCHEMA_VERSION,
            files: output.files.iter().map(file_report).collect(),
            window: Window {
                first: time(totals.first),
                last: time(totals.last),
            },
            diagnostics: diagnostics::build(output),
            overview: Overview {
                records: totals.records,
                events: totals.events.clone(),
                levels: totals.levels.clone(),
            },
            statements_by: match options.by {
                SortBy::Total => "total",
                SortBy::Calls => "calls",
                SortBy::P95 => "p95",
                SortBy::Max => "max",
            },
            statements: statements(acc, options),
            errors: errors(acc),
            locks: LocksReport {
                waits: totals.locks.waits,
                acquired: totals.locks.acquired,
                total_wait_ms: ms(totals.locks.wait_total_us),
                max_wait_ms: ms(totals.locks.wait_max_us),
                deadlocks: totals.locks.deadlocks,
            },
            temp_files: TempReport {
                files: totals.temp.files,
                bytes: totals.temp.bytes,
                max_bytes: totals.temp.max_bytes,
            },
            connections: ConnectionsReport {
                received: totals.connections.received,
                authenticated: totals.connections.authenticated,
                authorized: totals.connections.authorized,
                disconnections: totals.connections.disconnections,
                session_ms: quantiles(&totals.connections.session),
            },
            checkpoints: checkpoints(acc),
            autovacuum: autovacuum(acc),
            timeline: timeline(acc),
        }
    }

    /// The report as pretty-printed JSON (no trailing newline).
    ///
    /// # Errors
    ///
    /// Fails only if serialization fails, which these types do not produce in practice.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

fn file_report(file: &FileSummary) -> FileReport {
    let (prefix_source, prefix_match) = match &file.prefix_source {
        PrefixSource::Given => ("given", None),
        PrefixSource::Inferred { matched, sampled } => (
            "inferred",
            (*sampled > 0).then(|| percent(*matched as u64, *sampled as u64)),
        ),
        PrefixSource::NotApplicable => ("n/a", None),
    };
    FileReport {
        path: file.path.display().to_string(),
        bytes: file.bytes,
        format: match file.format {
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Stderr => "stderr",
        },
        prefix: file.prefix.clone(),
        prefix_source,
        prefix_match,
        lines: file.stats.lines,
        records: file.stats.records,
        skipped: file.stats.skipped,
    }
}

fn sort_key(query: &QueryStats, by: SortBy) -> u64 {
    match by {
        SortBy::Total => query.total_us,
        SortBy::Calls => query.calls,
        SortBy::P95 => query.sketch.quantile(0.95).map_or(0, to_micros),
        SortBy::Max => query.max_us,
    }
}

fn statements(acc: &Accumulators, options: &ReportOptions) -> Vec<StatementRow> {
    let overflow = &acc.overflow;
    let mut rows: Vec<&QueryStats> = acc.queries.values().collect();
    if overflow.calls > 0
        || overflow.errors > 0
        || overflow.lock_waits > 0
        || overflow.temp_files > 0
    {
        rows.push(overflow);
    }
    let total_us: u64 = rows.iter().map(|query| query.total_us).sum();
    rows.sort_by(|a, b| {
        sort_key(b, options.by)
            .cmp(&sort_key(a, options.by))
            .then(a.id.cmp(&b.id))
    });
    rows.truncate(options.top);
    rows.into_iter()
        .map(|query| statement_row(query, total_us))
        .collect()
}

fn statement_row(query: &QueryStats, total_us: u64) -> StatementRow {
    StatementRow {
        fingerprint: format!("{:016x}", query.id),
        query_id: query.query_id.map(|id| id.to_string()),
        text: query.text.clone(),
        parsed: query.parsed,
        calls: query.calls,
        timed_calls: query.timed,
        total_ms: ms(query.total_us),
        mean_ms: query.mean_ms(),
        min_ms: query.min_us.map(ms),
        p50_ms: query.sketch.quantile(0.5),
        p95_ms: query.sketch.quantile(0.95),
        p99_ms: query.sketch.quantile(0.99),
        max_ms: (query.timed > 0).then(|| ms(query.max_us)),
        share_pct: percent(query.total_us, total_us),
        errors: query.errors,
        lock_waits: query.lock_waits,
        lock_wait_ms: ms(query.lock_wait_us),
        temp_files: query.temp_files,
        temp_bytes: query.temp_bytes,
        first_seen: time(query.first_seen),
        last_seen: time(query.last_seen),
    }
}

fn errors(acc: &Accumulators) -> Vec<ErrorRow> {
    let mut rows: Vec<ErrorRow> = acc
        .totals
        .errors
        .iter()
        .map(|((level, sqlstate), stat)| ErrorRow {
            level: level.clone(),
            sqlstate: sqlstate.clone(),
            count: stat.count,
            first_message: stat.first_message.clone(),
            first_seen: stat.first_seen.to_string(),
            last_seen: stat.last_seen.to_string(),
        })
        .collect();
    rows.sort_by(|a, b| {
        Reverse(a.count)
            .cmp(&Reverse(b.count))
            .then_with(|| a.level.cmp(&b.level))
            .then_with(|| a.sqlstate.cmp(&b.sqlstate))
    });
    rows
}

fn checkpoints(acc: &Accumulators) -> Vec<CheckpointRow> {
    [(false, "checkpoint"), (true, "restartpoint")]
        .into_iter()
        .filter_map(|(restart, kind)| {
            acc.totals
                .checkpoints
                .get(&restart)
                .map(|stats| CheckpointRow {
                    kind,
                    started: stats.started,
                    completed: stats.done,
                    reasons: stats.reasons.clone(),
                    buffers: stats.buffers,
                    write_ms: quantiles(&stats.write),
                    sync_ms: quantiles(&stats.sync),
                    total_ms: quantiles(&stats.total),
                })
        })
        .collect()
}

fn autovacuum(acc: &Accumulators) -> Vec<TableRow> {
    let mut tables: Vec<(&String, _)> = acc.totals.autovacuum.iter().collect();
    tables.sort_by(|a, b| {
        Reverse(a.1.total_us)
            .cmp(&Reverse(b.1.total_us))
            .then(a.0.cmp(b.0))
    });
    tables
        .into_iter()
        .take(TOP_TABLES)
        .map(|(table, stats)| TableRow {
            table: table.clone(),
            vacuums: stats.vacuums,
            analyzes: stats.analyzes,
            total_ms: ms(stats.total_us),
            max_ms: ms(stats.max_us),
        })
        .collect()
}

fn timeline(acc: &Accumulators) -> TimelineReport {
    let timeline = &acc.totals.timeline;
    TimelineReport {
        bucket_secs: timeline.bucket_secs,
        buckets: timeline
            .buckets
            .iter()
            .map(|(start, bucket)| TimelineRow {
                start: Timestamp::from_second(*start)
                    .map_or_else(|_err| start.to_string(), |ts| ts.to_string()),
                statements: bucket.statements,
                errors: bucket.errors,
                connections: bucket.connections,
                lock_waits: bucket.lock_waits,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{
        log::{DecodeStats, LogRecord, Severity},
        testutil::approx_eq,
    };

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(1_760_000_000 + second).unwrap()
    }

    fn stmt(second: i64, ms: f64, sql: &str) -> LogRecord {
        LogRecord::new(
            at(second),
            Severity::Log,
            format!("duration: {ms} ms  statement: {sql}"),
        )
    }

    fn output() -> Output {
        let mut acc = Accumulators::new(100, 60);
        for (i, ms) in [10.0, 20.0, 30.0].iter().enumerate() {
            acc.add_record(&stmt(
                i64::try_from(i).unwrap(),
                *ms,
                &format!("SELECT * FROM film WHERE film_id = {i}"),
            ));
        }
        acc.add_record(&stmt(5, 500.0, "SELECT * FROM actor"));
        for _ in 0..10 {
            acc.add_record(&stmt(6, 1.0, "SELECT 1"));
        }
        let mut err = LogRecord::new(at(7), Severity::Error, "relation \"x\" does not exist");
        err.sqlstate = Some("42P01".to_owned());
        err.statement = Some("SELECT * FROM x".to_owned());
        acc.add_record(&err);
        acc.add_record(&LogRecord::new(
            at(8),
            Severity::Log,
            "temporary file: path \"p\", size 4096",
        ));
        acc.add_record(&LogRecord::new(
            at(9),
            Severity::Log,
            "checkpoint starting: time",
        ));
        acc.add_record(&LogRecord::new(
            at(10),
            Severity::Log,
            "restartpoint starting: wal",
        ));
        acc.add_record(&LogRecord::new(at(11), Severity::Log, "automatic vacuum of table \"d.s.big\": index scans: 0\nsystem usage: CPU: user: 0.00 s, system: 0.00 s, elapsed: 2.00 s"));
        acc.add_record(&LogRecord::new(at(12), Severity::Log, "automatic vacuum of table \"d.s.small\": index scans: 0\nsystem usage: CPU: user: 0.00 s, system: 0.00 s, elapsed: 0.10 s"));
        let files = vec![FileSummary {
            path: PathBuf::from("a.json"),
            bytes: 100,
            format: Format::Json,
            prefix: None,
            prefix_source: PrefixSource::NotApplicable,
            stats: DecodeStats {
                lines: 20,
                records: 20,
                ..DecodeStats::default()
            },
        }];
        Output { acc, files }
    }

    fn build_with(by: SortBy, top: usize) -> Report {
        Report::build(&output(), &ReportOptions { by, top })
    }

    #[test]
    fn statements_are_ranked_by_total_time_by_default() {
        let report = build_with(SortBy::Total, 10);
        let texts: Vec<&str> = report.statements.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts[0], "SELECT * FROM actor");
        assert_eq!(texts[1], "SELECT * FROM film WHERE film_id = $1");
        assert_eq!(texts[2], "SELECT $1");
        let film = &report.statements[1];
        assert_eq!(film.calls, 3);
        assert!(approx_eq(film.total_ms, 60.0));
        assert!(approx_eq(film.mean_ms.unwrap(), 20.0));
        assert!(approx_eq(film.min_ms.unwrap(), 10.0));
        assert!(approx_eq(film.max_ms.unwrap(), 30.0));
        let p50 = film.p50_ms.unwrap();
        assert!((p50 - 20.0).abs() <= 0.21, "{p50}");
        assert_eq!(film.fingerprint.len(), 16);
    }

    #[test]
    fn other_orders_and_top() {
        let by_calls = build_with(SortBy::Calls, 10);
        assert_eq!(by_calls.statements[0].text, "SELECT $1");
        assert_eq!(by_calls.statements[0].calls, 10);
        let by_max = build_with(SortBy::Max, 1);
        assert_eq!(by_max.statements.len(), 1);
        assert_eq!(by_max.statements[0].text, "SELECT * FROM actor");
        let by_p95 = build_with(SortBy::P95, 10);
        assert_eq!(by_p95.statements[0].text, "SELECT * FROM actor");
        assert_eq!(build_with(SortBy::Total, 0).statements.len(), 0);
    }

    #[test]
    fn shares_add_up_and_include_every_query() {
        let report = build_with(SortBy::Total, 100);
        let sum: f64 = report.statements.iter().map(|s| s.share_pct).sum();
        assert!((sum - 100.0).abs() < 0.01, "{sum}");
    }

    #[test]
    fn attribution_columns_are_filled() {
        let report = build_with(SortBy::Total, 100);
        let missing = report
            .statements
            .iter()
            .find(|s| s.text == "SELECT * FROM x")
            .unwrap();
        assert_eq!(missing.errors, 1);
        assert_eq!(missing.calls, 0);
        assert_eq!(missing.mean_ms, None);
    }

    #[test]
    fn sections_are_filled() {
        let report = build_with(SortBy::Total, 10);
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].sqlstate.as_deref(), Some("42P01"));
        assert_eq!(report.errors[0].count, 1);
        assert_eq!(report.temp_files.bytes, 4096);
        assert_eq!(report.checkpoints.len(), 2);
        assert_eq!(report.checkpoints[0].kind, "checkpoint");
        assert_eq!(report.checkpoints[1].kind, "restartpoint");
        assert_eq!(report.autovacuum[0].table, "d.s.big");
        assert!(approx_eq(report.autovacuum[0].total_ms, 2_000.0));
        assert_eq!(report.overview.events["statement"], 14);
        assert_eq!(
            report.window.first.as_deref(),
            Some(at(0).to_string().as_str())
        );
        assert_eq!(report.files[0].format, "json");
        assert_eq!(report.files[0].prefix_source, "n/a");
        assert_eq!(report.timeline.bucket_secs, 60);
        assert!(!report.timeline.buckets.is_empty());
    }

    #[test]
    fn json_is_versioned_and_typed() {
        let json = build_with(SortBy::Total, 10).to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert!(value["statements"][0]["fingerprint"].is_string());
        assert!(value["statements"][0]["total_ms"].is_number());
        assert!(value["statements"][0]["query_id"].is_null());
        assert_eq!(value["files"][0]["prefix_source"], "n/a");
        assert!(value["files"][0]["prefix"].is_null());
        assert!(value["diagnostics"]["warnings"].is_array());
        assert_eq!(value["errors"][0]["level"], "ERROR");
        assert_eq!(value["checkpoints"][0]["kind"], "checkpoint");
    }

    #[test]
    fn ordering_is_deterministic_on_ties() {
        let mut acc = Accumulators::new(100, 60);
        for sql in ["SELECT a FROM t1", "SELECT a FROM t2", "SELECT a FROM t3"] {
            acc.add_record(&stmt(0, 5.0, sql));
        }
        let out = Output { acc, files: vec![] };
        let a = Report::build(
            &out,
            &ReportOptions {
                by: SortBy::Total,
                top: 10,
            },
        );
        let b = Report::build(
            &out,
            &ReportOptions {
                by: SortBy::Total,
                top: 10,
            },
        );
        let ids: Vec<&String> = a.statements.iter().map(|s| &s.fingerprint).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "equal totals are ordered by fingerprint");
        assert_eq!(a.to_json().unwrap(), b.to_json().unwrap());
    }

    #[test]
    fn an_empty_run_builds_an_empty_report() {
        let out = Output {
            acc: Accumulators::new(10, 60),
            files: vec![],
        };
        let report = Report::build(
            &out,
            &ReportOptions {
                by: SortBy::Total,
                top: 10,
            },
        );
        assert_eq!(report.statements.len(), 0);
        assert_eq!(report.window.first, None);
        report.to_json().unwrap();
    }
}
