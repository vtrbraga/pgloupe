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
/// The log window may be this many times longer, or shorter, than the snapshot window before the
/// two sources stop being comparable.
const MAX_WINDOW_RATIO: f64 = 3.0;
/// Coverage is only judged when the log window is at least this share of the snapshot window.
const LOG_COVERS_STATS: f64 = 0.8;
/// Joined statements whose log count exceeds the statistics' by this factor are flagged.
const HIGH_COVERAGE: f64 = 1.5;
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
    /// Length of the log window ÷ length of the snapshot window; `None` without both.
    pub window_ratio: Option<f64>,
    /// The rows that will be shown.
    pub top: &'a [JoinedRow],
    pub has_logs: bool,
    /// The logs come from a store, whose windows are whole hours.
    pub from_store: bool,
    /// Statement calls the log folded into its overflow bucket.
    pub log_overflow_calls: u64,
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
    if let (Some(ratio), Some(overlap)) = (ctx.window_ratio, ctx.overlap_pct)
        && overlap >= MIN_OVERLAP_PCT
        && (ratio >= MAX_WINDOW_RATIO || ratio <= 1.0 / MAX_WINDOW_RATIO)
    {
        let how = if ratio >= MAX_WINDOW_RATIO && ctx.from_store {
            format!(
                "the log window is {ratio:.1}× the snapshot window: the store keeps whole hours, so its window cannot be narrower than an hour; take the snapshots on hour boundaries (about an hour apart), or analyze the log files directly instead of the store"
            )
        } else if ratio >= MAX_WINDOW_RATIO {
            format!(
                "the log window is {ratio:.1}× the snapshot window, so the log's counts and totals cover a longer period than the statistics' deltas; trim the logs to the snapshot interval"
            )
        } else {
            format!(
                "the log window is only {ratio:.2}× the snapshot window: the logs cover just part of the period the statistics describe"
            )
        };
        found.push(finding("windows_mismatch", "warn", how));
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
    if ctx.has_logs && ctx.delta.is_some() && !ctx.joined.rows.iter().any(|row| row.log.is_some()) {
        found.push(finding(
            "no_log_records",
            "warn",
            "the logs hold no statements for this period, so every row comes from pg_stat_statements alone: the logs do not cover the window, or statements are not logged (log_min_duration_statement, log_statement)"
                .to_owned(),
        ));
    }
    if ctx.has_logs && ctx.delta.is_some() {
        heavy_stats_only(ctx, &mut found);
        low_coverage(ctx, &mut found);
        high_coverage(ctx, &mut found);
        if ctx.top.iter().any(|row| row.link == "log") {
            found.push(finding(
                "log_only_present",
                "info",
                "some top statements appear in the logs but not in the pg_stat_statements window: they started after the second snapshot, the counters were reset, or they failed (pg_stat_statements does not count failed statements)"
                    .to_owned(),
            ));
        }
    }
    if ctx.log_overflow_calls > 0 {
        found.push(finding(
            "log_overflow",
            "info",
            format!(
                "{} statement calls were folded into the log's overflow bucket (more distinct statements than --max-fingerprints), so some rows marked as statistics-only may be in the logs after all",
                ctx.log_overflow_calls
            ),
        ));
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
    if delta.reset == Reset::Unknown {
        found.push(finding(
            "stats_reset_unknown",
            "warn",
            "no info files were given and no entry appears in both snapshots, so a reset cannot be ruled out; every delta is the entry's whole history, not the window's".to_owned(),
        ));
    }
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

/// Up to [`LISTED`] fingerprints of `rows` that the report shows, so a finding never points at a
/// row the reader cannot find.
fn listed<'a>(ctx: &Context<'_>, rows: impl IntoIterator<Item = &'a JoinedRow>) -> Vec<String> {
    rows.into_iter()
        .filter(|row| {
            ctx.top
                .iter()
                .any(|shown| shown.fingerprint == row.fingerprint)
        })
        .take(LISTED)
        .map(|row| row.fingerprint.clone())
        .collect()
}

fn is_nested_only(row: &JoinedRow) -> bool {
    row.stats
        .as_ref()
        .is_some_and(|stats| stats.flags.contains(&"nested"))
}

fn heavy_stats_only(ctx: &Context<'_>, found: &mut Vec<Finding>) {
    let exec = |row: &JoinedRow| row.stats.as_ref().map_or(0.0, |stats| stats.total_exec_ms);
    // Statements that only run inside functions are already part of their callers' time.
    let total: f64 = ctx
        .joined
        .rows
        .iter()
        .filter(|row| !is_nested_only(row))
        .map(exec)
        .sum();
    // Unreadable entries (`q…`) may well be in the logs; they only cannot be matched.
    let mut only: Vec<&JoinedRow> = ctx
        .joined
        .rows
        .iter()
        .filter(|r| r.link == "stats" && !r.fingerprint.starts_with('q') && !is_nested_only(r))
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
                "{} {} {:.0}% of the execution time in pg_stat_statements but never {} in the logs: they run faster than log_min_duration_statement, or the logs miss that period",
                statements_of(only.len()),
                if only.len() == 1 { "carries" } else { "carry" },
                share * 100.0 / total,
                if only.len() == 1 { "appears" } else { "appear" }
            ),
        );
        f.statements = listed(ctx, only.iter().copied());
        found.push(f);
    }
}

fn low_coverage(ctx: &Context<'_>, found: &mut Vec<Finding>) {
    // With a log window much shorter than the statistics', low coverage says nothing about
    // log_min_duration_statement.
    if ctx
        .window_ratio
        .is_some_and(|ratio| ratio < LOG_COVERS_STATS)
        || ctx.overlap_pct.is_some_and(|pct| pct < MIN_OVERLAP_PCT)
    {
        return;
    }
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
        f.statements = listed(ctx, low.iter().copied());
        found.push(f);
    }
}

fn high_coverage(ctx: &Context<'_>, found: &mut Vec<Finding>) {
    let comparable = ctx.overlap_pct.is_none_or(|pct| pct >= MIN_OVERLAP_PCT)
        && ctx
            .window_ratio
            .is_none_or(|ratio| ratio < MAX_WINDOW_RATIO && ratio > 1.0 / MAX_WINDOW_RATIO);
    if !comparable {
        return;
    }
    let high: Vec<&JoinedRow> = ctx
        .joined
        .rows
        .iter()
        .filter(|row| {
            row.stats
                .as_ref()
                .is_some_and(|s| s.calls >= COVERAGE_MIN_CALLS)
                && row.coverage.is_some_and(|c| c > HIGH_COVERAGE)
        })
        .collect();
    if !high.is_empty() {
        let mut f = finding(
            "coverage_high",
            "info",
            format!(
                "for {} the log counted over {:.0}% of the calls pg_stat_statements did: the log window is longer than the snapshot window, or the statistics lost calls to a reset or eviction",
                statements_of(high.len()),
                HIGH_COVERAGE * 100.0
            ),
        );
        f.statements = listed(ctx, high.iter().copied());
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
        run_with_ratio(delta, log, overlap, overlap.map(|_| 1.0))
    }

    fn run_with_ratio(
        delta: Option<&Delta>,
        log: Vec<crate::analyze::report::StatementRow>,
        overlap: Option<f64>,
        ratio: Option<f64>,
    ) -> Vec<Finding> {
        let joined = join(log, delta);
        let mut top = joined.rows.clone();
        rank(&mut top, crate::analyze::report::SortBy::Total);
        findings(&Context {
            delta,
            joined: &joined,
            overlap_pct: overlap,
            window_ratio: ratio,
            top: &top,
            has_logs: true,
            from_store: false,
            log_overflow_calls: 0,
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

    #[test]
    fn windows_of_very_different_length_are_flagged() {
        let (d, log) = healthy_inputs();
        for ratio in [24.0, 4.0, 0.2] {
            let found = run_with_ratio(Some(&d), log.clone(), Some(100.0), Some(ratio));
            let f = found.iter().find(|f| f.code == "windows_mismatch").unwrap();
            assert_eq!(f.severity, "warn");
            assert!(f.message.contains('×'), "{}", f.message);
        }
        for ratio in [1.0, 0.8, 2.5] {
            let found = run_with_ratio(Some(&d), log.clone(), Some(100.0), Some(ratio));
            assert!(!codes(&found).contains(&"windows_mismatch"), "{ratio}");
        }
    }

    #[test]
    fn a_short_log_window_does_not_blame_the_log_threshold() {
        let d = delta_with_info(&[(key(1), Some(FILM), 200, 2_000.0)], Some(quiet_infos()));
        let log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 10, 50.0)]);
        let short = run_with_ratio(Some(&d), log.clone(), Some(100.0), Some(0.1));
        assert!(!codes(&short).contains(&"low_coverage"), "{short:?}");
        assert!(codes(&short).contains(&"windows_mismatch"));
        let same = run_with_ratio(Some(&d), log, Some(100.0), Some(1.0));
        assert!(codes(&same).contains(&"low_coverage"));
    }

    #[test]
    fn unreadable_entries_are_not_called_absent_from_the_logs() {
        let d = delta_with_info(
            &[
                (key(1), Some("<insufficient privilege>"), 10, 900.0),
                (key(2), Some("SELECT * FROM actor"), 10, 100.0),
            ],
            Some(quiet_infos()),
        );
        let log = log_rows(&[("SELECT * FROM actor", 9, 10.0)]);
        let found = run(Some(&d), log, Some(100.0));
        assert!(codes(&found).contains(&"no_query_text"));
        assert!(!codes(&found).contains(&"stats_only_heavy"), "{found:?}");
    }

    #[test]
    fn an_unknown_reset_is_reported() {
        // No info files and no entry in both snapshots: nothing can show a reset.
        let before = snapshot(
            "2026-10-14T10:00:00Z",
            vec![(key(1), with_text(Some(FILM), 5, 5.0))],
        );
        let after = snapshot(
            "2026-10-14T11:00:00Z",
            vec![(key(2), with_text(Some("SELECT * FROM actor"), 50, 50.0))],
        );
        let d = crate::stats::delta::diff(&before, &after).unwrap();
        let found = run(Some(&d), Vec::new(), None);
        let f = found
            .iter()
            .find(|f| f.code == "stats_reset_unknown")
            .unwrap();
        assert_eq!(f.severity, "warn");
        assert!(f.message.contains("whole history"), "{}", f.message);
    }

    #[test]
    fn the_advice_for_a_store_does_not_suggest_flags_that_cannot_help() {
        let (d, log) = healthy_inputs();
        let joined = join(log, Some(&d));
        let found = findings(&Context {
            delta: Some(&d),
            joined: &joined,
            overlap_pct: Some(100.0),
            window_ratio: Some(9.9),
            top: &joined.rows,
            has_logs: true,
            from_store: true,
            log_overflow_calls: 0,
        });
        let f = found.iter().find(|f| f.code == "windows_mismatch").unwrap();
        assert!(f.message.contains("whole hours"), "{}", f.message);
        assert!(!f.message.contains("--since"), "{}", f.message);
        // Plain files get the trimming advice.
        let files = run_with_ratio(
            Some(&d),
            log_rows(&[("SELECT 1", 1, 1.0)]),
            Some(100.0),
            Some(9.9),
        );
        let f = files.iter().find(|f| f.code == "windows_mismatch").unwrap();
        assert!(f.message.contains("trim the logs"), "{}", f.message);
    }

    #[test]
    fn disjoint_windows_do_not_blame_the_log_threshold() {
        let d = delta_with_info(&[(key(1), Some(FILM), 200, 2_000.0)], Some(quiet_infos()));
        let log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 10, 50.0)]);
        let found = run(Some(&d), log, Some(10.0));
        assert!(codes(&found).contains(&"windows_disjoint"));
        assert!(!codes(&found).contains(&"low_coverage"), "{found:?}");
    }

    #[test]
    fn an_empty_log_side_is_called_out() {
        let d = delta_with_info(&[(key(1), Some(FILM), 10, 100.0)], Some(quiet_infos()));
        let found = run(Some(&d), Vec::new(), Some(100.0));
        let f = found.iter().find(|f| f.code == "no_log_records").unwrap();
        assert_eq!(f.severity, "warn");
        assert!(f.message.contains("log_min_duration_statement"));
    }

    #[test]
    fn a_log_that_counts_more_than_the_statistics_is_flagged() {
        let d = delta_with_info(&[(key(1), Some(FILM), 100, 1_000.0)], Some(quiet_infos()));
        let log = log_rows(&[("SELECT * FROM film WHERE film_id = 5", 200, 5.0)]);
        let found = run(Some(&d), log.clone(), Some(100.0));
        let f = found.iter().find(|f| f.code == "coverage_high").unwrap();
        assert_eq!(f.statements.len(), 1);
        // A window mismatch already explains it.
        let mismatched = run_with_ratio(Some(&d), log, Some(100.0), Some(9.0));
        assert!(!codes(&mismatched).contains(&"coverage_high"));
    }

    #[test]
    fn findings_only_name_statements_the_report_shows() {
        let d = delta_with_info(
            &[
                (key(1), Some(FILM), 10, 900.0),
                (key(2), Some("SELECT * FROM actor"), 10, 100.0),
            ],
            Some(quiet_infos()),
        );
        let log = log_rows(&[("SELECT * FROM actor", 4, 10.0)]);
        let joined = join(log, Some(&d));
        let context = |top| Context {
            delta: Some(&d),
            joined: &joined,
            overlap_pct: Some(100.0),
            window_ratio: Some(1.0),
            top,
            has_logs: true,
            from_store: false,
            log_overflow_calls: 0,
        };
        let heavy = |found: Vec<Finding>| {
            found
                .into_iter()
                .find(|f| f.code == "stats_only_heavy")
                .unwrap()
        };
        assert_eq!(heavy(findings(&context(&joined.rows))).statements.len(), 1);
        assert_eq!(
            heavy(findings(&context(&[]))).statements,
            Vec::<String>::new()
        );
    }

    #[test]
    fn one_statement_agrees_with_its_verb() {
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
        assert!(f.message.contains("carries"), "{}", f.message);
        assert!(f.message.contains("never appears"), "{}", f.message);
    }

    #[test]
    fn calls_in_the_overflow_bucket_are_explained() {
        let (d, log) = healthy_inputs();
        let joined = join(log, Some(&d));
        let found = findings(&Context {
            delta: Some(&d),
            joined: &joined,
            overlap_pct: Some(100.0),
            window_ratio: Some(1.0),
            top: &joined.rows,
            has_logs: true,
            from_store: false,
            log_overflow_calls: 42,
        });
        let f = found.iter().find(|f| f.code == "log_overflow").unwrap();
        assert!(f.message.contains("42") && f.message.contains("--max-fingerprints"));
    }
}
