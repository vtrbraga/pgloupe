//! The server-wide picture from `pg_stat_database`: load, health and how much of it
//! `pg_stat_statements` explains.

use serde::Serialize;

use super::Finding;
use crate::stats::{
    database::{Change, DatabaseDelta},
    delta::Delta,
};

/// Databases listed in the report, busiest first.
const LISTED: usize = 20;
/// `pg_stat_statements` explaining less than this share of the active time is worth a warning.
const LOW_COVERAGE_PCT: f64 = 50.0;
/// Rollbacks above this share of all transactions are worth mentioning.
const ROLLBACK_PCT: f64 = 5.0;
/// Sessions that ended without the client leaving cleanly, above this share, are worth mentioning.
const ABANDONED_PCT: f64 = 20.0;

/// One database (or all of them) over the window.
#[derive(Debug, Clone, Serialize)]
pub struct DatabaseRow {
    pub name: String,
    pub commits: u64,
    pub rollbacks: u64,
    /// Rollbacks as a share of commits plus rollbacks.
    pub rollback_pct: Option<f64>,
    /// Blocks served from cache as a share of all blocks asked for.
    pub hit_pct: Option<f64>,
    pub blocks_read: u64,
    pub rows_returned: u64,
    pub rows_written: u64,
    pub temp_files: u64,
    pub temp_bytes: u64,
    pub deadlocks: u64,
    pub conflicts: u64,
    pub sessions: u64,
    pub sessions_abandoned: u64,
    pub sessions_fatal: u64,
    pub sessions_killed: u64,
    /// Abandoned sessions as a share of sessions that started.
    pub abandoned_pct: Option<f64>,
    /// Average number of sessions running statements, idle in a transaction, and connected.
    pub avg_active: f64,
    pub avg_idle_in_transaction: f64,
    pub avg_sessions: f64,
    /// Backends connected when the second snapshot was taken.
    pub backends_now: u64,
    /// Time reading and writing data files, in milliseconds (`track_io_timing`).
    pub io_read_ms: f64,
    pub io_write_ms: f64,
    /// A reset or a new database: the numbers are a lower bound.
    pub partial: bool,
}

/// What `pg_stat_database` says about the window.
#[derive(Debug, Clone, Serialize)]
pub struct DatabaseReport {
    pub before_at: String,
    pub after_at: String,
    pub window_secs: f64,
    pub total: DatabaseRow,
    /// The busiest databases by time spent running statements.
    pub databases: Vec<DatabaseRow>,
    /// Databases beyond the listed ones.
    pub more_databases: usize,
    /// Share of the server's active time that `pg_stat_statements` accounts for.
    pub statements_coverage_pct: Option<f64>,
}

fn pct(part: u64, whole: u64) -> Option<f64> {
    #[expect(clippy::cast_precision_loss, reason = "counters stay far below 2^53")]
    (whole > 0).then(|| part as f64 * 100.0 / whole as f64)
}

fn row(change: &Change, window_ms: f64) -> DatabaseRow {
    let per_window = |ms: f64| if window_ms > 0.0 { ms / window_ms } else { 0.0 };
    DatabaseRow {
        name: change.name.clone(),
        commits: change.commits,
        rollbacks: change.rollbacks,
        rollback_pct: pct(change.rollbacks, change.commits + change.rollbacks),
        hit_pct: pct(change.blks_hit, change.blks_hit + change.blks_read),
        blocks_read: change.blks_read,
        rows_returned: change.rows_returned,
        rows_written: change.rows_written,
        temp_files: change.temp_files,
        temp_bytes: change.temp_bytes,
        deadlocks: change.deadlocks,
        conflicts: change.conflicts,
        sessions: change.sessions,
        sessions_abandoned: change.sessions_abandoned,
        sessions_fatal: change.sessions_fatal,
        sessions_killed: change.sessions_killed,
        abandoned_pct: pct(change.sessions_abandoned, change.sessions),
        avg_active: per_window(change.active_ms),
        avg_idle_in_transaction: per_window(change.idle_in_transaction_ms),
        avg_sessions: per_window(change.session_ms),
        backends_now: change.backends_now,
        io_read_ms: change.blk_read_ms,
        io_write_ms: change.blk_write_ms,
        partial: change.partial,
    }
}

/// Builds the section for `delta`, with the coverage of `statements` when there are any.
#[must_use]
pub fn database_report(delta: &DatabaseDelta, statements: Option<&Delta>) -> DatabaseReport {
    let window_ms = delta.window_secs * 1000.0;
    let statements_coverage_pct = statements.and_then(|s| {
        let seen: f64 = s.rows.iter().map(|r| r.delta.total_exec_time).sum();
        (delta.total.active_ms > 0.0 && !delta.total.partial)
            .then(|| seen * 100.0 / delta.total.active_ms)
    });
    DatabaseReport {
        before_at: delta.before_at.to_string(),
        after_at: delta.after_at.to_string(),
        window_secs: delta.window_secs,
        total: row(&delta.total, window_ms),
        databases: delta
            .databases
            .iter()
            .take(LISTED)
            .map(|c| row(c, window_ms))
            .collect(),
        more_databases: delta.databases.len().saturating_sub(LISTED),
        statements_coverage_pct,
    }
}

fn finding(code: &'static str, severity: &'static str, message: String) -> Finding {
    Finding {
        code,
        severity,
        message,
        statements: Vec::new(),
    }
}

/// "about 8%", or "under 1%" instead of a misleading "0%".
pub(super) fn about(percent: f64) -> String {
    if percent < 1.0 {
        "under 1%".to_owned()
    } else {
        format!("about {percent:.0}%")
    }
}

/// What the server-wide numbers say that a reader should know.
#[must_use]
pub fn database_findings(report: &DatabaseReport, statements: Option<&Delta>) -> Vec<Finding> {
    let mut found = Vec::new();
    let total = &report.total;
    if total.partial {
        found.push(finding(
            "database_stats_partial",
            "info",
            "pg_stat_database counters were reset between the snapshots, or a database is new: its numbers are what the second snapshot holds, a lower bound".to_owned(),
        ));
    }
    if let Some(coverage) = report.statements_coverage_pct
        && coverage < LOW_COVERAGE_PCT
    {
        let evicted = statements
            .and_then(|s| s.evictions)
            .filter(|rounds| *rounds > 0)
            .map(|rounds| {
                format!(
                    " Entries were evicted in {rounds} rounds during the window, which is the usual cause."
                )
            })
            .unwrap_or_default();
        found.push(finding(
            "statements_cover_little",
            "warn",
            format!(
                "pg_stat_statements accounts for {} of the time the server spent running statements, so the statement ranking describes a small part of the load.{evicted}",
                about(coverage)
            ),
        ));
    }
    if let Some(rollback) = total.rollback_pct
        && rollback >= ROLLBACK_PCT
        && total.rollbacks >= 100
    {
        found.push(finding(
            "many_rollbacks",
            "info",
            format!(
                "{rollback:.1}% of transactions ({}) were rolled back. Errors, deadlocks and clients that abandon transactions all show up here.",
                total.rollbacks
            ),
        ));
    }
    if let Some(abandoned) = total.abandoned_pct
        && abandoned >= ABANDONED_PCT
        && total.sessions_abandoned >= 50
    {
        found.push(finding(
            "sessions_abandoned",
            "info",
            format!(
                "{} of {} sessions ({abandoned:.0}%) ended without the client leaving cleanly: connections dropped by a pooler, a proxy or a health check, or clients that crashed.",
                total.sessions_abandoned, total.sessions
            ),
        ));
    }
    if total.avg_idle_in_transaction >= 1.0 {
        found.push(finding(
            "idle_in_transaction",
            "info",
            format!(
                "on average {:.1} sessions sat idle inside a transaction. They hold locks and keep vacuum from cleaning up; idle_in_transaction_session_timeout ends them.",
                total.avg_idle_in_transaction
            ),
        ));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::database::{DatabaseSnapshot, diff};

    const HEADER: &str = "snapshot_at,datid,datname,numbackends,xact_commit,xact_rollback,blks_read,blks_hit,tup_returned,tup_fetched,tup_inserted,tup_updated,tup_deleted,conflicts,temp_files,temp_bytes,deadlocks,checksum_failures,checksum_last_failure,blk_read_time,blk_write_time,session_time,active_time,idle_in_transaction_time,sessions,sessions_abandoned,sessions_fatal,sessions_killed,stats_reset";

    /// What one database's counters were at one moment.
    struct Moment<'a> {
        at: &'a str,
        name: &'a str,
        commit: i64,
        rollback: i64,
        active: f64,
        idle: f64,
        sessions: i64,
        abandoned: i64,
    }

    fn snap(m: &Moment<'_>) -> DatabaseSnapshot {
        let row = format!(
            "{},1,{},5,{},{},100,900,0,0,0,0,0,0,0,0,0,0,,0,0,3600000,{},{},{},{},0,0,2026-10-01 00:00:00+00",
            m.at, m.name, m.commit, m.rollback, m.active, m.idle, m.sessions, m.abandoned
        );
        DatabaseSnapshot::from_reader(format!("{HEADER}\n{row}\n").as_bytes(), "t.csv").unwrap()
    }

    fn moment(
        at: &str,
        commit: i64,
        rollback: i64,
        active: f64,
        idle: f64,
        sessions: i64,
        abandoned: i64,
    ) -> Moment<'_> {
        Moment {
            at,
            name: "shop",
            commit,
            rollback,
            active,
            idle,
            sessions,
            abandoned,
        }
    }

    #[test]
    fn rates_and_averages_are_per_window() {
        let before = snap(&moment("2026-10-14 10:00:00+00", 0, 0, 0.0, 0.0, 0, 0));
        // One hour: 1,800 s active is 0.5 backends; 7,200 s idle in a transaction is 2.
        let after = snap(&moment(
            "2026-10-14 11:00:00+00",
            800,
            200,
            1_800_000.0,
            7_200_000.0,
            100,
            60,
        ));
        let report = database_report(&diff(&before, &after).unwrap(), None);
        let total = &report.total;
        assert!((total.rollback_pct.unwrap() - 20.0).abs() < 1e-9);
        assert!((total.avg_active - 0.5).abs() < 1e-9);
        assert!((total.avg_idle_in_transaction - 2.0).abs() < 1e-9);
        assert!((total.abandoned_pct.unwrap() - 60.0).abs() < 1e-9);
        assert!(total.hit_pct.is_none(), "no blocks moved in the window");
        let codes: Vec<&str> = database_findings(&report, None)
            .iter()
            .map(|f| f.code)
            .collect();
        assert_eq!(
            codes,
            [
                "many_rollbacks",
                "sessions_abandoned",
                "idle_in_transaction"
            ]
        );
    }

    #[test]
    fn the_cache_hit_share_counts_blocks_served_from_memory() {
        let change = Change {
            blks_hit: 90,
            blks_read: 10,
            ..Change::default()
        };
        let shown = row(&change, 1000.0);
        assert!((shown.hit_pct.unwrap() - 90.0).abs() < 1e-9);
        assert!(row(&Change::default(), 1000.0).hit_pct.is_none());
    }

    #[test]
    fn a_quiet_server_has_nothing_to_say() {
        let before = snap(&moment("2026-10-14 10:00:00+00", 0, 0, 0.0, 0.0, 0, 0));
        let after = snap(&moment(
            "2026-10-14 11:00:00+00",
            10_000,
            10,
            1000.0,
            10.0,
            10,
            0,
        ));
        let report = database_report(&diff(&before, &after).unwrap(), None);
        let found = database_findings(&report, None);
        assert_eq!(found.len(), 0, "{found:?}");
    }

    #[test]
    fn statements_covering_little_of_the_active_time_is_a_warning() {
        let before = snap(&moment("2026-10-14 10:00:00+00", 0, 0, 0.0, 0.0, 0, 0));
        let after = snap(&moment(
            "2026-10-14 11:00:00+00",
            10_000,
            0,
            1_000_000.0,
            0.0,
            10,
            0,
        ));
        let mut statements = crate::combined::testkit::delta(&[(
            crate::testutil::key(1),
            Some("SELECT 1"),
            10,
            80_000.0,
        )]);
        statements.evictions = Some(12);
        let report = database_report(&diff(&before, &after).unwrap(), Some(&statements));
        let coverage = report.statements_coverage_pct.unwrap();
        assert!((coverage - 8.0).abs() < 0.01, "{coverage}");
        let found = database_findings(&report, Some(&statements));
        let f = found
            .iter()
            .find(|f| f.code == "statements_cover_little")
            .unwrap();
        assert_eq!(f.severity, "warn");
        assert!(f.message.contains("12 rounds"), "{}", f.message);
    }
}
