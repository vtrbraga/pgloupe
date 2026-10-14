//! Builders shared by the tests of this module.

use crate::{
    analyze::{
        acc::Accumulators,
        report::{StatementRow, all_statements},
    },
    log::{LogRecord, Severity},
    stats::{
        delta::{Delta, diff},
        snapshot::{Entry, Info, Key, Snapshot},
    },
    testutil::{entry, key, ts},
};

/// Log rows for `(sql, calls, ms each)`, through the real accumulators so ids are real.
pub(super) fn log_rows(statements: &[(&str, usize, f64)]) -> Vec<StatementRow> {
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

pub(super) fn with_text(text: Option<&str>, calls: u64, total: f64) -> Entry {
    Entry {
        query: text.map(str::to_owned),
        ..entry(calls, total)
    }
}

pub(super) fn snapshot(at: &str, entries: Vec<(Key, Entry)>) -> Snapshot {
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
pub(super) fn delta(rows: &[(Key, Option<&str>, u64, f64)]) -> Delta {
    delta_with_info(rows, None)
}

/// Like [`delta`], with the `pg_stat_statements_info` rows of the two snapshots.
pub(super) fn delta_with_info(
    rows: &[(Key, Option<&str>, u64, f64)],
    infos: Option<(Info, Info)>,
) -> Delta {
    let before = rows
        .iter()
        .map(|(k, text, _, _)| (*k, with_text(*text, 1, 1.0)))
        .collect();
    let after = rows
        .iter()
        .map(|(k, text, calls, total)| (*k, with_text(*text, 1 + calls, 1.0 + total)))
        .collect();
    let mut before = snapshot("2026-10-14T10:00:00Z", before);
    let mut after = snapshot("2026-10-14T11:00:00Z", after);
    before.info = infos.map(|(before, _)| before);
    after.info = infos.map(|(_, after)| after);
    diff(&before, &after).unwrap()
}

pub(super) fn user_key(queryid: i64, userid: u32) -> Key {
    Key {
        userid,
        ..key(queryid)
    }
}
