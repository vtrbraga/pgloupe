//! The versioned report: statistics shaped for display and for JSON consumers.

use std::{cmp::Reverse, collections::BTreeMap};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{
    acc::{Accumulators, QueryStats, to_micros},
    diagnostics::{self, Diagnostics},
    dimensions::{DimStats, Dimension},
    histogram::{self, HistogramBucket},
    kind::QueryKind,
    pipeline::{FileSummary, Output, PrefixSource},
    sketch::Sketch,
    slowest::Execution,
};
use crate::log::Format;

/// Version of the JSON layout; bump when a field changes meaning.
const SCHEMA_VERSION: u32 = 1;
/// Tables listed in the autovacuum section.
const TOP_TABLES: usize = 10;
/// Values listed per breakdown dimension.
const TOP_DIMENSION_VALUES: usize = 20;

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
    /// Statements by what they do, busiest first.
    pub kinds: Vec<KindRow>,
    /// Statement and session durations in fixed ranges.
    pub histograms: HistogramsReport,
    /// Statements, errors and sessions by database, user, application and host.
    pub dimensions: DimensionsReport,
    /// Statements cancelled by timeout or request.
    pub cancelled: CancelledReport,
    /// The slowest single executions.
    pub slowest: SlowestReport,
    pub timeline: TimelineReport,
    /// Present when the report was built from a store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<StoreInfo>,
}

/// One class of statements.
#[derive(Debug, Clone, Serialize)]
pub struct KindRow {
    /// `select`, `insert`, `update`, `delete`, `copy`, `ddl`, `tcl`, `utility` or `other`.
    pub kind: &'static str,
    /// Statements run.
    pub calls: u64,
    /// Of those, the ones that carried a duration.
    pub timed_calls: u64,
    /// Their summed duration, in milliseconds.
    pub total_ms: f64,
    /// `total_ms` ÷ `timed_calls`.
    pub mean_ms: Option<f64>,
    /// Share of all statement time, in percent.
    pub share_pct: f64,
}

/// Duration histograms; the last bucket of each has no upper edge.
#[derive(Debug, Clone, Serialize)]
pub struct HistogramsReport {
    /// Statement durations (edges 1 ms to 10 s).
    pub statements: Vec<HistogramBucket>,
    /// Session durations (edges 1 s to 24 h).
    pub sessions: Vec<HistogramBucket>,
}

/// What one database, user, application or host did.
#[derive(Debug, Clone, Serialize)]
pub struct DimensionRow {
    /// The value, `(none)` for records without it and `(other)` for the ones beyond the cap.
    pub value: String,
    /// Statements run.
    pub statements: u64,
    /// Of those, the ones that carried a duration.
    pub timed_calls: u64,
    /// Their summed duration, in milliseconds.
    pub total_ms: f64,
    /// `total_ms` ÷ `timed_calls`.
    pub mean_ms: Option<f64>,
    /// Records at `ERROR` or above.
    pub errors: u64,
    /// Connections authorized.
    pub connections: u64,
    /// Sessions that ended.
    pub sessions: u64,
    /// Their mean duration, in milliseconds.
    pub mean_session_ms: Option<f64>,
}

/// The busiest values of each dimension.
#[derive(Debug, Clone, Serialize)]
pub struct DimensionsReport {
    pub database: Vec<DimensionRow>,
    pub user: Vec<DimensionRow>,
    pub application: Vec<DimensionRow>,
    pub host: Vec<DimensionRow>,
}

/// A statement that failed with an error, and how often.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorStatementRow {
    /// 16 hex digits.
    pub fingerprint: String,
    /// Failures of this kind.
    pub count: u64,
}

/// How many statements were cancelled, why, and which ones.
#[derive(Debug, Clone, Serialize)]
pub struct CancelledReport {
    /// Cancelled executions.
    pub total: u64,
    /// Count per reason, most frequent first.
    pub by_reason: Vec<CancelReasonRow>,
    /// The statements cancelled most often.
    pub statements: Vec<CancelledStatement>,
}

/// Cancellations for one reason.
#[derive(Debug, Clone, Serialize)]
pub struct CancelReasonRow {
    /// `statement_timeout`, `user_request`, `lock_timeout`, `lock_not_available` or `other`.
    pub reason: String,
    /// Cancelled executions.
    pub count: u64,
    /// The statements cancelled for this reason, most often first.
    pub statements: Vec<ErrorStatementRow>,
}

/// A statement and how often it was cancelled.
#[derive(Debug, Clone, Serialize)]
pub struct CancelledStatement {
    /// 16 hex digits.
    pub fingerprint: String,
    /// The normalized statement.
    pub text: String,
    /// Times cancelled.
    pub cancelled: u64,
    /// Times it was run, as far as the log shows.
    pub calls: u64,
}

/// The slowest executions, slowest first.
#[derive(Debug, Clone, Serialize)]
pub struct SlowestReport {
    /// Whether any row carries bind parameters (they are kept only with `--params`).
    pub has_params: bool,
    /// The executions.
    pub rows: Vec<SlowestRow>,
}

/// One execution.
#[derive(Debug, Clone, Serialize)]
pub struct SlowestRow {
    /// When the log recorded it.
    pub timestamp: String,
    /// How long it took, in milliseconds.
    pub duration_ms: f64,
    /// 16 hex digits; `0000000000000000` for statements in the overflow row.
    pub fingerprint: String,
    pub database: Option<String>,
    pub user: Option<String>,
    pub application: Option<String>,
    pub host: Option<String>,
    /// The statement as the log spelled it.
    pub text: String,
    /// Bind parameters, when kept.
    pub params: Option<String>,
}

/// Statements listed in the cancelled section.
const TOP_CANCELLED: usize = 10;

/// Where a report came from, when it came from a store.
#[derive(Debug, Clone, Serialize)]
pub struct StoreInfo {
    pub path: String,
    /// The requested lower bound, snapped down to the hour; `None` when unbounded.
    pub since: Option<String>,
    /// The requested upper bound, snapped up to the hour; `None` when unbounded.
    pub until: Option<String>,
    pub inputs: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileReport {
    pub path: String,
    pub bytes: u64,
    /// `csv`, `json` or `stderr`.
    pub format: String,
    pub prefix: Option<String>,
    /// `given`, `inferred` or `n/a`.
    pub prefix_source: String,
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
    pub statement_calls: u64,
    pub execute_calls: u64,
    pub timed_calls: u64,
    pub total_ms: f64,
    /// `total_ms` plus parse, bind and fetch time: what the statement cost the server in all.
    pub all_ms: f64,
    /// Parse, bind and fetch time, when the log has any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phases: Option<PhasesRow>,
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
    /// The slowest run of this statement and who ran it.
    pub examples: Vec<ExampleRow>,
}

/// Time in one phase of a statement's life.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct PhaseRow {
    pub count: u64,
    pub total_ms: f64,
    pub max_ms: f64,
}

/// Parse, bind and fetch time of one statement.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct PhasesRow {
    pub parse: PhaseRow,
    pub bind: PhaseRow,
    pub fetch: PhaseRow,
}

/// One slow run of a statement.
#[derive(Debug, Clone, Serialize)]
pub struct ExampleRow {
    /// When the log recorded it.
    pub timestamp: String,
    /// How long it took, in milliseconds.
    pub duration_ms: f64,
    pub database: Option<String>,
    pub user: Option<String>,
    pub application: Option<String>,
    pub host: Option<String>,
    /// Bind parameters, when kept (`--params`).
    pub params: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorRow {
    pub level: String,
    pub sqlstate: Option<String>,
    pub count: u64,
    pub first_message: String,
    /// The DETAIL of the first record (for a deadlock: who waited for whom).
    pub detail: Option<String>,
    /// The HINT of the first record.
    pub hint: Option<String>,
    /// The backend process that logged the first record.
    pub pid: Option<u32>,
    /// The statements that failed with this error, most often first.
    pub statements: Vec<ErrorStatementRow>,
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
            kinds: kinds(acc),
            histograms: HistogramsReport {
                statements: histogram::buckets(
                    &acc.statement_durations(),
                    &histogram::STATEMENT_EDGES_MS,
                ),
                sessions: histogram::buckets(
                    &totals.connections.session,
                    &histogram::SESSION_EDGES_MS,
                ),
            },
            dimensions: DimensionsReport {
                database: dimension_rows(acc, Dimension::Database),
                user: dimension_rows(acc, Dimension::User),
                application: dimension_rows(acc, Dimension::Application),
                host: dimension_rows(acc, Dimension::Host),
            },
            cancelled: cancelled(acc),
            slowest: slowest(acc),
            timeline: timeline(acc),
            store: None,
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

pub(crate) fn file_report(file: &FileSummary) -> FileReport {
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
        }
        .to_owned(),
        prefix: file.prefix.clone(),
        prefix_source: prefix_source.to_owned(),
        prefix_match,
        lines: file.stats.lines,
        records: file.stats.records,
        skipped: file.stats.skipped,
    }
}

fn sort_key(query: &QueryStats, by: SortBy) -> u64 {
    match by {
        SortBy::Total => query.total_us + query.phases.total_us(),
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
    let total_us = total_us(acc);
    rows.sort_by(|a, b| {
        sort_key(b, options.by)
            .cmp(&sort_key(a, options.by))
            .then(a.id.cmp(&b.id))
    });
    rows.truncate(options.top);
    rows.into_iter()
        .map(|query| statement_row(query, total_us, &acc.who))
        .collect()
}

/// Execution time of every statement, overflow bucket included: the base of the shares.
fn total_us(acc: &Accumulators) -> u64 {
    acc.queries
        .values()
        .chain(std::iter::once(&acc.overflow))
        .map(|query| query.total_us + query.phases.total_us())
        .sum()
}

/// One row per fingerprint, in fingerprint order, with the same shares [`Report::build`] shows.
/// The overflow bucket is not a statement and gets no row.
#[must_use]
pub fn all_statements(acc: &Accumulators) -> Vec<StatementRow> {
    let total_us = total_us(acc);
    acc.queries
        .values()
        .map(|query| statement_row(query, total_us, &acc.who))
        .collect()
}

fn phases_row(phases: &crate::analyze::acc::Phases) -> Option<PhasesRow> {
    let row = |p: crate::analyze::acc::PhaseStat| PhaseRow {
        count: p.count,
        total_ms: ms(p.total_us),
        max_ms: ms(p.max_us),
    };
    (phases.total_us() > 0 || phases.parse.count + phases.bind.count + phases.fetch.count > 0).then(
        || PhasesRow {
            parse: row(phases.parse),
            bind: row(phases.bind),
            fetch: row(phases.fetch),
        },
    )
}

fn statement_row(
    query: &QueryStats,
    total_us: u64,
    who: &std::collections::BTreeMap<u64, crate::analyze::slowest::Who>,
) -> StatementRow {
    StatementRow {
        fingerprint: format!("{:016x}", query.id),
        query_id: query.query_id.map(|id| id.to_string()),
        text: query.text.clone(),
        parsed: query.parsed,
        calls: query.calls,
        statement_calls: query.statement_calls,
        execute_calls: query.execute_calls,
        timed_calls: query.timed,
        total_ms: ms(query.total_us),
        all_ms: ms(query.total_us + query.phases.total_us()),
        phases: phases_row(&query.phases),
        mean_ms: query.mean_ms(),
        min_ms: query.min_us.map(ms),
        p50_ms: query.sketch.quantile(0.5),
        p95_ms: query.sketch.quantile(0.95),
        p99_ms: query.sketch.quantile(0.99),
        max_ms: (query.timed > 0).then(|| ms(query.max_us)),
        share_pct: percent(query.total_us + query.phases.total_us(), total_us),
        errors: query.errors,
        lock_waits: query.lock_waits,
        lock_wait_ms: ms(query.lock_wait_us),
        temp_files: query.temp_files,
        temp_bytes: query.temp_bytes,
        first_seen: time(query.first_seen),
        last_seen: time(query.last_seen),
        examples: query
            .examples
            .rows()
            .iter()
            .map(|run| {
                let who = who.get(&run.who);
                ExampleRow {
                    timestamp: run.ts.to_string(),
                    duration_ms: ms(run.duration_us),
                    database: who.and_then(|w| w.database.clone()),
                    user: who.and_then(|w| w.user.clone()),
                    application: who.and_then(|w| w.application.clone()),
                    host: who.and_then(|w| w.host.clone()),
                    params: run.params.clone(),
                }
            })
            .collect(),
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
            detail: stat.first_detail.clone(),
            hint: stat.first_hint.clone(),
            pid: stat.first_pid,
            statements: {
                let mut busiest: Vec<(&u64, &u64)> = stat.statements.iter().collect();
                busiest.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                busiest
                    .into_iter()
                    .map(|(id, count)| ErrorStatementRow {
                        fingerprint: format!("{id:016x}"),
                        count: *count,
                    })
                    .collect()
            },
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

fn kinds(acc: &Accumulators) -> Vec<KindRow> {
    let total = total_us(acc);
    let mut rows: Vec<KindRow> = QueryKind::ALL
        .iter()
        .filter_map(|&kind| {
            let mut sums = (0, 0, 0);
            let members = acc
                .queries
                .values()
                .chain(std::iter::once(&acc.overflow))
                .filter(|query| {
                    let own = if query.id == 0 {
                        QueryKind::Other
                    } else {
                        query.kind
                    };
                    own == kind
                });
            for query in members {
                sums = (
                    sums.0 + query.calls,
                    sums.1 + query.timed,
                    sums.2 + query.total_us,
                );
            }
            (sums.0 > 0).then(|| KindRow {
                kind: kind.as_str(),
                calls: sums.0,
                timed_calls: sums.1,
                total_ms: ms(sums.2),
                mean_ms: (sums.1 > 0).then(|| ms(sums.2) / count(sums.1)),
                share_pct: percent(sums.2, total),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.total_ms
            .total_cmp(&a.total_ms)
            .then_with(|| b.calls.cmp(&a.calls))
    });
    rows
}

fn cancelled(acc: &Accumulators) -> CancelledReport {
    let mut by_reason: Vec<CancelReasonRow> = acc
        .totals
        .cancelled
        .iter()
        .map(|(reason, count)| CancelReasonRow {
            reason: reason.clone(),
            count: *count,
            statements: {
                let mut busiest: Vec<(&u64, &u64)> = acc
                    .totals
                    .cancelled_statements
                    .get(reason)
                    .map(|m| m.iter().collect())
                    .unwrap_or_default();
                busiest.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                busiest
                    .into_iter()
                    .map(|(id, count)| ErrorStatementRow {
                        fingerprint: format!("{id:016x}"),
                        count: *count,
                    })
                    .collect()
            },
        })
        .collect();
    by_reason.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.reason.cmp(&b.reason)));
    let mut statements: Vec<&QueryStats> =
        acc.queries.values().filter(|q| q.cancelled > 0).collect();
    statements.sort_by(|a, b| b.cancelled.cmp(&a.cancelled).then(a.id.cmp(&b.id)));
    CancelledReport {
        total: acc.totals.cancelled.values().sum(),
        by_reason,
        statements: statements
            .into_iter()
            .take(TOP_CANCELLED)
            .map(|q| CancelledStatement {
                fingerprint: format!("{:016x}", q.id),
                text: q.text.clone(),
                cancelled: q.cancelled,
                calls: q.calls,
            })
            .collect(),
    }
}

fn slowest(acc: &Accumulators) -> SlowestReport {
    let rows: Vec<SlowestRow> = acc
        .slowest
        .rows()
        .iter()
        .map(|run: &Execution| SlowestRow {
            timestamp: run.ts.to_string(),
            duration_ms: ms(run.duration_us),
            fingerprint: format!("{:016x}", run.fingerprint),
            database: run.database.clone(),
            user: run.user.clone(),
            application: run.application.clone(),
            host: run.host.clone(),
            text: run.text.clone(),
            params: run.params.clone(),
        })
        .collect();
    SlowestReport {
        has_params: rows.iter().any(|row| row.params.is_some()),
        rows,
    }
}

fn dimension_rows(acc: &Accumulators, dimension: Dimension) -> Vec<DimensionRow> {
    let mut rows: Vec<(&String, &DimStats)> = acc.dims.of(dimension).iter().collect();
    rows.sort_by(|a, b| {
        Reverse(a.1.total_us)
            .cmp(&Reverse(b.1.total_us))
            .then_with(|| b.1.statements.cmp(&a.1.statements))
            .then_with(|| b.1.connections.cmp(&a.1.connections))
            .then_with(|| a.0.cmp(b.0))
    });
    rows.into_iter()
        .take(TOP_DIMENSION_VALUES)
        .map(|(value, stats)| DimensionRow {
            value: value.clone(),
            statements: stats.statements,
            timed_calls: stats.timed,
            total_ms: ms(stats.total_us),
            mean_ms: (stats.timed > 0).then(|| ms(stats.total_us) / count(stats.timed)),
            errors: stats.errors,
            connections: stats.connections,
            sessions: stats.sessions,
            mean_session_ms: (stats.sessions > 0)
                .then(|| ms(stats.session_us) / count(stats.sessions)),
        })
        .collect()
}

#[expect(clippy::cast_precision_loss, reason = "counts stay far below 2^53")]
fn count(n: u64) -> f64 {
    n as f64
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
            filtered: 0,
            content: None,
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

    #[test]
    fn statement_rows_split_calls_by_protocol() {
        let mut acc = Accumulators::new(100, 60);
        acc.add_record(&LogRecord::new(
            at(0),
            Severity::Log,
            "duration: 1 ms  statement: SELECT 1",
        ));
        acc.add_record(&LogRecord::new(
            at(1),
            Severity::Log,
            "duration: 1 ms  execute S_1: SELECT 1",
        ));
        let report = Report::build(
            &Output { acc, files: vec![] },
            &ReportOptions {
                by: SortBy::Total,
                top: 5,
            },
        );
        let row = &report.statements[0];
        assert_eq!(
            (row.calls, row.statement_calls, row.execute_calls),
            (2, 1, 1)
        );
    }

    #[test]
    fn all_statements_covers_every_fingerprint_with_matching_shares() {
        let out = output();
        let report = Report::build(
            &out,
            &ReportOptions {
                by: SortBy::Total,
                top: 2,
            },
        );
        let all = all_statements(&out.acc);
        assert_eq!(report.statements.len(), 2);
        assert_eq!(all.len(), out.acc.queries.len());
        assert!(all.len() > 2);
        for shown in &report.statements {
            let same = all
                .iter()
                .find(|row| row.fingerprint == shown.fingerprint)
                .unwrap();
            assert_eq!(same.calls, shown.calls);
            assert!(approx_eq(same.share_pct, shown.share_pct));
        }
    }

    #[test]
    fn cancellations_and_slow_executions_are_reported() {
        let mut acc = Accumulators::new(100, 60).with_capture(2, true);
        for (second, ms) in [(1, 5.0), (2, 70.0), (3, 30.0)] {
            acc.add_record(&LogRecord {
                detail: Some(format!("parameters: $1 = '{second}'")),
                ..LogRecord::new(
                    at(second),
                    Severity::Log,
                    format!("duration: {ms} ms  execute <unnamed>: SELECT * FROM t WHERE id = $1"),
                )
            });
        }
        for second in [4, 5] {
            let mut cancelled = LogRecord::new(
                at(second),
                Severity::Error,
                "canceling statement due to statement timeout",
            );
            cancelled.sqlstate = Some("57014".to_owned());
            cancelled.statement = Some("SELECT pg_sleep(60)".to_owned());
            acc.add_record(&cancelled);
        }
        let report = Report::build(
            &Output {
                acc,
                files: Vec::new(),
            },
            &ReportOptions {
                by: SortBy::Total,
                top: 10,
            },
        );
        assert_eq!(report.cancelled.total, 2);
        assert_eq!(report.cancelled.by_reason[0].reason, "statement_timeout");
        assert_eq!(report.cancelled.statements[0].cancelled, 2);
        assert!(report.cancelled.statements[0].text.contains("pg_sleep"));
        let slowest = &report.slowest;
        assert!(slowest.has_params);
        assert_eq!(slowest.rows.len(), 2);
        assert!(approx_eq(slowest.rows[0].duration_ms, 70.0));
        assert_eq!(slowest.rows[0].params.as_deref(), Some("$1 = '2'"));
    }

    #[test]
    fn errors_name_their_statements_and_keep_the_first_detail() {
        let mut acc = Accumulators::new(100, 60);
        for (second, sql) in [
            (1, "SELECT * FROM a"),
            (2, "SELECT * FROM a"),
            (3, "SELECT * FROM b"),
        ] {
            let mut error = LogRecord::new(at(second), Severity::Error, "deadlock detected");
            error.sqlstate = Some("40P01".to_owned());
            error.statement = Some(sql.to_owned());
            error.detail = Some(format!("Process {second} waits for ShareLock"));
            error.hint = Some("See the server log".to_owned());
            error.pid = Some(4_000 + u32::try_from(second).unwrap());
            acc.add_record(&error);
        }
        let report = Report::build(
            &Output {
                acc,
                files: Vec::new(),
            },
            &ReportOptions {
                by: SortBy::Total,
                top: 10,
            },
        );
        let row = &report.errors[0];
        assert_eq!(row.count, 3);
        assert_eq!(row.detail.as_deref(), Some("Process 1 waits for ShareLock"));
        assert_eq!(row.hint.as_deref(), Some("See the server log"));
        assert_eq!(row.pid, Some(4_001));
        let counts: Vec<u64> = row.statements.iter().map(|s| s.count).collect();
        assert_eq!(counts, [2, 1]);
    }
}
