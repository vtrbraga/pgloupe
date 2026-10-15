//! The combined report: log statistics joined with `pg_stat_statements` deltas.

mod actions;
mod advice;
mod compare;
mod csv;
mod database;
mod findings;
mod html;
mod join;
#[cfg(test)]
mod testkit;
mod text;

pub use actions::{Action, actions};
pub use advice::{Advice, Input as AdviceInput, advise, detect_vendor, read_settings};
pub use compare::{Comparison, compare};
pub use csv::csv;
pub use database::{DatabaseReport, DatabaseRow, database_findings, database_report};
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
    /// Shared blocks the statement dirtied, and the ones it wrote itself.
    pub shared_blks_dirtied: u64,
    pub shared_blks_written: u64,
    /// Time reading and writing shared data files, in milliseconds (`track_io_timing`).
    pub io_read_ms: f64,
    pub io_write_ms: f64,
    /// Time reading and writing temporary files, in milliseconds (`track_io_timing`).
    pub temp_io_ms: f64,
    /// Blocks found in shared buffers as a share of all blocks asked for.
    pub hit_pct: Option<f64>,
    /// Time in file I/O as a share of execution time; 0 when `track_io_timing` is off.
    pub io_pct: Option<f64>,
    /// Rows returned or affected per call.
    pub rows_per_call: Option<f64>,
    /// Shared blocks read per row returned: high means scanning much to return little.
    pub blocks_read_per_row: Option<f64>,
    /// WAL bytes per call.
    pub wal_per_call: Option<f64>,
    /// Standard deviation and slowest run, in milliseconds, since the entry was created or last
    /// reset (not window figures).
    pub stddev_ms: Option<f64>,
    pub max_ms: Option<f64>,
    /// Standard deviation over the mean: how uneven the run times are.
    pub variability: Option<f64>,
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
    /// `new`, `restarted`, `partial`, `evicted_suspect`, `nested` (only ever run inside functions),
    /// `slower` and `faster` (the mean moved by half or more since before the window).
    pub flags: Vec<&'static str>,
}

/// Something the report noticed about the data and wants the reader to know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
const SCHEMA_VERSION: u32 = 2;

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
    /// Settings that limit what this report can show, and what to change.
    pub advice: Vec<Advice>,
    /// Caveats about the data: what the reader should know before trusting the numbers.
    pub findings: Vec<Finding>,
    /// What ranks `statements`: `total`, `calls`, `p95` or `max`.
    pub statements_by: &'static str,
    /// The ranked top statements, joined across logs and statistics.
    pub statements: Vec<JoinedRow>,
    /// The log analysis; its own `statements` are empty because they are above.
    pub logs: Option<Report>,
    /// This period against an earlier one, when asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Comparison>,
    /// The server-wide counters from `pg_stat_database`, when two snapshots of them were given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<DatabaseReport>,
    /// The zone the dashboard shows times in; every timestamp in this document stays UTC.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_tz: Option<String>,
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
    /// Eviction rounds during the window, when the info files say. Each round removes about 5% of
    /// `pg_stat_statements.max` entries.
    pub evictions: Option<u64>,
    /// About how many entries those rounds removed.
    pub evicted_estimate: Option<u64>,
    /// No info files were given, so a reset would go unnoticed.
    pub blind_spot: bool,
    /// How many entries were new, restarted, gone and so on.
    pub counts: Counts,
    /// What every entry did in the window, added up, and the ratios worth knowing.
    pub totals: StatsTotals,
    /// The same per database and per role, busiest first (top-level statements only).
    pub by_database: Vec<StatsRollup>,
    pub by_user: Vec<StatsRollup>,
}

/// What the statements of one database or role did in the window.
#[derive(Debug, Clone, Serialize)]
pub struct StatsRollup {
    pub name: String,
    /// Statements (distinct entries) that ran.
    pub entries: usize,
    pub calls: u64,
    pub total_ms: f64,
    /// Share of all execution time, in percent.
    pub share_pct: f64,
    pub rows: u64,
    pub hit_pct: Option<f64>,
    /// File I/O as a share of this group's execution time.
    pub io_pct: Option<f64>,
}

/// The statistics of the whole window, over all entries.
#[derive(Debug, Clone, Serialize)]
pub struct StatsTotals {
    pub calls: u64,
    pub exec_ms: f64,
    /// Planning time, in milliseconds (0 unless `pg_stat_statements.track_planning` is on).
    pub plan_ms: f64,
    /// Planning time as a share of execution time.
    pub plan_pct: Option<f64>,
    /// Time reading and writing files (`track_io_timing`), and its share of execution time.
    pub io_ms: f64,
    pub io_pct: Option<f64>,
    /// Blocks found in shared buffers as a share of all blocks asked for.
    pub hit_pct: Option<f64>,
    pub blocks_read: u64,
    pub wal_bytes: u64,
    /// Entries that only ran inside functions.
    pub nested_entries: usize,
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

/// What the logs were filtered by, when anything was left out.
fn log_filter(report: Option<&Report>) -> Option<String> {
    let report = report?;
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
}

/// Statements kept beyond the top for each other axis the Statements view can sort by.
const AXIS_LEADERS: usize = 10;

/// Keeps the first `shown` rows (already ranked), and any row below them that is among the
/// `per_axis` leaders on calls, rows, blocks read, WAL, I/O time or mean time, so sorting by one
/// of those finds them in the report.
fn keep_axis_leaders(rows: &mut Vec<JoinedRow>, shown: usize, per_axis: usize) {
    type Axis = fn(&StatsSide) -> f64;
    #[expect(clippy::cast_precision_loss, reason = "counters stay far below 2^53")]
    let axes: [Axis; 6] = [
        |s| s.calls as f64,
        |s| s.rows as f64,
        |s| s.shared_blks_read as f64,
        |s| s.wal_bytes as f64,
        |s| s.io_read_ms + s.io_write_ms + s.temp_io_ms,
        |s| s.mean_ms.unwrap_or(0.0),
    ];
    let mut keep = vec![false; rows.len()];
    keep.iter_mut().take(shown).for_each(|k| *k = true);
    for axis in axes {
        let mut order: Vec<usize> = (0..rows.len())
            .filter(|&i| rows[i].stats.as_ref().is_some_and(|s| axis(s) > 0.0))
            .collect();
        order.sort_by(|&a, &b| {
            let value = |i: usize| rows[i].stats.as_ref().map_or(0.0, axis);
            value(b)
                .total_cmp(&value(a))
                .then_with(|| rows[a].fingerprint.cmp(&rows[b].fingerprint))
        });
        for &i in order.iter().take(per_axis) {
            keep[i] = true;
        }
    }
    let mut index = 0;
    rows.retain(|_| {
        index += 1;
        keep[index - 1]
    });
}

impl CombinedReport {
    /// Adds the `pg_stat_database` section and the findings it supports.
    #[must_use]
    pub fn with_database(
        mut self,
        database: &crate::stats::database::DatabaseDelta,
        statements: Option<&Delta>,
    ) -> Self {
        let section = database_report(database, statements);
        self.findings
            .extend(database_findings(&section, statements));
        self.findings.sort_by_key(|f| f.severity != "warn");
        self.database = Some(section);
        self
    }
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
    let log_filter = log_filter(log_report.as_ref());
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
    keep_axis_leaders(&mut joined.rows, shown, options.top.min(AXIS_LEADERS));
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
        advice: Vec::new(),
        comparison: None,
        database: None,
        display_tz: None,
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
        evicted_estimate: delta.evicted_estimate,
        blind_spot: delta.blind_spot,
        counts: delta.counts,
        totals: stats_totals(&delta.totals),
        by_database: rollups(delta, |row| row.datname.as_deref()),
        by_user: rollups(delta, |row| row.rolname.as_deref()),
    }
}

/// Databases or roles listed in a rollup.
const ROLLUP_LISTED: usize = 20;

/// Adds the top-level entries up by the name `of` gives, busiest first.
fn rollups(
    delta: &Delta,
    of: impl Fn(&crate::stats::delta::Row) -> Option<&str>,
) -> Vec<StatsRollup> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<&str, (usize, crate::stats::snapshot::Counters)> = BTreeMap::new();
    for row in delta
        .rows
        .iter()
        .filter(|r| r.key.toplevel && r.delta.calls > 0)
    {
        let slot = groups.entry(of(row).unwrap_or("(unknown)")).or_default();
        slot.0 += 1;
        join::add_counters(&mut slot.1, &row.delta);
    }
    let total: f64 = groups.values().map(|(_, c)| c.total_exec_time).sum();
    let mut out: Vec<StatsRollup> = groups
        .into_iter()
        .map(|(name, (entries, c))| StatsRollup {
            name: name.to_owned(),
            entries,
            calls: c.calls,
            total_ms: c.total_exec_time,
            share_pct: if total > 0.0 {
                c.total_exec_time * 100.0 / total
            } else {
                0.0
            },
            rows: c.rows,
            hit_pct: join::ratio_pct(c.shared_blks_hit, c.shared_blks_hit + c.shared_blks_read),
            io_pct: (c.total_exec_time > 0.0).then(|| {
                (c.blk_read_time + c.blk_write_time + c.temp_blk_read_time + c.temp_blk_write_time)
                    * 100.0
                    / c.total_exec_time
            }),
        })
        .collect();
    out.sort_by(|a, b| {
        b.total_ms
            .total_cmp(&a.total_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    out.truncate(ROLLUP_LISTED);
    out
}

fn stats_totals(t: &crate::stats::delta::Totals) -> StatsTotals {
    #[expect(
        clippy::cast_precision_loss,
        reason = "block counts stay far below 2^53"
    )]
    let hit_pct = (t.shared_blks_hit + t.shared_blks_read > 0).then(|| {
        t.shared_blks_hit as f64 * 100.0 / (t.shared_blks_hit + t.shared_blks_read) as f64
    });
    StatsTotals {
        calls: t.calls,
        exec_ms: t.exec_ms,
        plan_ms: t.plan_ms,
        plan_pct: (t.exec_ms > 0.0 && t.plan_ms > 0.0).then(|| t.plan_ms * 100.0 / t.exec_ms),
        io_ms: t.io_ms,
        io_pct: (t.exec_ms > 0.0).then(|| t.io_ms * 100.0 / t.exec_ms),
        hit_pct,
        blocks_read: t.shared_blks_read,
        wal_bytes: t.wal_bytes,
        nested_entries: t.nested_entries,
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
    fn statements_that_lead_on_another_axis_stay_in_a_short_report() {
        use crate::{combined::testkit::delta, testutil::key};
        // The slow one wins on total time; the other makes by far the most calls.
        let d = delta(&[
            (key(1), Some("SELECT slow FROM t"), 2, 9_000.0),
            (key(2), Some("SELECT chatty FROM t"), 50_000, 500.0),
            (key(3), Some("SELECT quiet FROM t"), 1, 1.0),
        ]);
        let report = build(None, Some(&d), &options(SortBy::Total, 1));
        let texts: Vec<&str> = report.statements.iter().map(|s| s.text.as_str()).collect();
        assert!(texts.contains(&"SELECT slow FROM t"), "{texts:?}");
        assert!(texts.contains(&"SELECT chatty FROM t"), "{texts:?}");
        assert_eq!(
            texts[0], "SELECT slow FROM t",
            "ranked by the chosen axis first"
        );
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
        // The cut keeps the top two by the chosen metric, then any leader on another axis.
        let two = rank_of(SortBy::Total, 2);
        assert_eq!(two[..2], by_total[..2]);
        assert!(two.len() < by_total.len(), "{two:?}");
        // Δcalls: the film SELECT ran 60 times.
        assert_eq!(
            rank_of(SortBy::Calls, 1)[0],
            "SELECT * FROM film WHERE film_id = $1"
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
        assert_eq!(value["schema_version"], 2);
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
