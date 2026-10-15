//! `pg_stat_database` snapshots and the change between two of them: how busy and how healthy the
//! server was, whatever `pg_stat_statements` saw of it.

use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

use jiff::Timestamp;

/// Why a database snapshot could not be read or compared.
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("{file}: {message}")]
    Read { file: String, message: String },
    #[error("the second pg_stat_database snapshot is not later than the first")]
    Order,
}

/// The counters of one database at one moment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counters {
    pub numbackends: i64,
    pub xact_commit: i64,
    pub xact_rollback: i64,
    pub blks_read: i64,
    pub blks_hit: i64,
    pub tup_returned: i64,
    pub tup_fetched: i64,
    pub tup_inserted: i64,
    pub tup_updated: i64,
    pub tup_deleted: i64,
    pub conflicts: i64,
    pub temp_files: i64,
    pub temp_bytes: i64,
    pub deadlocks: i64,
    /// Milliseconds spent reading and writing data files (`track_io_timing`).
    pub blk_read_time: f64,
    pub blk_write_time: f64,
    /// Milliseconds sessions were connected, running statements, and idle in a transaction.
    pub session_time: f64,
    pub active_time: f64,
    pub idle_in_transaction_time: f64,
    pub sessions: i64,
    pub sessions_abandoned: i64,
    pub sessions_fatal: i64,
    pub sessions_killed: i64,
    pub stats_reset: Option<Timestamp>,
}

/// Every database of the server at one moment.
#[derive(Debug, Clone)]
pub struct DatabaseSnapshot {
    pub taken_at: Timestamp,
    pub databases: BTreeMap<String, Counters>,
}

impl DatabaseSnapshot {
    /// Reads a `pg_stat_database` export (`snap` writes one beside each statistics snapshot).
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be opened or parsed, lacks `snapshot_at`, `datname` or the
    /// transaction counters, or has no rows.
    pub fn from_path(path: &Path) -> Result<Self, DatabaseError> {
        let label = path.display().to_string();
        let file = File::open(path).map_err(|e| DatabaseError::Read {
            file: label.clone(),
            message: e.to_string(),
        })?;
        Self::from_reader(file, &label)
    }

    /// Like [`DatabaseSnapshot::from_path`], over any reader.
    ///
    /// # Errors
    ///
    /// See [`DatabaseSnapshot::from_path`].
    pub fn from_reader(reader: impl Read, label: &str) -> Result<Self, DatabaseError> {
        let fail = |message: String| DatabaseError::Read {
            file: label.to_owned(),
            message,
        };
        let mut rdr = csv::Reader::from_reader(reader);
        let headers = rdr.headers().map_err(|e| fail(e.to_string()))?.clone();
        let column = |name: &str| headers.iter().position(|h| h == name);
        for needed in ["snapshot_at", "datname", "xact_commit", "xact_rollback"] {
            if column(needed).is_none() {
                return Err(fail(format!("the column {needed} is missing")));
            }
        }
        let mut taken_at: Option<Timestamp> = None;
        let mut databases = BTreeMap::new();
        for (index, record) in rdr.records().enumerate() {
            let record = record.map_err(|e| fail(e.to_string()))?;
            let line = index + 2;
            let text = |name: &str| column(name).and_then(|i| record.get(i)).unwrap_or("");
            let whole = |name: &str| text(name).trim().parse::<i64>().unwrap_or(0);
            let real = |name: &str| text(name).trim().parse::<f64>().unwrap_or(0.0);
            let at = text("snapshot_at")
                .parse::<Timestamp>()
                .map_err(|e| fail(format!("line {line}: snapshot_at: {e}")))?;
            taken_at = Some(taken_at.map_or(at, |t| t.max(at)));
            let name = text("datname");
            if name.is_empty() {
                continue;
            }
            databases.insert(
                name.to_owned(),
                Counters {
                    numbackends: whole("numbackends"),
                    xact_commit: whole("xact_commit"),
                    xact_rollback: whole("xact_rollback"),
                    blks_read: whole("blks_read"),
                    blks_hit: whole("blks_hit"),
                    tup_returned: whole("tup_returned"),
                    tup_fetched: whole("tup_fetched"),
                    tup_inserted: whole("tup_inserted"),
                    tup_updated: whole("tup_updated"),
                    tup_deleted: whole("tup_deleted"),
                    conflicts: whole("conflicts"),
                    temp_files: whole("temp_files"),
                    temp_bytes: whole("temp_bytes"),
                    deadlocks: whole("deadlocks"),
                    blk_read_time: real("blk_read_time"),
                    blk_write_time: real("blk_write_time"),
                    session_time: real("session_time"),
                    active_time: real("active_time"),
                    idle_in_transaction_time: real("idle_in_transaction_time"),
                    sessions: whole("sessions"),
                    sessions_abandoned: whole("sessions_abandoned"),
                    sessions_fatal: whole("sessions_fatal"),
                    sessions_killed: whole("sessions_killed"),
                    stats_reset: text("stats_reset").parse::<Timestamp>().ok(),
                },
            );
        }
        let taken_at = taken_at.ok_or_else(|| fail("the file has no rows".to_owned()))?;
        Ok(DatabaseSnapshot {
            taken_at,
            databases,
        })
    }
}

/// What one database did between two snapshots.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Change {
    pub name: String,
    pub commits: u64,
    pub rollbacks: u64,
    pub blks_read: u64,
    pub blks_hit: u64,
    pub rows_returned: u64,
    pub rows_fetched: u64,
    pub rows_written: u64,
    pub conflicts: u64,
    pub temp_files: u64,
    pub temp_bytes: u64,
    pub deadlocks: u64,
    pub blk_read_ms: f64,
    pub blk_write_ms: f64,
    pub session_ms: f64,
    pub active_ms: f64,
    pub idle_in_transaction_ms: f64,
    pub sessions: u64,
    pub sessions_abandoned: u64,
    pub sessions_fatal: u64,
    pub sessions_killed: u64,
    /// Backends connected when the second snapshot was taken.
    pub backends_now: u64,
    /// The counters were reset between the snapshots (or the database is new): the change is
    /// what the second snapshot holds, a lower bound.
    pub partial: bool,
}

/// The change in every database, and the server as a whole.
#[derive(Debug, Clone)]
pub struct DatabaseDelta {
    pub before_at: Timestamp,
    pub after_at: Timestamp,
    /// Seconds between the snapshots.
    pub window_secs: f64,
    /// Busiest first (by active time, then name).
    pub databases: Vec<Change>,
    /// All databases added up; `partial` when any of them is.
    pub total: Change,
}

fn up(after: i64, before: i64) -> u64 {
    u64::try_from(after.saturating_sub(before)).unwrap_or(0)
}

fn up_ms(after: f64, before: f64) -> f64 {
    (after - before).max(0.0)
}

fn change(name: &str, after: &Counters, before: Option<&Counters>) -> Change {
    // A counter that went down, or a changed reset time, means the statistics were reset.
    let partial = before.is_none_or(|b| {
        after.xact_commit < b.xact_commit
            || after.sessions < b.sessions
            || (after.stats_reset != b.stats_reset && after.stats_reset.is_some())
    });
    let zero = Counters::default();
    let b = if partial {
        &zero
    } else {
        before.unwrap_or(&zero)
    };
    Change {
        name: name.to_owned(),
        commits: up(after.xact_commit, b.xact_commit),
        rollbacks: up(after.xact_rollback, b.xact_rollback),
        blks_read: up(after.blks_read, b.blks_read),
        blks_hit: up(after.blks_hit, b.blks_hit),
        rows_returned: up(after.tup_returned, b.tup_returned),
        rows_fetched: up(after.tup_fetched, b.tup_fetched),
        rows_written: up(after.tup_inserted, b.tup_inserted)
            + up(after.tup_updated, b.tup_updated)
            + up(after.tup_deleted, b.tup_deleted),
        conflicts: up(after.conflicts, b.conflicts),
        temp_files: up(after.temp_files, b.temp_files),
        temp_bytes: up(after.temp_bytes, b.temp_bytes),
        deadlocks: up(after.deadlocks, b.deadlocks),
        blk_read_ms: up_ms(after.blk_read_time, b.blk_read_time),
        blk_write_ms: up_ms(after.blk_write_time, b.blk_write_time),
        session_ms: up_ms(after.session_time, b.session_time),
        active_ms: up_ms(after.active_time, b.active_time),
        idle_in_transaction_ms: up_ms(after.idle_in_transaction_time, b.idle_in_transaction_time),
        sessions: up(after.sessions, b.sessions),
        sessions_abandoned: up(after.sessions_abandoned, b.sessions_abandoned),
        sessions_fatal: up(after.sessions_fatal, b.sessions_fatal),
        sessions_killed: up(after.sessions_killed, b.sessions_killed),
        backends_now: u64::try_from(after.numbackends).unwrap_or(0),
        partial,
    }
}

/// What changed between two snapshots.
///
/// # Errors
///
/// Fails when `after` is not later than `before`.
pub fn diff(
    before: &DatabaseSnapshot,
    after: &DatabaseSnapshot,
) -> Result<DatabaseDelta, DatabaseError> {
    let seconds = after.taken_at.as_second() - before.taken_at.as_second();
    if seconds <= 0 {
        return Err(DatabaseError::Order);
    }
    let mut databases: Vec<Change> = after
        .databases
        .iter()
        .map(|(name, counters)| change(name, counters, before.databases.get(name)))
        .collect();
    databases.sort_by(|a, b| {
        b.active_ms
            .total_cmp(&a.active_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut total = Change {
        name: "(all databases)".to_owned(),
        ..Change::default()
    };
    for c in &databases {
        total.commits += c.commits;
        total.rollbacks += c.rollbacks;
        total.blks_read += c.blks_read;
        total.blks_hit += c.blks_hit;
        total.rows_returned += c.rows_returned;
        total.rows_fetched += c.rows_fetched;
        total.rows_written += c.rows_written;
        total.conflicts += c.conflicts;
        total.temp_files += c.temp_files;
        total.temp_bytes += c.temp_bytes;
        total.deadlocks += c.deadlocks;
        total.blk_read_ms += c.blk_read_ms;
        total.blk_write_ms += c.blk_write_ms;
        total.session_ms += c.session_ms;
        total.active_ms += c.active_ms;
        total.idle_in_transaction_ms += c.idle_in_transaction_ms;
        total.sessions += c.sessions;
        total.sessions_abandoned += c.sessions_abandoned;
        total.sessions_fatal += c.sessions_fatal;
        total.sessions_killed += c.sessions_killed;
        total.backends_now += c.backends_now;
        total.partial |= c.partial;
    }
    #[expect(clippy::cast_precision_loss, reason = "a window of seconds")]
    let window_secs = seconds as f64;
    Ok(DatabaseDelta {
        before_at: before.taken_at,
        after_at: after.taken_at,
        window_secs,
        databases,
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "snapshot_at,datid,datname,numbackends,xact_commit,xact_rollback,blks_read,blks_hit,tup_returned,tup_fetched,tup_inserted,tup_updated,tup_deleted,conflicts,temp_files,temp_bytes,deadlocks,checksum_failures,checksum_last_failure,blk_read_time,blk_write_time,session_time,active_time,idle_in_transaction_time,sessions,sessions_abandoned,sessions_fatal,sessions_killed,stats_reset";

    fn row(at: &str, name: &str, commit: i64, rollback: i64, active: f64, sessions: i64) -> String {
        format!(
            "{at},1,{name},5,{commit},{rollback},100,900,1000,500,10,20,30,0,2,4096,1,0,,12.5,3.5,60000,{active},2000,{sessions},1,0,0,2026-10-01 00:00:00+00"
        )
    }

    fn snapshot(rows: &[String]) -> DatabaseSnapshot {
        let text = format!("{HEADER}\n{}\n", rows.join("\n"));
        DatabaseSnapshot::from_reader(text.as_bytes(), "test.csv").unwrap()
    }

    #[test]
    fn a_window_of_counters_becomes_changes_per_database_and_in_total() {
        let before = snapshot(&[
            row("2026-10-14 10:00:00+00", "shop", 1000, 10, 10_000.0, 100),
            row("2026-10-14 10:00:00+00", "billing", 50, 0, 100.0, 5),
        ]);
        let after = snapshot(&[
            row("2026-10-14 11:00:00+00", "shop", 4600, 410, 70_000.0, 160),
            row("2026-10-14 11:00:00+00", "billing", 80, 0, 500.0, 6),
        ]);
        let delta = diff(&before, &after).unwrap();
        assert!((delta.window_secs - 3600.0).abs() < f64::EPSILON);
        let shop = &delta.databases[0];
        assert_eq!(shop.name, "shop", "busiest first");
        assert_eq!((shop.commits, shop.rollbacks), (3600, 400));
        assert!((shop.active_ms - 60_000.0).abs() < 1e-9);
        assert_eq!(shop.sessions, 60);
        assert!(!shop.partial);
        assert_eq!(delta.total.commits, 3630);
        assert_eq!(delta.total.rollbacks, 400);
        assert_eq!(delta.total.sessions, 61);
    }

    #[test]
    fn a_reset_or_a_new_database_makes_the_change_a_lower_bound() {
        let before = snapshot(&[row(
            "2026-10-14 10:00:00+00",
            "shop",
            1000,
            10,
            10_000.0,
            100,
        )]);
        let after = snapshot(&[
            row("2026-10-14 11:00:00+00", "shop", 20, 1, 5.0, 3),
            row("2026-10-14 11:00:00+00", "fresh", 7, 0, 1.0, 1),
        ]);
        let delta = diff(&before, &after).unwrap();
        assert!(delta.databases.iter().all(|d| d.partial));
        assert!(delta.total.partial);
        let shop = delta.databases.iter().find(|d| d.name == "shop").unwrap();
        assert_eq!(shop.commits, 20, "what the second snapshot holds");
    }

    #[test]
    fn snapshots_out_of_order_or_without_columns_are_refused() {
        let a = snapshot(&[row("2026-10-14 11:00:00+00", "shop", 1, 0, 1.0, 1)]);
        let b = snapshot(&[row("2026-10-14 10:00:00+00", "shop", 1, 0, 1.0, 1)]);
        assert!(matches!(diff(&a, &b), Err(DatabaseError::Order)));
        let err = DatabaseSnapshot::from_reader("a,b\n1,2\n".as_bytes(), "x.csv").unwrap_err();
        assert!(err.to_string().contains("snapshot_at"), "{err}");
    }
}
