//! The combined report: log statistics joined with `pg_stat_statements` deltas.

mod actions;
mod findings;
mod html;
mod join;
#[cfg(test)]
mod testkit;
mod text;

pub use actions::{Action, actions};
pub use findings::{Context, findings};
pub use html::{HtmlError, html, html_live};
pub use join::{Joined, join, rank};
pub use text::text;

use jiff::Timestamp;
use serde::Serialize;

use crate::{
    analyze::report::{Report, ReportOptions, SortBy, StatementRow},
    stats::delta::{Counts, Delta, Reset},
};

/// One statement as seen by the logs, by `pg_stat_statements`, or both.
#[derive(Debug, Clone, Serialize)]
pub struct JoinedRow {
    /// 16 hex digits, or `q<queryid>` for a statistics entry whose text could not be fingerprinted.
    pub fingerprint: String,
    /// The statement text: the log's sample when there is one, else the statistics'.
    pub text: String,
    /// `both`, `log` or `stats`.
    pub link: &'static str,
    /// What the log saw; `None` for a statistics-only row.
    pub log: Option<StatementRow>,
    /// What `pg_stat_statements` saw; `None` for a log-only row.
    pub stats: Option<StatsSide>,
    /// Log calls that carried a duration ÷ statistics calls in the window.
    pub coverage: Option<f64>,
    /// Whether the `query_id` the log carried is among the statistics entry's `queryid`s.
    pub query_id_matches: Option<bool>,
}

/// What `pg_stat_statements` saw of a statement during the snapshot window, summed over every
/// entry (user, database, partition) with the same fingerprint.
#[derive(Debug, Clone, Serialize)]
pub struct StatsSide {
    /// Calls during the window.
    pub calls: u64,
    /// Execution time during the window, in milliseconds.
    pub total_exec_ms: f64,
    /// `total_exec_ms` ÷ `calls`.
    pub mean_ms: Option<f64>,
    /// Rows returned or affected.
    pub rows: u64,
    /// Planner runs (0 unless `pg_stat_statements.track_planning` is on).
    pub plans: u64,
    /// Shared buffer hits.
    pub shared_blks_hit: u64,
    /// Shared blocks read from disk or the OS cache.
    pub shared_blks_read: u64,
    /// Temporary blocks read.
    pub temp_blks_read: u64,
    /// Temporary blocks written.
    pub temp_blks_written: u64,
    /// WAL generated, in bytes.
    pub wal_bytes: u64,
    /// Share of all execution time in the window, in percent.
    pub share_pct: f64,
    /// Calls of the same statement inside functions (`track = all`), kept out of `calls`.
    pub nested_calls: u64,
    /// Statistics entries merged into this row.
    pub entries: usize,
    /// Distinct `queryid`s among them.
    pub queryid_count: usize,
    /// The first five, ascending, as text (64-bit ids do not survive JSON numbers).
    pub queryids: Vec<String>,
    /// `new`, `restarted`, `partial`, `evicted_suspect`, `nested` (only ever run inside functions).
    pub flags: Vec<&'static str>,
}

/// Something the report noticed about the data and wants the reader to know.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    /// Stable identifier, for tools and tests.
    pub code: &'static str,
    /// `info` or `warn`.
    pub severity: &'static str,
    pub message: String,
    /// Fingerprints the finding is about, when it is about particular statements.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub statements: Vec<String>,
}

/// Version of the JSON layout; bump when a field changes meaning.
const SCHEMA_VERSION: u32 = 1;

/// The whole report: what the inputs were, what they say, and what the reader should know.
#[derive(Debug, Clone, Serialize)]
pub struct CombinedReport {
    /// Version of this layout.
    pub schema_version: u32,
    /// Always `pgloupe.report`; tells a consumer which document this is.
    pub kind: &'static str,
    /// Program and version that wrote the report.
    pub generator: String,
    /// Where the data came from.
    pub inputs: Inputs,
    /// The periods the two sources cover and how they relate.
    pub windows: Windows,
    /// The problems to look at first, most urgent first, each with what to do about it.
    pub actions: Vec<Action>,
    /// Caveats about the data: what the reader should know before trusting the numbers.
    pub findings: Vec<Finding>,
    /// What ranks `statements`: `total`, `calls`, `p95` or `max`.
    pub statements_by: &'static str,
    /// The ranked top statements, joined across logs and statistics.
    pub statements: Vec<JoinedRow>,
    /// The log analysis; its own `statements` are empty because they are above.
    pub logs: Option<Report>,
}

/// Where the report's data came from.
#[derive(Debug, Clone, Serialize)]
pub struct Inputs {
    /// The log side; `None` for a snapshots-only report.
    pub logs: Option<LogInput>,
    /// The statistics side; `None` for a logs-only report.
    pub stats: Option<StatsInput>,
}

/// The log input of a report.
#[derive(Debug, Clone, Serialize)]
pub struct LogInput {
    /// `files` or `store`.
    pub source: &'static str,
    /// Log files analyzed, or inputs of the store inside the window.
    pub inputs: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

/// The statistics input of a report.
#[derive(Debug, Clone, Serialize)]
pub struct StatsInput {
    /// When the older snapshot was taken.
    pub before_at: String,
    /// When the newer snapshot was taken.
    pub after_at: String,
    /// Server version of the snapshots (`server_version_num`).
    pub server_version_num: u32,
    /// Whether the counters were reset between the snapshots.
    pub reset: Reset,
    /// When the counters were last reset, when known.
    pub reset_at: Option<String>,
    /// Entries evicted during the window, when the info files say.
    pub evictions: Option<u64>,
    /// No info files were given, so a reset would go unnoticed.
    pub blind_spot: bool,
    /// How many entries were new, restarted, gone and so on.
    pub counts: Counts,
}

/// The periods the two sources cover.
#[derive(Debug, Clone, Serialize)]
pub struct Windows {
    /// First to last log record.
    pub logs: Option<Span>,
    /// Older to newer snapshot.
    pub stats: Option<Span>,
    /// How much of the shorter window the other one covers, in percent.
    pub overlap_pct: Option<f64>,
    /// Length of the log window ÷ length of the statistics window.
    pub length_ratio: Option<f64>,
}

/// A period of time, as RFC 3339 timestamps.
#[derive(Debug, Clone, Serialize)]
pub struct Span {
    /// Start of the period.
    pub from: String,
    /// End of the period.
    pub to: String,
}

/// The log side of a report: the analysis, a row for every fingerprint, and where it came from.
#[derive(Debug)]
pub struct LogData {
    /// The log analysis.
    pub report: Report,
    /// One row per fingerprint.
    pub rows: Vec<StatementRow>,
    /// `files` or `store`.
    pub source: &'static str,
    /// Files analyzed, or inputs of the store inside the window.
    pub inputs: usize,
    /// The store's path, when the logs came from one.
    pub store: Option<String>,
}

/// Joins the two sources, ranks and truncates the statements by `options`, and adds findings.
#[must_use]
pub fn build(
    logs: Option<LogData>,
    delta: Option<&Delta>,
    options: &ReportOptions,
) -> CombinedReport {
    let (log_rows, log_report, log_input) = match logs {
        Some(data) => (
            data.rows,
            Some(data.report),
            Some(LogInput {
                source: data.source,
                inputs: data.inputs,
                store: data.store,
            }),
        ),
        None => (Vec::new(), None, None),
    };
    let log_span = log_report.as_ref().and_then(|report| {
        let first = report.window.first.clone()?;
        let last = report.window.last.clone()?;
        Some(Span {
            from: first,
            to: last,
        })
    });
    let stats_span = delta.map(|delta| Span {
        from: delta.before_at.to_string(),
        to: delta.after_at.to_string(),
    });
    let (overlap, ratio) = match (&log_span, delta) {
        (Some(log), Some(delta)) => match (log.from.parse(), log.to.parse()) {
            (Ok(from), Ok(to)) => {
                let stats = (delta.before_at, delta.after_at);
                (
                    Some(overlap_pct((from, to), stats)),
                    length_ratio((from, to), stats),
                )
            }
            _ => (None, None),
        },
        _ => (None, None),
    };
    let log_overflow_calls = log_report
        .as_ref()
        .map_or(0, |report| report.diagnostics.overflow_calls);
    let log_filter = log_report.as_ref().and_then(|report| {
        report
            .store
            .as_ref()
            .and_then(|store| store.filter.clone())
            .or_else(|| {
                (report.diagnostics.filtered > 0).then(|| {
                    format!(
                        "{} records dropped by the filters",
                        report.diagnostics.filtered
                    )
                })
            })
    });
    let mut joined = join(log_rows, delta);
    rank(&mut joined.rows, options.by);
    let shown = joined.rows.len().min(options.top);
    let found = findings(&Context {
        delta,
        joined: &joined,
        overlap_pct: overlap,
        window_ratio: ratio,
        top: &joined.rows[..shown],
        has_logs: log_input.is_some(),
        from_store: log_input
            .as_ref()
            .is_some_and(|input| input.source == "store"),
        log_overflow_calls,
        log_filter,
    });
    let to_do = actions(log_report.as_ref(), &joined);
    joined.rows.truncate(shown);
    CombinedReport {
        schema_version: SCHEMA_VERSION,
        kind: "pgloupe.report",
        generator: format!("pgloupe {}", env!("CARGO_PKG_VERSION")),
        inputs: Inputs {
            logs: log_input,
            stats: delta.map(stats_input),
        },
        windows: Windows {
            logs: log_span,
            stats: stats_span,
            overlap_pct: overlap,
            length_ratio: ratio,
        },
        actions: to_do,
        findings: found,
        statements_by: match options.by {
            SortBy::Total => "total",
            SortBy::Calls => "calls",
            SortBy::P95 => "p95",
            SortBy::Max => "max",
        },
        statements: joined.rows,
        logs: log_report.map(|mut report| {
            report.statements.clear();
            report
        }),
    }
}

fn stats_input(delta: &Delta) -> StatsInput {
    StatsInput {
        before_at: delta.before_at.to_string(),
        after_at: delta.after_at.to_string(),
        server_version_num: delta.server_version_num,
        reset: delta.reset,
        reset_at: delta.reset_at.map(|at| at.to_string()),
        evictions: delta.evictions,
        blind_spot: delta.blind_spot,
        counts: delta.counts,
    }
}

/// The share of the shorter of two periods that the other one covers, in percent. A period of
/// zero length is inside (100) or outside (0).
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "millisecond spans stay far below 2^53"
)]
pub fn overlap_pct(logs: (Timestamp, Timestamp), stats: (Timestamp, Timestamp)) -> f64 {
    let millis = |a: Timestamp, b: Timestamp| (b.as_millisecond() - a.as_millisecond()).max(0);
    let (log_len, stats_len) = (millis(logs.0, logs.1), millis(stats.0, stats.1));
    let shared = millis(logs.0.max(stats.0), logs.1.min(stats.1));
    let shorter = log_len.min(stats_len);
    if shorter == 0 {
        let inside = logs.0 >= stats.0 && logs.1 <= stats.1;
        return if inside { 100.0 } else { 0.0 };
    }
    (shared as f64 * 100.0 / shorter as f64).clamp(0.0, 100.0)
}

/// Length of the log window ÷ length of the statistics window; `None` when either is empty.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "millisecond spans stay far below 2^53"
)]
pub fn length_ratio(logs: (Timestamp, Timestamp), stats: (Timestamp, Timestamp)) -> Option<f64> {
    let millis = |(a, b): (Timestamp, Timestamp)| b.as_millisecond() - a.as_millisecond();
    let (log_len, stats_len) = (millis(logs), millis(stats));
    (log_len > 0 && stats_len > 0).then(|| log_len as f64 / stats_len as f64)
}

impl CombinedReport {
    /// The report as pretty-printed JSON (no trailing newline).
    ///
    /// # Errors
    ///
    /// Fails only if serialization fails, which these types do not produce in practice.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value;

    use super::*;
    use crate::{
        analyze::{
            pipeline::{self, Options},
            report::all_statements,
        },
        stats::{delta::diff, snapshot::Snapshot},
        testutil::ts,
    };

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    fn options(by: SortBy, top: usize) -> ReportOptions {
        ReportOptions { by, top }
    }

    fn log_data() -> LogData {
        let output =
            pipeline::run(&[fixtures().join("combined/logs.json")], &Options::new()).unwrap();
        LogData {
            report: Report::build(&output, &options(SortBy::Total, 20)),
            rows: all_statements(&output.acc),
            source: "files",
            inputs: 1,
            store: None,
        }
    }

    fn stats_delta() -> Delta {
        let before = Snapshot::from_path(&fixtures().join("stats/basic_before.csv")).unwrap();
        let after = Snapshot::from_path(&fixtures().join("stats/basic_after.csv")).unwrap();
        diff(&before, &after).unwrap()
    }

    fn links(report: &CombinedReport) -> Vec<&'static str> {
        report.statements.iter().map(|row| row.link).collect()
    }

    #[test]
    fn a_logs_only_report_has_no_statistics_side() {
        let report = build(Some(log_data()), None, &options(SortBy::Total, 20));
        assert!(links(&report).iter().all(|link| *link == "log"));
        assert!(report.inputs.stats.is_none() && report.windows.stats.is_none());
        assert_eq!(report.windows.overlap_pct, None);
        let logs = report.logs.as_ref().unwrap();
        assert!(
            logs.statements.is_empty(),
            "statements live at the top level"
        );
        assert_eq!(logs.overview.records, 9);
        assert_eq!(report.inputs.logs.as_ref().unwrap().source, "files");
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    #[test]
    fn a_statistics_only_report_has_no_log_section() {
        let report = build(None, Some(&stats_delta()), &options(SortBy::Total, 20));
        assert!(links(&report).iter().all(|link| *link == "stats"));
        assert!(report.logs.is_none() && report.inputs.logs.is_none());
        assert!(report.windows.logs.is_none());
        assert!(!report.findings.iter().any(|f| f.code == "windows_disjoint"));
        assert_eq!(
            report.inputs.stats.as_ref().unwrap().server_version_num,
            170_011
        );
    }

    #[test]
    fn rows_are_ranked_by_the_chosen_metric_and_truncated() {
        let d = stats_delta();
        let rank_of = |by, top| {
            build(Some(log_data()), Some(&d), &options(by, top))
                .statements
                .iter()
                .map(|row| row.text.clone())
                .collect::<Vec<_>>()
        };
        // Δtime: UPDATE 400 ms, DELETE 70 ms, SELECT film 30 ms; actor and nope are log-only.
        let by_total = rank_of(SortBy::Total, 20);
        assert_eq!(by_total[0], "UPDATE film SET rental_rate = $1");
        assert_eq!(by_total[1], "DELETE FROM rental WHERE rental_id = $1");
        assert_eq!(rank_of(SortBy::Total, 2).len(), 2);
        // Δcalls: the film SELECT ran 60 times.
        assert_eq!(
            rank_of(SortBy::Calls, 1),
            ["SELECT * FROM film WHERE film_id = $1"]
        );
        // Max logged duration: the DELETE has no log side and goes last among the stats rows.
        let by_max = rank_of(SortBy::Max, 20);
        assert_eq!(by_max[0], "UPDATE film SET rental_rate = $1");
        assert!(by_max.iter().position(|t| t.starts_with("DELETE")).unwrap() >= 3);
    }

    #[test]
    fn both_sides_join_and_explain() {
        let report = build(
            Some(log_data()),
            Some(&stats_delta()),
            &options(SortBy::Total, 20),
        );
        let film = report
            .statements
            .iter()
            .find(|r| r.text.contains("film_id"))
            .unwrap();
        assert_eq!(film.link, "both");
        assert!((film.coverage.unwrap() - 4.0 / 60.0).abs() < 1e-9);
        assert!(report.statements.iter().any(|r| r.link == "log"));
        assert!(report.statements.iter().any(|r| r.link == "stats"));
        let codes: Vec<_> = report.findings.iter().map(|f| f.code).collect();
        assert!(
            codes.contains(&"stats_blind_spot") && codes.contains(&"log_only_present"),
            "{codes:?}"
        );
        // The snapshots cover 10:00-10:10 and the log 10:01-10:09: fully overlapping.
        assert!((report.windows.overlap_pct.unwrap() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn overlap_is_a_share_of_the_shorter_window() {
        let span = |a: &str, b: &str| (ts(a), ts(b));
        let case = |logs, stats| overlap_pct(logs, stats);
        let hour = span("2026-10-14T10:00:00Z", "2026-10-14T11:00:00Z");
        assert!(
            (case(span("2026-10-14T10:15:00Z", "2026-10-14T10:45:00Z"), hour) - 100.0).abs() < 1e-9
        );
        assert!(
            (case(span("2026-10-14T10:30:00Z", "2026-10-14T12:30:00Z"), hour) - 50.0).abs() < 1e-9
        );
        assert!(case(span("2026-10-14T12:00:00Z", "2026-10-14T13:00:00Z"), hour).abs() < 1e-9);
        // A log window of zero length counts as inside or outside.
        assert!(
            (case(span("2026-10-14T10:20:00Z", "2026-10-14T10:20:00Z"), hour) - 100.0).abs() < 1e-9
        );
        assert!(case(span("2026-10-14T09:00:00Z", "2026-10-14T09:00:00Z"), hour).abs() < 1e-9);
    }

    #[test]
    fn the_json_layout_is_pinned() {
        let report = build(
            Some(log_data()),
            Some(&stats_delta()),
            &options(SortBy::Total, 20),
        );
        let mut value: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["kind"], "pgloupe.report");
        // Machine- and version-dependent values.
        value["generator"] = Value::String("pgloupe".to_owned());
        value["logs"]["files"][0]["path"] = Value::String("logs.json".to_owned());
        let golden = fixtures().join("combined/report.golden.json");
        let rendered = serde_json::to_string_pretty(&value).unwrap() + "\n";
        if std::env::var_os("PGLOUPE_BLESS").is_some() {
            std::fs::write(&golden, &rendered).unwrap();
        }
        assert_eq!(rendered, std::fs::read_to_string(golden).unwrap());
    }
}
