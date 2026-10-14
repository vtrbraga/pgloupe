//! Per-query deltas between two snapshots.

use jiff::Timestamp;
use serde::Serialize;

use crate::{
    error::DeltaError,
    stats::snapshot::{Counters, Entry, Info, Key, Snapshot},
};

/// Whether the counters were reset between the snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reset {
    /// No evidence of a reset.
    None,
    /// Every entry present in both snapshots restarted, but no info files prove it.
    Inferred,
    /// The info files show different `stats_reset` times.
    Proven,
}

/// How a row's delta was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// `after - before`.
    Ok,
    /// Only in `after`; the delta is `after`'s values.
    New,
    /// The entry's counters restarted; the delta is `after`'s values.
    Restarted,
}

/// One compared entry.
#[derive(Debug, Clone)]
pub struct Row {
    pub key: Key,
    pub datname: Option<String>,
    pub rolname: Option<String>,
    pub query: Option<String>,
    pub delta: Counters,
    pub status: Status,
    /// The window starts at a reset, so the delta is a lower bound.
    pub partial: bool,
    /// New entry in a window with evictions: it may have been evicted and re-admitted.
    pub evicted_suspect: bool,
    /// Cumulative mean execution time (ms) in the `before` snapshot.
    pub mean_before: Option<f64>,
    /// Cumulative mean execution time (ms) in the `after` snapshot.
    pub mean_after: Option<f64>,
    /// Share of the window's total execution time, in percent.
    pub share_pct: f64,
}

impl Row {
    /// Mean execution time (ms) of the calls inside the window.
    #[must_use]
    pub fn window_mean(&self) -> Option<f64> {
        mean(self.delta.total_exec_time, self.delta.calls)
    }
}

/// How many entries fell into each bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// Entries in the `after` snapshot.
    pub entries: usize,
    pub new: usize,
    pub restarted: usize,
    /// In `before` but not in `after`; not ranked.
    pub gone: usize,
    /// Present but with no calls in the window; not ranked.
    pub unchanged: usize,
    /// Rows of the `after` file without a usable `queryid`.
    pub skipped_no_queryid: usize,
}

/// The comparison of two snapshots.
#[derive(Debug, Clone)]
pub struct Delta {
    pub before_at: Timestamp,
    pub after_at: Timestamp,
    pub server_version_num: u32,
    pub reset: Reset,
    /// The `stats_reset` time when the reset is proven.
    pub reset_at: Option<Timestamp>,
    /// Entries evicted in the window; `None` without info files.
    pub evictions: Option<u64>,
    /// A reset would go unnoticed: no info files, no `stats_since`, and none detected.
    pub blind_spot: bool,
    pub counts: Counts,
    /// All compared entries, ordered by key.
    pub rows: Vec<Row>,
}

/// An `after` entry with its `before` counterpart.
struct Paired<'a> {
    key: Key,
    after: &'a Entry,
    before: Option<&'a Entry>,
    restarted: bool,
}

/// Compares two snapshots.
///
/// # Errors
///
/// Fails if the snapshots come from different Postgres major versions or `after` is not later
/// than `before`.
pub fn diff(before: &Snapshot, after: &Snapshot) -> Result<Delta, DeltaError> {
    check_window(before, after)?;
    let infos = before.info.zip(after.info);
    let pairs = pair(before, after);
    let reset = judge_reset(infos, &pairs);
    let reset_at = match (reset, infos) {
        (Reset::Proven, Some((_, after_info))) => after_info.stats_reset,
        _ => None,
    };
    let evictions = count_evictions(infos, reset);
    let blind_spot = reset == Reset::None && !can_detect_reset(infos, &pairs);
    let partial = reset != Reset::None;
    let mut rows: Vec<Row> = pairs
        .iter()
        .map(|p| build_row(p, partial, evictions))
        .collect();
    fill_shares(&mut rows);
    let counts = count(before, after, &rows);
    Ok(Delta {
        before_at: before.taken_at,
        after_at: after.taken_at,
        server_version_num: after.server_version_num,
        reset,
        reset_at,
        evictions,
        blind_spot,
        counts,
        rows,
    })
}

fn check_window(before: &Snapshot, after: &Snapshot) -> Result<(), DeltaError> {
    let (before_major, after_major) = (
        before.server_version_num / 10_000,
        after.server_version_num / 10_000,
    );
    if before_major != after_major {
        return Err(DeltaError::VersionMismatch {
            before: before_major,
            after: after_major,
        });
    }
    if after.taken_at <= before.taken_at {
        return Err(DeltaError::NotChronological {
            before: before.taken_at,
            after: after.taken_at,
        });
    }
    Ok(())
}

fn pair<'a>(before: &'a Snapshot, after: &'a Snapshot) -> Vec<Paired<'a>> {
    after
        .entries
        .iter()
        .map(|(key, after_entry)| {
            let before_entry = before.entries.get(key);
            Paired {
                key: *key,
                after: after_entry,
                before: before_entry,
                restarted: before_entry.is_some_and(|b| entry_restarted(b, after_entry)),
            }
        })
        .collect()
}

/// An entry restarted if PG17's `stats_since` moved or any counter went backwards.
fn entry_restarted(before: &Entry, after: &Entry) -> bool {
    let since_moved = matches!(
        (before.stats_since, after.stats_since),
        (Some(b), Some(a)) if a != b
    );
    since_moved || after.counters.went_backwards(&before.counters)
}

fn judge_reset(infos: Option<(Info, Info)>, pairs: &[Paired<'_>]) -> Reset {
    let proven = infos.is_some_and(|(before, after)| {
        after.stats_reset.is_some() && before.stats_reset != after.stats_reset
    });
    if proven {
        return Reset::Proven;
    }
    let shared = pairs.iter().filter(|p| p.before.is_some()).count();
    let restarted = pairs.iter().filter(|p| p.restarted).count();
    if shared > 0 && shared == restarted {
        Reset::Inferred
    } else {
        Reset::None
    }
}

/// With a proven reset, `dealloc` was zeroed by it, so `after.dealloc` counts evictions since.
fn count_evictions(infos: Option<(Info, Info)>, reset: Reset) -> Option<u64> {
    let (before, after) = infos?;
    Some(if reset == Reset::Proven {
        after.dealloc
    } else {
        after.dealloc.saturating_sub(before.dealloc)
    })
}

/// A reset can be seen with info files, or when every shared entry carries `stats_since`.
fn can_detect_reset(infos: Option<(Info, Info)>, pairs: &[Paired<'_>]) -> bool {
    if infos.is_some() {
        return true;
    }
    let shared: Vec<_> = pairs
        .iter()
        .filter_map(|p| p.before.map(|before| (before, p.after)))
        .collect();
    !shared.is_empty()
        && shared
            .iter()
            .all(|(before, after)| before.stats_since.is_some() && after.stats_since.is_some())
}

fn build_row(p: &Paired<'_>, partial: bool, evictions: Option<u64>) -> Row {
    let after = &p.after.counters;
    let fresh = partial || p.restarted || p.before.is_none();
    let delta = match p.before {
        Some(before) if !fresh => after.minus(&before.counters),
        _ => *after,
    };
    let status = if p.before.is_none() {
        Status::New
    } else if partial || p.restarted {
        Status::Restarted
    } else {
        Status::Ok
    };
    Row {
        key: p.key,
        datname: p.after.datname.clone(),
        rolname: p.after.rolname.clone(),
        query: p.after.query.clone(),
        delta,
        status,
        partial,
        evicted_suspect: status == Status::New && evictions.is_some_and(|n| n > 0),
        mean_before: p
            .before
            .and_then(|b| mean(b.counters.total_exec_time, b.counters.calls)),
        mean_after: mean(after.total_exec_time, after.calls),
        share_pct: 0.0,
    }
}

fn fill_shares(rows: &mut [Row]) {
    let total: f64 = rows.iter().map(|row| row.delta.total_exec_time).sum();
    if total > 0.0 {
        for row in rows {
            row.share_pct = 100.0 * row.delta.total_exec_time / total;
        }
    }
}

fn count(before: &Snapshot, after: &Snapshot, rows: &[Row]) -> Counts {
    let with_status = |status: Status| rows.iter().filter(|row| row.status == status).count();
    Counts {
        entries: after.entries.len(),
        new: with_status(Status::New),
        restarted: with_status(Status::Restarted),
        gone: before
            .entries
            .keys()
            .filter(|key| !after.entries.contains_key(key))
            .count(),
        unchanged: rows.iter().filter(|row| row.delta.calls == 0).count(),
        skipped_no_queryid: after.skipped_no_queryid,
    }
}

fn mean(total_ms: f64, calls: u64) -> Option<f64> {
    (calls > 0).then(|| total_ms / to_f64(calls))
}

#[expect(
    clippy::cast_precision_loss,
    reason = "counters stay far below 2^53 in practice"
)]
pub(crate) fn to_f64(n: u64) -> f64 {
    n as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{approx_eq, entry, snap};

    const T0: &str = "2026-10-14 10:00:00+00";
    const T1: &str = "2026-10-14 10:10:00+00";
    const V17: u32 = 170_011;

    fn row_for(delta: &Delta, queryid: i64) -> &Row {
        delta
            .rows
            .iter()
            .find(|row| row.key.queryid == queryid)
            .unwrap()
    }

    #[test]
    fn growth_is_after_minus_before() {
        let before = snap(T0, V17, vec![(1, entry(100, 50.0))]);
        let after = snap(T1, V17, vec![(1, entry(160, 80.0))]);
        let delta = diff(&before, &after).unwrap();
        let row = row_for(&delta, 1);
        assert_eq!(row.delta.calls, 60);
        assert_eq!(row.delta.rows, 60);
        assert!(approx_eq(row.delta.total_exec_time, 30.0));
        assert_eq!(row.status, Status::Ok);
        assert!(!row.partial);
        assert!(!row.evicted_suspect);
        assert_eq!(delta.reset, Reset::None);
        assert_eq!(delta.reset_at, None);
        assert_eq!(delta.evictions, None);
        assert!(approx_eq(row.mean_before.unwrap(), 0.5));
        assert!(approx_eq(row.mean_after.unwrap(), 0.5));
        assert!(approx_eq(row.window_mean().unwrap(), 0.5));
    }

    #[test]
    fn new_gone_and_unchanged_entries_are_classified() {
        let before = snap(
            T0,
            V17,
            vec![
                (1, entry(100, 50.0)),
                (2, entry(10, 200.0)),
                (3, entry(5, 5.0)),
            ],
        );
        let after = snap(
            T1,
            V17,
            vec![
                (1, entry(100, 50.0)),
                (2, entry(14, 600.0)),
                (4, entry(2, 70.0)),
            ],
        );
        let delta = diff(&before, &after).unwrap();
        assert_eq!(
            delta.counts,
            Counts {
                entries: 3,
                new: 1,
                restarted: 0,
                gone: 1,
                unchanged: 1,
                skipped_no_queryid: 0,
            }
        );
        assert_eq!(row_for(&delta, 4).status, Status::New);
        assert_eq!(row_for(&delta, 4).delta.calls, 2);
        assert_eq!(row_for(&delta, 4).mean_before, None);
        assert_eq!(row_for(&delta, 1).delta.calls, 0);
        assert!(delta.rows.iter().all(|row| row.key.queryid != 3));
    }

    #[test]
    fn skipped_rows_come_from_the_after_snapshot() {
        let before = snap(T0, V17, vec![(1, entry(1, 1.0))]);
        let mut after = snap(T1, V17, vec![(1, entry(2, 2.0))]);
        after.skipped_no_queryid = 4;
        assert_eq!(diff(&before, &after).unwrap().counts.skipped_no_queryid, 4);
    }

    #[test]
    fn different_major_versions_are_refused() {
        let before = snap(T0, 160_005, vec![(1, entry(1, 1.0))]);
        let after = snap(T1, V17, vec![(1, entry(2, 2.0))]);
        assert_eq!(
            diff(&before, &after).unwrap_err(),
            DeltaError::VersionMismatch {
                before: 16,
                after: 17
            }
        );
    }

    #[test]
    fn minor_version_difference_is_fine() {
        let before = snap(T0, 170_004, vec![(1, entry(1, 1.0))]);
        let after = snap(T1, 170_011, vec![(1, entry(2, 2.0))]);
        diff(&before, &after).unwrap();
    }

    #[test]
    fn after_must_be_later_than_before() {
        let before = snap(T1, V17, vec![(1, entry(1, 1.0))]);
        let same = snap(T1, V17, vec![(1, entry(2, 2.0))]);
        assert!(matches!(
            diff(&before, &same).unwrap_err(),
            DeltaError::NotChronological { .. }
        ));
        let earlier = snap(T0, V17, vec![(1, entry(2, 2.0))]);
        assert!(matches!(
            diff(&before, &earlier).unwrap_err(),
            DeltaError::NotChronological { .. }
        ));
    }

    #[test]
    fn share_is_a_percentage_of_the_window_total() {
        let before = snap(T0, V17, vec![(1, entry(10, 10.0)), (2, entry(10, 10.0))]);
        let after = snap(T1, V17, vec![(1, entry(20, 40.0)), (2, entry(20, 20.0))]);
        let delta = diff(&before, &after).unwrap();
        assert!(approx_eq(row_for(&delta, 1).share_pct, 75.0));
        assert!(approx_eq(row_for(&delta, 2).share_pct, 25.0));
    }

    #[test]
    fn zero_total_gives_zero_share() {
        let before = snap(T0, V17, vec![(1, entry(10, 10.0))]);
        let after = snap(T1, V17, vec![(1, entry(10, 10.0))]);
        let delta = diff(&before, &after).unwrap();
        assert!(approx_eq(row_for(&delta, 1).share_pct, 0.0));
    }

    #[test]
    fn zero_calls_have_no_mean() {
        let before = snap(T0, V17, vec![(1, entry(0, 0.0))]);
        let after = snap(T1, V17, vec![(1, entry(0, 0.0))]);
        let delta = diff(&before, &after).unwrap();
        let row = row_for(&delta, 1);
        assert_eq!(row.mean_before, None);
        assert_eq!(row.mean_after, None);
        assert_eq!(row.window_mean(), None);
    }

    #[test]
    fn rows_are_ordered_by_key() {
        let before = snap(T0, V17, vec![(1, entry(1, 1.0))]);
        let after = snap(
            T1,
            V17,
            vec![(9, entry(1, 1.0)), (1, entry(2, 2.0)), (5, entry(1, 1.0))],
        );
        let delta = diff(&before, &after).unwrap();
        let ids: Vec<i64> = delta.rows.iter().map(|row| row.key.queryid).collect();
        assert_eq!(ids, [1, 5, 9]);
    }
}
