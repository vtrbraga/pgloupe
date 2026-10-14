//! The rules that explain odd-looking results.

use super::{Finding, Joined, JoinedRow};
use crate::stats::delta::{Delta, Reset};

/// Statistics-only statements must carry at least this share of the execution time to be reported.
const HEAVY_SHARE: f64 = 0.05;
/// Joined statements need this many calls in the window before their coverage means anything.
const COVERAGE_MIN_CALLS: u64 = 100;
/// Log calls below this fraction of the statistics' calls count as "the log sees little".
const LOW_COVERAGE: f64 = 0.2;
/// Windows overlapping less than this (percent of the shorter one) are not comparable.
const MIN_OVERLAP_PCT: f64 = 50.0;
/// Fingerprints listed in a finding.
const LISTED: usize = 5;

/// What the rules look at.
#[derive(Debug)]
pub struct Context<'a> {
    pub delta: Option<&'a Delta>,
    /// Every joined row, not only the ranked top.
    pub joined: &'a Joined,
    /// How much the log and snapshot windows overlap, in percent; `None` without both.
    pub overlap_pct: Option<f64>,
    /// The rows that will be shown.
    pub top: &'a [JoinedRow],
    pub has_logs: bool,
}

/// `1 statement` or `3 statements`.
fn statements_of(n: usize) -> String {
    format!("{n} statement{}", if n == 1 { "" } else { "s" })
}

fn finding(code: &'static str, severity: &'static str, message: String) -> Finding {
    Finding {
        code,
        severity,
        message,
        statements: Vec::new(),
    }
}

/// Applies the rules in the order the report lists them.
#[must_use]
pub fn findings(ctx: &Context<'_>) -> Vec<Finding> {
    let mut found = Vec::new();
    if let Some(delta) = ctx.delta {
        stats_rules(delta, &mut found);
    }
    if let Some(overlap) = ctx.overlap_pct
        && overlap < MIN_OVERLAP_PCT
    {
        found.push(finding(
            "windows_disjoint",
            "warn",
            format!(
                "the log window and the snapshot window overlap by only {overlap:.0}% of the shorter one, so the log's counts and the statistics' deltas describe different periods"
            ),
        ));
    }
    if ctx.joined.unusable_entries > 0 {
        found.push(finding(
            "no_query_text",
            "warn",
            format!(
                "{} pg_stat_statements entries ({:.0} ms of execution time) have no readable query text and cannot be matched to the logs; grant pg_read_all_stats to the exporting role",
                ctx.joined.unusable_entries, ctx.joined.unusable_exec_ms
            ),
        ));
    }
    if ctx.has_logs && ctx.delta.is_some() {
        heavy_stats_only(ctx, &mut found);
        low_coverage(ctx, &mut found);
        if ctx.top.iter().any(|row| row.link == "log") {
            found.push(finding(
                "log_only_present",
                "info",
                "some top statements appear in the logs but not in the pg_stat_statements window: they started after the second snapshot, or the counters were reset"
                    .to_owned(),
            ));
        }
    }
    if ctx.joined.rollups > 0 {
        found.push(finding(
            "partition_rollup",
            "info",
            format!(
                "{} matched several pg_stat_statements entries with different queryids; partitions and per-tenant variants are rolled up by fingerprint, so their deltas are sums",
                statements_of(ctx.joined.rollups)
            ),
        ));
    }
    found
}

fn stats_rules(delta: &Delta, found: &mut Vec<Finding>) {
    if let Reset::Proven | Reset::Inferred = delta.reset {
        let how = if delta.reset == Reset::Proven {
            "proven by the info files"
        } else {
            "inferred from restarted counters"
        };
        found.push(finding(
            "stats_reset",
            "warn",
            format!(
                "pg_stat_statements counters were reset between the snapshots ({how}); the deltas are lower bounds"
            ),
        ));
    }
    if let Some(evicted) = delta.evictions
        && evicted > 0
    {
        found.push(finding(
            "stats_evictions",
            "warn",
            format!(
                "{evicted} entries were evicted from pg_stat_statements during the window, so the least used statements are incomplete; raise pg_stat_statements.max"
            ),
        ));
    }
    if delta.blind_spot {
        found.push(finding(
            "stats_blind_spot",
            "info",
            "no pg_stat_statements_info files were given, so a reset between the snapshots would go unnoticed; export the info row with each snapshot"
                .to_owned(),
        ));
    }
}

fn heavy_stats_only(ctx: &Context<'_>, found: &mut Vec<Finding>) {
    let exec = |row: &JoinedRow| row.stats.as_ref().map_or(0.0, |stats| stats.total_exec_ms);
    let total: f64 = ctx.joined.rows.iter().map(exec).sum();
    let mut only: Vec<&JoinedRow> = ctx
        .joined
        .rows
        .iter()
        .filter(|r| r.link == "stats")
        .collect();
    only.sort_by(|a, b| {
        exec(b)
            .total_cmp(&exec(a))
            .then(a.fingerprint.cmp(&b.fingerprint))
    });
    let share: f64 = only.iter().map(|row| exec(row)).sum::<f64>();
    if total > 0.0 && share / total >= HEAVY_SHARE {
        let mut f = finding(
            "stats_only_heavy",
            "warn",
            format!(
                "{} {} {:.0}% of the execution time in pg_stat_statements but never appear in the logs: they run faster than log_min_duration_statement, or the logs miss that period",
                statements_of(only.len()),
                if only.len() == 1 { "carries" } else { "carry" },
                share * 100.0 / total
            ),
        );
        f.statements = only
            .iter()
            .take(LISTED)
            .map(|r| r.fingerprint.clone())
            .collect();
        found.push(f);
    }
}

fn low_coverage(ctx: &Context<'_>, found: &mut Vec<Finding>) {
    let low: Vec<&JoinedRow> = ctx
        .joined
        .rows
        .iter()
        .filter(|row| {
            row.stats
                .as_ref()
                .is_some_and(|s| s.calls >= COVERAGE_MIN_CALLS)
                && row.coverage.is_some_and(|c| c < LOW_COVERAGE)
        })
        .collect();
    if !low.is_empty() {
        let mut f = finding(
            "low_coverage",
            "info",
            format!(
                "for {} the log recorded under {:.0}% of the calls pg_stat_statements counted: log_min_duration_statement hides the fast ones, so their log percentiles describe only the slow calls",
                statements_of(low.len()),
                LOW_COVERAGE * 100.0
            ),
        );
        f.statements = low
            .iter()
            .take(LISTED)
            .map(|r| r.fingerprint.clone())
            .collect();
        found.push(f);
    }
}

#[cfg(test)]
mod tests {
    use super::{super::testkit::*, *};
    use crate::{
        combined::{join, rank},
        testutil::{info, key},
    };

    const FILM: &str = "SELECT * FROM film WHERE film_id = $1";

    fn quiet_infos() -> (crate::stats::snapshot::Info, crate::stats::snapshot::Info) {
        (
            info(0, "2026-10-01T00:00:00Z"),
            info(0, "2026-10-01T00:00:00Z"),
        )
    }

    fn run(
        delta: Option<&Delta>,
        log: Vec<crate::analyze::report::StatementRow>,
        overlap: Option<f64>,
    ) -> Vec<Finding> {
        let joined = join(log, delta);
        let mut top = joined.rows.clone();
        rank(&mut top, crate::analyze::report::SortBy::Total);
        findings(&Context {
            delta,
            joined: &joined,
            overlap_pct: overlap,
            top: &top,
            has_logs: true,
        })
    }

    fn codes(found: &[Finding]) -> Vec<&'static str> {
        found.iter().map(|f| f.code).collect()
    }

    fn healthy_inputs() -> (Delta, Vec<crate::analyze::report::StatementRow>) {
        let d = delta_with_info(&[(key(1), Some(FILM), 10, 100.0)], Some(quiet_infos()));
        let log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 9, 10.0)]);
        (d, log)
    }

    #[test]
    fn a_healthy_report_has_no_findings() {
        let (d, log) = healthy_inputs();
        assert_eq!(codes(&run(Some(&d), log, Some(100.0))), Vec::<&str>::new());
    }

    #[test]
    fn logs_alone_have_no_statistics_findings() {
        let log = log_rows(&[("SELECT 1", 2, 1.0)]);
        assert_eq!(codes(&run(None, log, None)), Vec::<&str>::new());
    }

    #[test]
    fn a_reset_is_reported() {
        let d = delta_with_info(
            &[(key(1), Some(FILM), 10, 100.0)],
            Some((
                info(0, "2026-10-01T00:00:00Z"),
                info(0, "2026-10-14T10:30:00Z"),
            )),
        );
        let found = run(Some(&d), Vec::new(), None);
        let f = found.iter().find(|f| f.code == "stats_reset").unwrap();
        assert_eq!(f.severity, "warn");
        assert!(f.message.contains("lower bound"), "{}", f.message);
    }

    #[test]
    fn evictions_say_what_to_raise() {
        let d = delta_with_info(
            &[(key(1), Some(FILM), 10, 100.0)],
            Some((
                info(0, "2026-10-01T00:00:00Z"),
                info(7, "2026-10-01T00:00:00Z"),
            )),
        );
        let found = run(Some(&d), Vec::new(), None);
        let f = found.iter().find(|f| f.code == "stats_evictions").unwrap();
        assert!(f.message.contains("pg_stat_statements.max") && f.message.contains('7'));
    }

    #[test]
    fn missing_info_files_are_a_blind_spot() {
        let d = delta(&[(key(1), Some(FILM), 10, 100.0)]);
        let found = run(Some(&d), Vec::new(), None);
        assert!(codes(&found).contains(&"stats_blind_spot"), "{found:?}");
    }

    #[test]
    fn windows_that_barely_overlap_are_flagged() {
        let (d, log) = healthy_inputs();
        let found = run(Some(&d), log.clone(), Some(10.0));
        assert!(codes(&found).contains(&"windows_disjoint"));
        assert!(!codes(&run(Some(&d), log, Some(80.0))).contains(&"windows_disjoint"));
    }

    #[test]
    fn unreadable_text_points_at_the_privilege() {
        let d = delta_with_info(
            &[(key(1), Some("<insufficient privilege>"), 10, 100.0)],
            Some(quiet_infos()),
        );
        let found = run(Some(&d), Vec::new(), None);
        let f = found.iter().find(|f| f.code == "no_query_text").unwrap();
        assert!(f.message.contains("pg_read_all_stats"));
    }

    #[test]
    fn heavy_statements_missing_from_the_logs_are_listed() {
        let d = delta_with_info(
            &[
                (key(1), Some(FILM), 10, 900.0),
                (key(2), Some("SELECT * FROM actor"), 10, 100.0),
            ],
            Some(quiet_infos()),
        );
        let log = log_rows(&[("SELECT * FROM actor", 4, 10.0)]);
        let found = run(Some(&d), log, Some(100.0));
        let f = found.iter().find(|f| f.code == "stats_only_heavy").unwrap();
        assert_eq!(f.statements.len(), 1);
        assert!(f.message.contains("log_min_duration_statement"));
        // A light statistics-only statement does not fire it.
        let d = delta_with_info(
            &[
                (key(1), Some(FILM), 1, 1.0),
                (key(2), Some("SELECT * FROM actor"), 10, 1_000.0),
            ],
            Some(quiet_infos()),
        );
        let log = log_rows(&[("SELECT * FROM actor", 9, 10.0)]);
        assert!(!codes(&run(Some(&d), log, Some(100.0))).contains(&"stats_only_heavy"));
    }

    #[test]
    fn a_log_that_sees_few_calls_is_flagged() {
        let d = delta_with_info(&[(key(1), Some(FILM), 200, 2_000.0)], Some(quiet_infos()));
        let log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 10, 50.0)]);
        let found = run(Some(&d), log, Some(100.0));
        let f = found.iter().find(|f| f.code == "low_coverage").unwrap();
        assert_eq!(f.severity, "info");
        assert_eq!(f.statements.len(), 1);
    }

    #[test]
    fn a_log_only_statement_among_the_top_is_noted() {
        let d = delta_with_info(&[(key(1), Some(FILM), 10, 100.0)], Some(quiet_infos()));
        let log = log_rows(&[("SELECT * FROM actor", 4, 10.0)]);
        let found = run(Some(&d), log, Some(100.0));
        assert!(codes(&found).contains(&"log_only_present"), "{found:?}");
    }

    #[test]
    fn partition_roll_ups_are_explained() {
        let d = delta_with_info(
            &[
                (
                    key(1),
                    Some("SELECT * FROM events_p0001 WHERE id = $1"),
                    10,
                    10.0,
                ),
                (
                    key(2),
                    Some("SELECT * FROM events_p0002 WHERE id = $1"),
                    10,
                    10.0,
                ),
            ],
            Some(quiet_infos()),
        );
        let found = run(Some(&d), Vec::new(), None);
        let f = found.iter().find(|f| f.code == "partition_rollup").unwrap();
        assert!(f.message.contains("partition"));
    }
}
