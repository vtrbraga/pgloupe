//! The combined report: log statistics joined with `pg_stat_statements` deltas.

mod join;

pub use join::{Joined, join, rank};

use serde::Serialize;

use crate::analyze::report::StatementRow;

/// One statement as seen by the logs, by `pg_stat_statements`, or both.
#[derive(Debug, Clone, Serialize)]
pub struct JoinedRow {
    /// 16 hex digits, or `q<queryid>` for a statistics entry whose text could not be fingerprinted.
    pub fingerprint: String,
    pub text: String,
    /// `both`, `log` or `stats`.
    pub link: &'static str,
    pub log: Option<StatementRow>,
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
    pub calls: u64,
    pub total_exec_ms: f64,
    pub mean_ms: Option<f64>,
    pub rows: u64,
    pub plans: u64,
    pub shared_blks_hit: u64,
    pub shared_blks_read: u64,
    pub temp_blks_read: u64,
    pub temp_blks_written: u64,
    pub wal_bytes: u64,
    pub share_pct: f64,
    /// Statistics entries merged into this row.
    pub entries: usize,
    /// Distinct `queryid`s among them.
    pub queryid_count: usize,
    /// The first five, ascending, as text (64-bit ids do not survive JSON numbers).
    pub queryids: Vec<String>,
    /// `new`, `restarted`, `partial`, `evicted_suspect`.
    pub flags: Vec<&'static str>,
}
