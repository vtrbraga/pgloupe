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

/// Statistics entries that share a fingerprint. Entries for statements run inside functions
/// (`toplevel = false`, only with `pg_stat_statements.track = all`) are kept apart: the log does
/// not record them, and their time is already part of their caller's.
#[derive(Default)]
struct Group {
    text: String,
    queryids: BTreeSet<i64>,
    entries: usize,
    top: Counters,
    top_entries: usize,
    nested: Counters,
    top_share_pct: f64,
    nested_share_pct: f64,
    flags: BTreeSet<&'static str>,
}

fn add_counters(into: &mut Counters, d: &Counters) {
    into.calls += d.calls;
    into.rows += d.rows;
    into.plans += d.plans;
    into.total_plan_time += d.total_plan_time;
    into.total_exec_time += d.total_exec_time;
    into.shared_blks_hit += d.shared_blks_hit;
    into.shared_blks_read += d.shared_blks_read;
    into.temp_blks_read += d.temp_blks_read;
    into.temp_blks_written += d.temp_blks_written;
    into.wal_bytes += d.wal_bytes;
}

impl Group {
    fn add(&mut self, row: &Row) {
        if self.entries == 0 {
            self.text = cut(row.query.as_deref().unwrap_or_default());
        }
        self.entries += 1;
        self.queryids.insert(row.key.queryid);
        if row.key.toplevel {
            self.top_entries += 1;
            add_counters(&mut self.top, &row.delta);
            self.top_share_pct += row.share_pct;
        } else {
            add_counters(&mut self.nested, &row.delta);
            self.nested_share_pct += row.share_pct;
        }
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

    /// Adds everything `other` knows; the text stays this group's unless it has none yet.
    fn merge(&mut self, other: Group) {
        if self.entries == 0 {
            self.text = other.text;
        }
        self.entries += other.entries;
        self.queryids.extend(other.queryids);
        add_counters(&mut self.top, &other.top);
        self.top_entries += other.top_entries;
        add_counters(&mut self.nested, &other.nested);
        self.top_share_pct += other.top_share_pct;
        self.nested_share_pct += other.nested_share_pct;
        self.flags.extend(other.flags);
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "call counts stay far below 2^53"
    )]
    fn side(&self) -> StatsSide {
        // Top-level entries speak for the statement; a statement seen only inside functions
        // speaks through its nested entries and says so.
        let nested_only = self.top_entries == 0;
        let (counted, share_pct) = if nested_only {
            (&self.nested, self.nested_share_pct)
        } else {
            (&self.top, self.top_share_pct)
        };
        let calls = counted.calls;
        let mut flags = self.flags.clone();
        if nested_only {
            flags.insert("nested");
        }
        StatsSide {
            calls,
            total_exec_ms: counted.total_exec_time,
            mean_ms: (calls > 0).then(|| counted.total_exec_time / calls as f64),
            rows: counted.rows,
            plans: counted.plans,
            shared_blks_hit: counted.shared_blks_hit,
            shared_blks_read: counted.shared_blks_read,
            temp_blks_read: counted.temp_blks_read,
            temp_blks_written: counted.temp_blks_written,
            wal_bytes: counted.wal_bytes,
            share_pct,
            nested_calls: if nested_only { 0 } else { self.nested.calls },
            entries: self.entries,
            queryid_count: self.queryids.len(),
            queryids: self
                .queryids
                .iter()
                .take(MAX_QUERYIDS)
                .map(ToString::to_string)
                .collect(),
            flags: flags.into_iter().collect(),
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
    rescue_unreadable(
        &mut groups,
        &log,
        &mut unusable_entries,
        &mut unusable_exec_ms,
    );
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

/// Joins entries whose text is unreadable to the statement their `queryid` belongs to: a readable
/// entry with the same `queryid` (another user or database), or the `query_id` the log carried.
fn rescue_unreadable(
    groups: &mut BTreeMap<String, Group>,
    log: &[StatementRow],
    unusable_entries: &mut usize,
    unusable_exec_ms: &mut f64,
) {
    let mut owners: BTreeMap<i64, String> = BTreeMap::new();
    for (key, group) in groups.iter().filter(|(key, _)| !key.starts_with('q')) {
        for id in &group.queryids {
            owners.entry(*id).or_insert_with(|| key.clone());
        }
    }
    for row in log {
        if let Some(id) = row.query_id.as_deref().and_then(|id| id.parse().ok()) {
            owners.entry(id).or_insert_with(|| row.fingerprint.clone());
        }
    }
    let unreadable: Vec<String> = groups
        .keys()
        .filter(|key| key.starts_with('q'))
        .cloned()
        .collect();
    for key in unreadable {
        let Some(owner) = groups
            .get(&key)
            .and_then(|group| group.queryids.iter().find_map(|id| owners.get(id)))
            .cloned()
        else {
            continue;
        };
        let Some(group) = groups.remove(&key) else {
            continue;
        };
        *unusable_entries = unusable_entries.saturating_sub(group.entries);
        *unusable_exec_ms =
            (*unusable_exec_ms - group.top.total_exec_time - group.nested.total_exec_time).max(0.0);
        groups.entry(owner).or_default().merge(group);
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
    // The log keeps one sample `query_id` per fingerprint, so a miss only means something when the
    // statistics have a single `queryid` for it.
    let query_id_matches = match (&log, group) {
        (Some(log), Some(group)) => log
            .query_id
            .as_deref()
            .and_then(|id| id.parse::<i64>().ok())
            .and_then(|id| {
                if group.queryids.contains(&id) {
                    Some(true)
                } else {
                    (group.queryids.len() == 1).then_some(false)
                }
            }),
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
    let value = match by {
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
    };
    value.filter(|value| !value.is_nan())
}

/// Orders `rows` by `by`, largest first; rows without a value go last, then by total time, then
/// by fingerprint.
pub fn rank(rows: &mut [JoinedRow], by: SortBy) {
    let by_value = |a: &JoinedRow, b: &JoinedRow, by| match (rank_value(a, by), rank_value(b, by)) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    };
    rows.sort_by(|a, b| {
        by_value(a, b, by)
            .then_with(|| by_value(a, b, SortBy::Total))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
}

#[cfg(test)]
mod tests {
    use super::{super::testkit::*, *};
    use crate::{
        stats::{delta::diff, snapshot::Key},
        testutil::key,
    };

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

    #[test]
    fn an_unreadable_entry_joins_through_a_readable_sibling() {
        let d = delta(&[
            (user_key(1, 10), Some(FILM), 5, 50.0),
            (user_key(1, 11), Some("<insufficient privilege>"), 7, 70.0),
        ]);
        let joined = join(Vec::new(), Some(&d));
        assert_eq!(joined.rows.len(), 1);
        let stats = joined.rows[0].stats.as_ref().unwrap();
        assert_eq!((stats.calls, stats.entries), (12, 2));
        assert_eq!(joined.unusable_entries, 0);
        assert!(joined.unusable_exec_ms.abs() < 1e-9);
    }

    #[test]
    fn an_unreadable_entry_joins_through_the_logged_query_id() {
        let mut log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 2, 1.0)]);
        log[0].query_id = Some("9".to_owned());
        let d = delta(&[(key(9), Some("<insufficient privilege>"), 8, 80.0)]);
        let joined = join(log, Some(&d));
        assert_eq!(joined.rows.len(), 1);
        assert_eq!(joined.rows[0].link, "both");
        assert_eq!(joined.rows[0].stats.as_ref().unwrap().calls, 8);
        assert_eq!(joined.unusable_entries, 0);
    }

    #[test]
    fn a_missing_query_id_is_only_false_when_the_statistics_have_one() {
        let mut log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 1, 1.0)]);
        log[0].query_id = Some("99".to_owned());
        let one = delta(&[(key(1), Some(FILM), 3, 3.0)]);
        assert_eq!(
            join(log.clone(), Some(&one)).rows[0].query_id_matches,
            Some(false)
        );
        let several = delta(&[
            (key(1), Some(FILM), 3, 3.0),
            (
                key(2),
                Some("SELECT * FROM film WHERE film_id = $2"),
                3,
                3.0,
            ),
        ]);
        assert_eq!(join(log, Some(&several)).rows[0].query_id_matches, None);
    }

    #[test]
    fn a_nan_time_ranks_last() {
        let d = delta(&[
            (key(1), Some("SELECT * FROM a"), 5, f64::NAN),
            (key(2), Some("SELECT * FROM b"), 5, 1.0),
        ]);
        let mut rows = join(Vec::new(), Some(&d)).rows;
        rank(&mut rows, SortBy::Total);
        assert_eq!(rows[0].text, "SELECT * FROM b");
    }

    #[test]
    fn p95_and_max_without_logs_fall_back_to_statistics_time() {
        let d = delta(&[
            (key(1), Some("SELECT * FROM a"), 5, 1.0),
            (key(2), Some("SELECT * FROM b"), 5, 9.0),
            (key(3), Some("SELECT * FROM c"), 5, 4.0),
        ]);
        for by in [SortBy::P95, SortBy::Max] {
            let mut rows = join(Vec::new(), Some(&d)).rows;
            rank(&mut rows, by);
            let order: Vec<_> = rows.iter().map(|r| r.text.as_str()).collect();
            assert_eq!(
                order,
                ["SELECT * FROM b", "SELECT * FROM c", "SELECT * FROM a"]
            );
        }
    }

    fn nested_key(queryid: i64) -> Key {
        Key {
            toplevel: false,
            ..key(queryid)
        }
    }

    #[test]
    fn nested_calls_do_not_inflate_the_top_level_numbers() {
        // `pg_stat_statements.track = all`: the same statement also ran inside a function.
        let d = delta(&[
            (key(1), Some(FILM), 30, 30.0),
            (nested_key(1), Some(FILM), 100, 40.0),
        ]);
        let log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 30, 1.0)]);
        let joined = join(log, Some(&d));
        assert_eq!(joined.rows.len(), 1);
        let row = &joined.rows[0];
        let stats = row.stats.as_ref().unwrap();
        assert_eq!(
            (stats.calls, stats.nested_calls, stats.entries),
            (30, 100, 2)
        );
        assert!((stats.total_exec_ms - 30.0).abs() < 1e-9);
        assert!((row.coverage.unwrap() - 1.0).abs() < 1e-9);
        assert!(!stats.flags.contains(&"nested"));
    }

    #[test]
    fn a_statement_that_only_runs_nested_is_flagged() {
        let d = delta(&[(nested_key(7), Some(FILM), 12, 24.0)]);
        let joined = join(Vec::new(), Some(&d));
        let stats = joined.rows[0].stats.as_ref().unwrap();
        assert_eq!((stats.calls, stats.nested_calls), (12, 0));
        assert!(stats.flags.contains(&"nested"), "{:?}", stats.flags);
    }
}
