//! Joining the log's statements with the statistics' entries by fingerprint.

use std::collections::{BTreeMap, BTreeSet};

use super::{JoinedRow, StatsSide};
use crate::{
    analyze::{
        fingerprint::fingerprint,
        report::{SortBy, StatementRow},
    },
    stats::{
        delta::{Delta, Row, Status},
        snapshot::Counters,
    },
};

/// `queryid`s listed per row.
const MAX_QUERYIDS: usize = 5;
/// Statistics text kept for a row that has no log side.
const MAX_TEXT_CHARS: usize = 4_096;
/// What `pg_stat_statements` shows a role that may not read other roles' queries.
const NO_PRIVILEGE: &str = "<insufficient privilege>";

/// The result of a join.
#[derive(Debug)]
pub struct Joined {
    /// Every statement seen by either side, in fingerprint order (rank them with [`rank`]).
    pub rows: Vec<JoinedRow>,
    /// Statistics entries with calls in the window whose text could not be fingerprinted.
    pub unusable_entries: usize,
    /// Their summed execution time, in milliseconds.
    pub unusable_exec_ms: f64,
    /// Fingerprints that merged more than one `queryid`.
    pub rollups: usize,
}

/// Statistics entries that share a fingerprint.
#[derive(Default)]
struct Group {
    text: String,
    queryids: BTreeSet<i64>,
    entries: usize,
    delta: Counters,
    share_pct: f64,
    flags: BTreeSet<&'static str>,
}

impl Group {
    fn add(&mut self, row: &Row) {
        if self.entries == 0 {
            self.text = cut(row.query.as_deref().unwrap_or_default());
        }
        self.entries += 1;
        self.queryids.insert(row.key.queryid);
        let d = &row.delta;
        self.delta.calls += d.calls;
        self.delta.rows += d.rows;
        self.delta.plans += d.plans;
        self.delta.total_plan_time += d.total_plan_time;
        self.delta.total_exec_time += d.total_exec_time;
        self.delta.shared_blks_hit += d.shared_blks_hit;
        self.delta.shared_blks_read += d.shared_blks_read;
        self.delta.temp_blks_read += d.temp_blks_read;
        self.delta.temp_blks_written += d.temp_blks_written;
        self.delta.wal_bytes += d.wal_bytes;
        self.share_pct += row.share_pct;
        match row.status {
            Status::New => {
                self.flags.insert("new");
            }
            Status::Restarted => {
                self.flags.insert("restarted");
            }
            Status::Ok => {}
        }
        if row.partial {
            self.flags.insert("partial");
        }
        if row.evicted_suspect {
            self.flags.insert("evicted_suspect");
        }
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "call counts stay far below 2^53"
    )]
    fn side(&self) -> StatsSide {
        let calls = self.delta.calls;
        StatsSide {
            calls,
            total_exec_ms: self.delta.total_exec_time,
            mean_ms: (calls > 0).then(|| self.delta.total_exec_time / calls as f64),
            rows: self.delta.rows,
            plans: self.delta.plans,
            shared_blks_hit: self.delta.shared_blks_hit,
            shared_blks_read: self.delta.shared_blks_read,
            temp_blks_read: self.delta.temp_blks_read,
            temp_blks_written: self.delta.temp_blks_written,
            wal_bytes: self.delta.wal_bytes,
            share_pct: self.share_pct,
            entries: self.entries,
            queryid_count: self.queryids.len(),
            queryids: self
                .queryids
                .iter()
                .take(MAX_QUERYIDS)
                .map(ToString::to_string)
                .collect(),
            flags: self.flags.iter().copied().collect(),
        }
    }
}

fn cut(text: &str) -> String {
    text.chars().take(MAX_TEXT_CHARS).collect()
}

/// The group an entry belongs to: its text's fingerprint, or `q<queryid>` when the text is
/// missing, withheld or not parsable. The flag says whether the text was usable.
fn group_key(row: &Row) -> (String, bool) {
    let text = row.query.as_deref().map(str::trim).unwrap_or_default();
    if !text.is_empty() && text != NO_PRIVILEGE {
        let found = fingerprint(text);
        if found.parsed {
            return (format!("{:016x}", found.id), true);
        }
    }
    (format!("q{}", row.key.queryid), false)
}

/// Joins `log` (one row per fingerprint) with the entries of `delta` that had calls in the window.
#[must_use]
pub fn join(log: Vec<StatementRow>, delta: Option<&Delta>) -> Joined {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let (mut unusable_entries, mut unusable_exec_ms) = (0, 0.0);
    for row in delta.into_iter().flat_map(|delta| &delta.rows) {
        if row.delta.calls == 0 {
            continue;
        }
        let (key, usable) = group_key(row);
        if !usable {
            unusable_entries += 1;
            unusable_exec_ms += row.delta.total_exec_time;
        }
        groups.entry(key).or_default().add(row);
    }
    let rollups = groups
        .iter()
        .filter(|(key, group)| !key.starts_with('q') && group.queryids.len() > 1)
        .count();
    let mut logs: BTreeMap<String, StatementRow> = log
        .into_iter()
        .map(|row| (row.fingerprint.clone(), row))
        .collect();
    let keys: BTreeSet<String> = groups.keys().chain(logs.keys()).cloned().collect();
    let rows = keys
        .into_iter()
        .map(|key| {
            let log = logs.remove(&key);
            let stats = groups.get(&key);
            joined_row(key, log, stats)
        })
        .collect();
    Joined {
        rows,
        unusable_entries,
        unusable_exec_ms,
        rollups,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "call counts stay far below 2^53"
)]
fn joined_row(fingerprint: String, log: Option<StatementRow>, group: Option<&Group>) -> JoinedRow {
    let stats = group.map(Group::side);
    let coverage = match (&log, &stats) {
        (Some(log), Some(stats)) if stats.calls > 0 => {
            Some(log.timed_calls as f64 / stats.calls as f64)
        }
        _ => None,
    };
    let query_id_matches = match (&log, group) {
        (Some(log), Some(group)) => log
            .query_id
            .as_deref()
            .and_then(|id| id.parse::<i64>().ok())
            .map(|id| group.queryids.contains(&id)),
        _ => None,
    };
    let text = log
        .as_ref()
        .map(|log| log.text.clone())
        .or_else(|| group.map(|group| group.text.clone()))
        .unwrap_or_default();
    JoinedRow {
        fingerprint,
        text,
        link: match (&log, &stats) {
            (Some(_), Some(_)) => "both",
            (Some(_), None) => "log",
            _ => "stats",
        },
        log,
        stats,
        coverage,
        query_id_matches,
    }
}

/// What `rank` orders by: the statistics value when the row has one (for `Total` and `Calls`),
/// else the log's; `P95` and `Max` only exist on the log side.
#[expect(
    clippy::cast_precision_loss,
    reason = "call counts stay far below 2^53"
)]
fn rank_value(row: &JoinedRow, by: SortBy) -> Option<f64> {
    match by {
        SortBy::Total => row
            .stats
            .as_ref()
            .map(|stats| stats.total_exec_ms)
            .or_else(|| row.log.as_ref().map(|log| log.total_ms)),
        SortBy::Calls => row
            .stats
            .as_ref()
            .map(|stats| stats.calls as f64)
            .or_else(|| row.log.as_ref().map(|log| log.calls as f64)),
        SortBy::P95 => row.log.as_ref().and_then(|log| log.p95_ms),
        SortBy::Max => row.log.as_ref().and_then(|log| log.max_ms),
    }
}

/// Orders `rows` by `by`, largest first; rows without a value go last; ties by fingerprint.
pub fn rank(rows: &mut [JoinedRow], by: SortBy) {
    rows.sort_by(|a, b| {
        match (rank_value(a, by), rank_value(b, by)) {
            (Some(x), Some(y)) => y.total_cmp(&x),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        analyze::{acc::Accumulators, report::all_statements},
        log::{LogRecord, Severity},
        stats::{
            delta::diff,
            snapshot::{Entry, Key, Snapshot},
        },
        testutil::{entry, key, ts},
    };

    /// Log rows for `(sql, calls, ms each)`, through the real accumulators so ids are real.
    fn log_rows(statements: &[(&str, usize, f64)]) -> Vec<StatementRow> {
        let mut acc = Accumulators::new(1_000, 60);
        let mut second = 0;
        for (sql, calls, ms) in statements {
            for _ in 0..*calls {
                second += 1;
                acc.add_record(&LogRecord::new(
                    ts("2026-10-14T10:00:00Z") + std::time::Duration::from_secs(second),
                    Severity::Log,
                    format!("duration: {ms} ms  statement: {sql}"),
                ));
            }
        }
        all_statements(&acc)
    }

    fn with_text(text: Option<&str>, calls: u64, total: f64) -> Entry {
        Entry {
            query: text.map(str::to_owned),
            ..entry(calls, total)
        }
    }

    fn snapshot(at: &str, entries: Vec<(Key, Entry)>) -> Snapshot {
        Snapshot {
            taken_at: ts(at),
            server_version_num: 170_000,
            entries: entries.into_iter().collect(),
            skipped_no_queryid: 0,
            optional_columns: std::collections::BTreeSet::new(),
            info: None,
        }
    }

    /// A delta whose rows have the given `(key, text, calls, total ms)` as their window counts:
    /// `before` has the entries at zero calls so they are `Ok`, not `New`.
    fn delta(rows: &[(Key, Option<&str>, u64, f64)]) -> Delta {
        let before = rows
            .iter()
            .map(|(k, text, _, _)| (*k, with_text(*text, 1, 1.0)))
            .collect();
        let after = rows
            .iter()
            .map(|(k, text, calls, total)| (*k, with_text(*text, 1 + calls, 1.0 + total)))
            .collect();
        diff(
            &snapshot("2026-10-14T10:00:00Z", before),
            &snapshot("2026-10-14T11:00:00Z", after),
        )
        .unwrap()
    }

    fn user_key(queryid: i64, userid: u32) -> Key {
        Key {
            userid,
            ..key(queryid)
        }
    }

    const FILM: &str = "SELECT * FROM film WHERE film_id = $1";

    #[test]
    fn a_statement_on_both_sides_is_one_row() {
        let log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 3, 10.0)]);
        let d = delta(&[(key(1), Some(FILM), 10, 100.0)]);
        let joined = join(log, Some(&d));
        assert_eq!(joined.rows.len(), 1);
        let row = &joined.rows[0];
        assert_eq!(row.link, "both");
        assert_eq!(row.stats.as_ref().unwrap().calls, 10);
        assert_eq!(row.log.as_ref().unwrap().calls, 3);
        assert!(
            (row.coverage.unwrap() - 0.3).abs() < 1e-9,
            "{:?}",
            row.coverage
        );
        assert_eq!(row.text, row.log.as_ref().unwrap().text);
    }

    #[test]
    fn log_only_and_stats_only_rows_are_marked() {
        let log = log_rows(&[("SELECT * FROM actor", 2, 5.0)]);
        let d = delta(&[(key(1), Some(FILM), 10, 100.0)]);
        let joined = join(log, Some(&d));
        let links: Vec<_> = joined.rows.iter().map(|r| r.link).collect();
        assert!(
            links.contains(&"log") && links.contains(&"stats"),
            "{links:?}"
        );
        let stats_only = joined.rows.iter().find(|r| r.link == "stats").unwrap();
        assert!(stats_only.log.is_none() && stats_only.coverage.is_none());
        assert_eq!(stats_only.text, FILM);
        let log_only = joined.rows.iter().find(|r| r.link == "log").unwrap();
        assert!(log_only.stats.is_none());
    }

    #[test]
    fn without_statistics_every_row_is_log_only() {
        let joined = join(log_rows(&[("SELECT 1", 2, 1.0)]), None);
        assert_eq!(joined.rows.len(), 1);
        assert_eq!(joined.rows[0].link, "log");
    }

    #[test]
    fn partitions_roll_up_by_fingerprint() {
        let d = delta(&[
            (
                key(1),
                Some("SELECT * FROM events_p0001 WHERE id = $1"),
                10,
                10.0,
            ),
            (
                key(2),
                Some("SELECT * FROM events_p0002 WHERE id = $1"),
                20,
                20.0,
            ),
            (
                key(3),
                Some("SELECT * FROM events_p0003 WHERE id = $1"),
                30,
                30.0,
            ),
        ]);
        let joined = join(Vec::new(), Some(&d));
        assert_eq!(joined.rows.len(), 1);
        let stats = joined.rows[0].stats.as_ref().unwrap();
        assert_eq!(
            (stats.entries, stats.queryid_count, stats.calls),
            (3, 3, 60)
        );
        assert_eq!(stats.queryids, ["1", "2", "3"]);
        assert_eq!(joined.rollups, 1);
    }

    #[test]
    fn users_and_databases_are_summed_once() {
        let d = delta(&[
            (user_key(1, 10), Some(FILM), 5, 50.0),
            (user_key(1, 11), Some(FILM), 7, 70.0),
        ]);
        let joined = join(Vec::new(), Some(&d));
        let stats = joined.rows[0].stats.as_ref().unwrap();
        assert_eq!(
            (stats.entries, stats.queryid_count, stats.calls),
            (2, 1, 12)
        );
        assert!((stats.total_exec_ms - 120.0).abs() < 1e-9);
        assert!((stats.mean_ms.unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(joined.rollups, 0);
    }

    #[test]
    fn unusable_text_is_kept_and_counted() {
        let d = delta(&[
            (key(1), Some("<insufficient privilege>"), 4, 40.0),
            (key(2), None, 5, 50.0),
            (key(3), Some("SELEC 1"), 6, 60.0),
            (key(4), Some(FILM), 7, 70.0),
        ]);
        let log = log_rows(&[("SELECT * FROM film WHERE film_id = 9", 1, 1.0)]);
        let joined = join(log, Some(&d));
        assert_eq!(joined.unusable_entries, 3);
        assert!((joined.unusable_exec_ms - 150.0).abs() < 1e-9);
        let stats_only: Vec<_> = joined.rows.iter().filter(|r| r.link == "stats").collect();
        assert_eq!(stats_only.len(), 3);
        assert!(stats_only.iter().all(|r| r.fingerprint.starts_with('q')));
        assert_eq!(joined.rows.iter().filter(|r| r.link == "both").count(), 1);
    }

    #[test]
    fn entries_without_calls_in_the_window_are_ignored() {
        let d = delta(&[(key(1), Some(FILM), 0, 0.0)]);
        assert!(join(Vec::new(), Some(&d)).rows.is_empty());
    }

    #[test]
    fn the_logged_query_id_is_checked_against_the_entries() {
        let mut log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 1, 1.0)]);
        log[0].query_id = Some("1".to_owned());
        let hit = join(log.clone(), Some(&delta(&[(key(1), Some(FILM), 3, 3.0)])));
        assert_eq!(hit.rows[0].query_id_matches, Some(true));
        let miss = join(log, Some(&delta(&[(key(9), Some(FILM), 3, 3.0)])));
        assert_eq!(miss.rows[0].query_id_matches, Some(false));
        let none = join(
            log_rows(&[("SELECT * FROM film WHERE film_id = 5", 1, 1.0)]),
            Some(&delta(&[(key(9), Some(FILM), 3, 3.0)])),
        );
        assert_eq!(none.rows[0].query_id_matches, None);
    }

    #[test]
    fn flags_come_from_the_delta() {
        // Entry 1 exists only after (new); entry 2 restarts (counters go down).
        let before = snapshot(
            "2026-10-14T10:00:00Z",
            vec![(key(2), with_text(Some("SELECT * FROM bb"), 50, 500.0))],
        );
        let after = snapshot(
            "2026-10-14T11:00:00Z",
            vec![
                (key(1), with_text(Some("SELECT * FROM aa"), 5, 5.0)),
                (key(2), with_text(Some("SELECT * FROM bb"), 4, 4.0)),
            ],
        );
        let d = diff(&before, &after).unwrap();
        let joined = join(Vec::new(), Some(&d));
        let flags = |text: &str| {
            joined
                .rows
                .iter()
                .find(|r| r.text == text)
                .unwrap()
                .stats
                .as_ref()
                .unwrap()
                .flags
                .clone()
        };
        assert!(flags("SELECT * FROM aa").contains(&"new"));
        assert!(flags("SELECT * FROM bb").contains(&"restarted"));
    }

    #[test]
    fn ranking_follows_the_chosen_metric() {
        let log = log_rows(&[("SELECT * FROM a", 2, 500.0), ("SELECT * FROM b", 50, 1.0)]);
        let d = delta(&[
            (key(1), Some("SELECT * FROM c"), 1_000, 5_000.0),
            (key(2), Some("SELECT * FROM a"), 2, 1_000.0),
        ]);
        let mut joined = join(log, Some(&d)).rows;
        let order = |rows: &[JoinedRow]| rows.iter().map(|r| r.text.clone()).collect::<Vec<_>>();
        rank(&mut joined, SortBy::Total);
        assert_eq!(order(&joined)[0], "SELECT * FROM c");
        rank(&mut joined, SortBy::Calls);
        assert_eq!(order(&joined)[0], "SELECT * FROM c");
        rank(&mut joined, SortBy::Max);
        // `a` has max 500 ms, `b` 1 ms, `c` has no log side and goes last.
        assert_eq!(
            order(&joined),
            ["SELECT * FROM a", "SELECT * FROM b", "SELECT * FROM c"]
        );
        rank(&mut joined, SortBy::P95);
        assert_eq!(order(&joined).last().unwrap(), "SELECT * FROM c");
    }
}
