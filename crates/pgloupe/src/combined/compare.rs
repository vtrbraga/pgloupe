//! One period against an earlier one: what got slower, what is new, what went away.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::analyze::report::{Report, StatementRow};

/// Statements listed in a comparison.
const MOVERS: usize = 50;
/// Change in the mean duration that counts as slower or faster.
const MEAN_CHANGE: f64 = 0.25;

/// A number in this period and in the baseline.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Pair {
    pub now: f64,
    pub before: f64,
}

impl Pair {
    /// Percent change from `before`; `None` when there was nothing before.
    pub(super) fn change_pct(self) -> Option<f64> {
        (self.before > 0.0).then(|| (self.now - self.before) / self.before * 100.0)
    }
}

/// Where the baseline came from.
#[derive(Debug, Clone, Serialize)]
pub struct Baseline {
    pub since: Option<String>,
    pub until: Option<String>,
    pub inputs: usize,
}

/// Whole-period totals.
#[derive(Debug, Clone, Serialize)]
pub struct Totals {
    pub statements: Pair,
    pub total_ms: Pair,
    pub errors: Pair,
    pub lock_waits: Pair,
    pub deadlocks: Pair,
    pub temp_bytes: Pair,
}

/// How one statement moved.
#[derive(Debug, Clone, Serialize)]
pub struct Mover {
    pub fingerprint: String,
    pub text: String,
    /// `new`, `gone`, `slower`, `faster` or `same`.
    pub status: &'static str,
    pub calls: Pair,
    pub total_ms: Pair,
    pub mean_ms: Option<Pair>,
    pub p95_ms: Option<Pair>,
    pub errors: Pair,
    /// Change of `total_ms` in percent; `None` for a new statement.
    pub total_change_pct: Option<f64>,
}

/// This period against the baseline.
#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub baseline: Baseline,
    pub totals: Totals,
    /// The statements whose total time moved most, in either direction.
    pub statements: Vec<Mover>,
    /// Statements in this period and not in the baseline.
    pub new_statements: usize,
    /// Statements in the baseline and not in this period.
    pub gone_statements: usize,
}

fn totals(report: &Report, rows: &[StatementRow]) -> [f64; 6] {
    #[expect(clippy::cast_precision_loss, reason = "counts stay far below 2^53")]
    let count = |n: u64| n as f64;
    [
        count(rows.iter().map(|r| r.calls).sum()),
        rows.iter().map(|r| r.all_ms).sum(),
        count(report.errors.iter().map(|e| e.count).sum()),
        count(report.locks.waits),
        count(report.locks.deadlocks),
        count(report.temp_files.bytes),
    ]
}

fn pair(now: f64, before: f64) -> Pair {
    Pair { now, before }
}

fn optional(now: Option<f64>, before: Option<f64>) -> Option<Pair> {
    Some(pair(now?, before?))
}

#[expect(clippy::cast_precision_loss, reason = "counts stay far below 2^53")]
fn mover(fingerprint: &str, now: Option<&StatementRow>, before: Option<&StatementRow>) -> Mover {
    let calls = |r: Option<&StatementRow>| r.map_or(0.0, |r| r.calls as f64);
    let total = |r: Option<&StatementRow>| r.map_or(0.0, |r| r.all_ms);
    let errors = |r: Option<&StatementRow>| r.map_or(0.0, |r| r.errors as f64);
    let mean = optional(now.and_then(|r| r.mean_ms), before.and_then(|r| r.mean_ms));
    let p95 = optional(now.and_then(|r| r.p95_ms), before.and_then(|r| r.p95_ms));
    let status = match (now, before) {
        (Some(_), None) => "new",
        (None, Some(_)) => "gone",
        _ => match mean.and_then(Pair::change_pct) {
            Some(change) if change >= MEAN_CHANGE * 100.0 => "slower",
            Some(change) if change <= -MEAN_CHANGE * 100.0 => "faster",
            _ => "same",
        },
    };
    let total_ms = pair(total(now), total(before));
    Mover {
        fingerprint: fingerprint.to_owned(),
        text: now.or(before).map(|r| r.text.clone()).unwrap_or_default(),
        status,
        calls: pair(calls(now), calls(before)),
        total_ms,
        mean_ms: mean,
        p95_ms: p95,
        errors: pair(errors(now), errors(before)),
        total_change_pct: total_ms.change_pct(),
    }
}

/// Compares this period (`now`) with `before`, each as its report and one row per statement.
#[must_use]
pub fn compare(
    (now_report, now_rows): (&Report, &[StatementRow]),
    (before_report, before_rows): (&Report, &[StatementRow]),
) -> Comparison {
    let by_fp = |rows: &'_ [StatementRow]| -> BTreeMap<String, usize> {
        rows.iter()
            .enumerate()
            .map(|(i, r)| (r.fingerprint.clone(), i))
            .collect()
    };
    let (now_index, before_index) = (by_fp(now_rows), by_fp(before_rows));
    let mut movers: Vec<Mover> = now_index
        .keys()
        .chain(
            before_index
                .keys()
                .filter(|fp| !now_index.contains_key(*fp)),
        )
        .map(|fp| {
            mover(
                fp,
                now_index.get(fp).map(|&i| &now_rows[i]),
                before_index.get(fp).map(|&i| &before_rows[i]),
            )
        })
        .collect();
    let new_statements = movers.iter().filter(|m| m.status == "new").count();
    let gone_statements = movers.iter().filter(|m| m.status == "gone").count();
    movers.sort_by(|a, b| {
        let moved = |m: &Mover| (m.total_ms.now - m.total_ms.before).abs();
        moved(b)
            .total_cmp(&moved(a))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    movers.truncate(MOVERS);
    let (n, b) = (
        totals(now_report, now_rows),
        totals(before_report, before_rows),
    );
    let at = |i: usize| pair(n[i], b[i]);
    Comparison {
        baseline: Baseline {
            since: before_report.store.as_ref().and_then(|s| s.since.clone()),
            until: before_report.store.as_ref().and_then(|s| s.until.clone()),
            inputs: before_report.files.len(),
        },
        totals: Totals {
            statements: at(0),
            total_ms: at(1),
            errors: at(2),
            lock_waits: at(3),
            deadlocks: at(4),
            temp_bytes: at(5),
        },
        statements: movers,
        new_statements,
        gone_statements,
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::{
        analyze::{
            acc::Accumulators,
            pipeline::Output,
            report::{ReportOptions, SortBy, all_statements},
        },
        log::{LogRecord, Severity},
    };

    /// A report over statements `(table, calls, mean_ms, errors)`, one fingerprint per table.
    fn period(specs: &[(&str, u64, f64, u64)]) -> (Report, Vec<StatementRow>) {
        let mut acc = Accumulators::new(1_000, 60);
        let ts = Timestamp::UNIX_EPOCH;
        for (table, calls, mean, errors) in specs {
            let sql = format!("SELECT * FROM {table}");
            for _ in 0..*calls {
                acc.add_record(&LogRecord::new(
                    ts,
                    Severity::Log,
                    format!("duration: {mean} ms  statement: {sql}"),
                ));
            }
            for _ in 0..*errors {
                let mut e = LogRecord::new(ts, Severity::Error, "boom");
                e.statement = Some(sql.clone());
                acc.add_record(&e);
            }
        }
        let rows = all_statements(&acc);
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
        (report, rows)
    }

    /// Letters only: names that differ in trailing digits are one partitioned table.
    fn name(i: u32) -> String {
        let letter = |n: u32| char::from(b'a' + u8::try_from(n % 26).unwrap_or(0));
        format!("{}{}", letter(i / 26), letter(i))
    }

    fn status<'a>(c: &'a Comparison, table: &str) -> Option<&'a Mover> {
        c.statements.iter().find(|m| m.text.ends_with(table))
    }

    #[test]
    fn statements_are_new_gone_slower_faster_or_the_same() {
        let (nr, now) = period(&[
            ("a", 10, 30.0, 0),
            ("b", 10, 10.0, 0),
            ("c", 10, 10.0, 2),
            ("n", 5, 1.0, 0),
        ]);
        let (br, before) = period(&[
            ("a", 10, 10.0, 0),
            ("b", 10, 20.0, 0),
            ("c", 10, 10.5, 0),
            ("g", 5, 1.0, 0),
        ]);
        let c = compare((&nr, &now), (&br, &before));
        assert_eq!(status(&c, "a").map(|m| m.status), Some("slower"));
        assert_eq!(status(&c, "b").map(|m| m.status), Some("faster"));
        assert_eq!(status(&c, "c").map(|m| m.status), Some("same"));
        assert_eq!(status(&c, "n").map(|m| m.status), Some("new"));
        assert_eq!(status(&c, "g").map(|m| m.status), Some("gone"));
        assert_eq!((c.new_statements, c.gone_statements), (1, 1));
        // Most movement first: a (+200 ms) then b (-100 ms).
        assert!(c.statements[0].text.ends_with('a'));
        assert!(c.statements[1].text.ends_with('b'));
        let a = status(&c, "a").unwrap();
        assert!((a.total_change_pct.unwrap() - 200.0).abs() < 1e-6);
        assert!(status(&c, "n").unwrap().total_change_pct.is_none());
        assert!((c.totals.errors.now - 2.0).abs() < 1e-9);
        assert!((c.totals.errors.before).abs() < 1e-9);
    }

    #[test]
    fn an_empty_baseline_makes_everything_new() {
        let (nr, now) = period(&[("a", 3, 5.0, 0)]);
        let (br, before) = period(&[]);
        let c = compare((&nr, &now), (&br, &before));
        assert_eq!(c.statements[0].status, "new");
        assert_eq!(c.new_statements, 1);
        assert!(c.totals.total_ms.change_pct().is_none());
    }

    #[test]
    fn only_the_biggest_movers_are_listed() {
        let specs: Vec<(String, u64, f64, u64)> = (0..80)
            .map(|i| (format!("t_{}", name(i)), 1, f64::from(i + 1), 0))
            .collect();
        let refs: Vec<(&str, u64, f64, u64)> = specs
            .iter()
            .map(|(t, c, m, e)| (t.as_str(), *c, *m, *e))
            .collect();
        let (nr, now) = period(&refs);
        let (br, before) = period(&[]);
        let c = compare((&nr, &now), (&br, &before));
        assert_eq!(c.statements.len(), MOVERS);
        assert_eq!(c.new_statements, 80);
        assert!(c.statements[0].text.ends_with(&name(79)));
    }
}
