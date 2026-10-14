//! Shared helpers for unit tests.

use jiff::Timestamp;

use crate::stats::snapshot::{Counters, Entry, Info, Key, Snapshot};

/// True when `a` and `b` differ by less than 1e-9.
pub(crate) fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

pub(crate) fn ts(text: &str) -> Timestamp {
    text.parse().unwrap()
}

pub(crate) fn key(queryid: i64) -> Key {
    Key {
        dbid: 1,
        userid: 10,
        queryid,
        toplevel: true,
    }
}

/// An entry with `calls` calls (and as many rows) and `total` ms of execution time.
pub(crate) fn entry(calls: u64, total: f64) -> Entry {
    Entry {
        counters: Counters {
            calls,
            rows: calls,
            total_exec_time: total,
            ..Counters::default()
        },
        ..Entry::default()
    }
}

pub(crate) fn snap(at: &str, version: u32, entries: Vec<(i64, Entry)>) -> Snapshot {
    Snapshot {
        taken_at: ts(at),
        server_version_num: version,
        entries: entries.into_iter().map(|(id, e)| (key(id), e)).collect(),
        skipped_no_queryid: 0,
        optional_columns: std::collections::BTreeSet::new(),
        info: None,
    }
}

pub(crate) fn entry_since(calls: u64, total: f64, since: &str) -> Entry {
    Entry {
        stats_since: Some(ts(since)),
        ..entry(calls, total)
    }
}

pub(crate) fn info(dealloc: u64, reset: &str) -> Info {
    Info {
        dealloc,
        stats_reset: Some(ts(reset)),
    }
}
