//! Reports over a time window of a store.

use std::path::Path;

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use thiserror::Error;

use super::{Input, Store, StoreError};
use crate::analyze::{
    Merge,
    acc::{Accumulators, Timeline},
    diagnostics::merge_shapes,
    pipeline::Output,
    report::{Report, ReportOptions, StoreInfo},
    slice::{HOUR, hour_of},
};

/// Fingerprints kept when slices are merged for a report, as in `logs analyze`.
const FINGERPRINT_LIMIT: usize = 50_000;

/// Why a report could not be built.
#[derive(Debug, Error)]
pub enum ReportError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("--since must be earlier than --until")]
    EmptyWindow,
    #[error("the store keeps one-minute buckets; use a multiple of 1m")]
    Bucket,
}

/// Parses `2026-10-14T10:00:00Z` (any offset) or `2026-10-14` (midnight UTC).
///
/// # Errors
///
/// Fails with a message naming the accepted forms.
pub fn parse_time(text: &str) -> Result<Timestamp, String> {
    if let Ok(timestamp) = text.parse::<Timestamp>() {
        return Ok(timestamp);
    }
    let bad = || format!("{text:?} is not a time like 2026-10-14T10:00:00Z or 2026-10-14");
    // jiff would also read the date out of a date-time and drop the time; refuse that.
    if text.len() != 10 {
        return Err(bad());
    }
    let date: Date = text.parse().map_err(|_err| bad())?;
    date.to_zoned(TimeZone::UTC)
        .map(|zoned| zoned.timestamp())
        .map_err(|_err| bad())
}

fn floor_hour(timestamp: Timestamp) -> i64 {
    hour_of(timestamp.as_second())
}

fn ceil_hour(timestamp: Timestamp) -> i64 {
    let floor = floor_hour(timestamp);
    if floor == timestamp.as_second() && timestamp.subsec_nanosecond() == 0 {
        floor
    } else {
        floor + HOUR
    }
}

/// Merges `slices` into one set of statistics capped at `limit` fingerprints. The merge itself
/// is uncapped: a fingerprint that is small in every hour but large overall must survive.
#[cfg(test)]
fn merge_slices(slices: Vec<Accumulators>, limit: usize) -> Accumulators {
    let mut merged = merge_target();
    for slice in slices {
        merged.merge(slice);
    }
    merged.recap(limit);
    merged
}

/// Where slices are merged: uncapped, so the single recap at the end decides what overflows.
fn merge_target() -> Accumulators {
    Accumulators::new(usize::MAX, 60)
}

/// Builds the report for the hours from `since` (rounded down) to `until` (rounded up).
///
/// `bucket_secs` is the timeline resolution: a multiple of 60.
///
/// # Errors
///
/// Fails on an empty window, an unsupported bucket, or store problems.
pub fn build(
    store: &Store,
    path: &Path,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    options: &ReportOptions,
    bucket_secs: i64,
) -> Result<Report, ReportError> {
    build_parts(store, path, since, until, options, bucket_secs).map(|(report, _acc)| report)
}

/// Like [`build`], and also returns the merged statistics the report was made from.
///
/// # Errors
///
/// Fails like [`build`].
pub fn build_parts(
    store: &Store,
    path: &Path,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    options: &ReportOptions,
    bucket_secs: i64,
) -> Result<(Report, Accumulators), ReportError> {
    if bucket_secs < 60 || bucket_secs % 60 != 0 {
        return Err(ReportError::Bucket);
    }
    let lo = since.map_or(i64::MIN, floor_hour);
    let hi = until.map_or(i64::MAX, ceil_hour);
    if lo >= hi {
        return Err(ReportError::EmptyWindow);
    }
    let mut acc = merge_target();
    store.for_each_slice(lo, hi, |slice| acc.merge(slice))?;
    acc.recap(FINGERPRINT_LIMIT);
    acc.totals.timeline = std::mem::replace(
        &mut acc.totals.timeline,
        Timeline {
            bucket_secs: 60,
            buckets: std::collections::BTreeMap::default(),
        },
    )
    .rebucket(bucket_secs);
    let inputs = store.inputs_in(lo, hi)?;
    let output = Output {
        acc,
        files: Vec::new(),
    };
    let mut report = Report::build(&output, options);
    report.files = inputs.iter().map(|input| input.summary.clone()).collect();
    report.diagnostics = stored_diagnostics(&inputs, report.diagnostics);
    let edge = |bound: Option<Timestamp>, value: i64| {
        bound
            .and_then(|_| Timestamp::from_second(value).ok())
            .map(|timestamp| timestamp.to_string())
    };
    report.store = Some(StoreInfo {
        path: path.display().to_string(),
        since: edge(since, lo),
        until: edge(until, hi),
        inputs: inputs.len(),
    });
    Ok((report, output.acc))
}

/// The diagnostics a report over `inputs` shows: what each file recorded at ingest, followed by
/// what the merged statistics say (`computed`).
fn stored_diagnostics(
    inputs: &[Input],
    computed: crate::analyze::diagnostics::Diagnostics,
) -> crate::analyze::diagnostics::Diagnostics {
    let mut warnings: Vec<String> = Vec::new();
    for input in inputs {
        for warning in &input.diag.warnings {
            if !warnings.contains(warning) {
                warnings.push(warning.clone());
            }
        }
    }
    warnings.extend(computed.warnings);
    crate::analyze::diagnostics::Diagnostics {
        lines: inputs.iter().map(|input| input.diag.lines).sum(),
        records: inputs.iter().map(|input| input.records).sum(),
        skipped: inputs.iter().map(|input| input.skipped).sum(),
        unmatched_shapes: merge_shapes(
            inputs
                .iter()
                .flat_map(|input| input.diag.shapes.iter().cloned()),
        ),
        warnings,
        ..computed
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use serde_json::Value;

    use super::*;
    use crate::{
        analyze::{pipeline, report::SortBy},
        log::{LogRecord, Severity},
        store::{Status, ingest},
    };

    const OPTIONS: ReportOptions = ReportOptions {
        by: SortBy::Total,
        top: 20,
    };

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pgloupe-report-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn line(hour: usize, minute: usize, ms: f64, sql: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-10-14 {hour:02}:{minute:02}:00.000 UTC","pid":1,"session_id":"a.1","line_num":1,"error_severity":"LOG","message":"duration: {ms} ms  statement: {sql}","backend_type":"client backend"}}"#
        ) + "\n"
    }

    /// Three hours of statements; hour `h` only runs `SELECT {h}`-flavoured text.
    fn three_hours() -> String {
        let mut text = String::new();
        for hour in 10..13 {
            for minute in 0..5 {
                text.push_str(&line(
                    hour,
                    minute,
                    1.0 + f64::from(u8::try_from(minute).unwrap()),
                    &format!("SELECT * FROM t{hour}"),
                ));
            }
        }
        text
    }

    fn ingest_text(dir: &std::path::Path, text: &str) -> Store {
        let log = dir.join("a.json");
        fs::write(&log, text).unwrap();
        let mut store = Store::create(&dir.join("s.db")).unwrap();
        let out = ingest(&mut store, &[log], &pipeline::Options::new(), false);
        assert_eq!(out[0].status, Status::Ingested, "{:?}", out[0].status);
        store
    }

    fn without_sections(report: &Report) -> Value {
        let mut value: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        let map = value.as_object_mut().unwrap();
        for key in ["files", "diagnostics", "store"] {
            map.remove(key);
        }
        value
    }

    #[test]
    fn parse_time_accepts_rfc3339_and_dates() {
        assert_eq!(
            parse_time("2026-10-14T10:00:00Z").unwrap(),
            at("2026-10-14T10:00:00Z")
        );
        assert_eq!(
            parse_time("2026-10-14").unwrap(),
            at("2026-10-14T00:00:00Z")
        );
        assert_eq!(
            parse_time("2026-10-14T10:00:00-03:00").unwrap(),
            at("2026-10-14T13:00:00Z")
        );
        parse_time("yesterday").unwrap_err();
        parse_time("2026-10-14T10:00:00").unwrap_err();
    }

    #[test]
    fn snapping_floors_since_and_ceils_until() {
        assert_eq!(
            floor_hour(at("2026-10-14T10:20:00Z")),
            floor_hour(at("2026-10-14T10:00:00Z"))
        );
        assert_eq!(
            ceil_hour(at("2026-10-14T11:00:00Z")),
            floor_hour(at("2026-10-14T11:00:00Z"))
        );
        assert_eq!(
            ceil_hour(at("2026-10-14T11:00:01Z")),
            floor_hour(at("2026-10-14T12:00:00Z"))
        );
        assert_eq!(
            ceil_hour(at("2026-10-14T11:00:00.5Z")),
            floor_hour(at("2026-10-14T12:00:00Z"))
        );
    }

    #[test]
    fn an_empty_window_is_an_error() {
        let d = dir("empty-window");
        let store = Store::create(&d.join("s.db")).unwrap();
        let t = Some(at("2026-10-14T11:00:00Z"));
        let err = build(&store, &d.join("s.db"), t, t, &OPTIONS, 60).unwrap_err();
        assert!(matches!(err, ReportError::EmptyWindow), "{err}");
        // Both inside one hour snap to the same hour start/ceil: still non-empty.
        build(
            &store,
            &d.join("s.db"),
            Some(at("2026-10-14T11:10:00Z")),
            Some(at("2026-10-14T11:20:00Z")),
            &OPTIONS,
            60,
        )
        .unwrap();
    }

    #[test]
    fn buckets_must_be_whole_minutes() {
        let d = dir("bucket");
        let store = Store::create(&d.join("s.db")).unwrap();
        for bad in [30, 90, 0] {
            let err = build(&store, &d.join("s.db"), None, None, &OPTIONS, bad).unwrap_err();
            assert!(matches!(err, ReportError::Bucket), "{bad}: {err}");
        }
        for good in [60, 3_600] {
            build(&store, &d.join("s.db"), None, None, &OPTIONS, good).unwrap();
        }
    }

    #[test]
    fn report_over_the_whole_store_matches_analyze() {
        let d = dir("whole");
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/logs/sample.json");
        let mut store = Store::create(&d.join("s.db")).unwrap();
        ingest(
            &mut store,
            std::slice::from_ref(&fixture),
            &pipeline::Options::new(),
            false,
        );
        let stored = build(&store, &d.join("s.db"), None, None, &OPTIONS, 60).unwrap();
        let output = pipeline::run(&[fixture], &pipeline::Options::new()).unwrap();
        let analyzed = Report::build(&output, &OPTIONS);
        assert_eq!(without_sections(&stored), without_sections(&analyzed));
        assert_eq!(stored.files.len(), 1);
        assert!(stored.store.is_some());
    }

    #[test]
    fn a_window_selects_whole_hours() {
        let d = dir("window");
        let store = ingest_text(&d, &three_hours());
        let middle = build(
            &store,
            &d.join("s.db"),
            Some(at("2026-10-14T11:20:00Z")),
            Some(at("2026-10-14T11:40:00Z")),
            &OPTIONS,
            60,
        )
        .unwrap();
        assert_eq!(middle.overview.records, 5);
        assert_eq!(middle.statements.len(), 1);
        assert!(
            middle.statements[0].text.contains("t11"),
            "{}",
            middle.statements[0].text
        );
        let info = middle.store.unwrap();
        assert_eq!(info.since.as_deref(), Some("2026-10-14T11:00:00Z"));
        assert_eq!(info.until.as_deref(), Some("2026-10-14T12:00:00Z"));
        assert_eq!(info.inputs, 1);
    }

    #[test]
    fn a_window_equals_analyze_of_the_filtered_log() {
        let d = dir("filtered");
        let store = ingest_text(&d, &three_hours());
        let stored = build(
            &store,
            &d.join("s.db"),
            Some(at("2026-10-14T11:00:00Z")),
            Some(at("2026-10-14T12:00:00Z")),
            &OPTIONS,
            60,
        )
        .unwrap();
        let only_11: String = three_hours()
            .lines()
            .filter(|l| l.contains(" 11:"))
            .flat_map(|l| {
                [
                    l, "
",
                ]
            })
            .collect();
        let filtered = d.join("only11.json");
        fs::write(&filtered, only_11).unwrap();
        let output = pipeline::run(&[filtered], &pipeline::Options::new()).unwrap();
        assert_eq!(
            without_sections(&stored),
            without_sections(&Report::build(&output, &OPTIONS))
        );
    }

    #[test]
    fn an_empty_store_reports_no_data() {
        let d = dir("nodata");
        let store = Store::create(&d.join("s.db")).unwrap();
        let report = build(&store, &d.join("s.db"), None, None, &OPTIONS, 60).unwrap();
        assert!(report.statements.is_empty());
        assert_eq!(report.files.len(), 0);
        assert_eq!(report.window.first, None);
    }

    fn record(hour: i64, sql: &str) -> LogRecord {
        LogRecord::new(
            Timestamp::from_second(hour * 3_600 + 5).unwrap(),
            Severity::Log,
            format!("duration: 1.0 ms  statement: {sql}"),
        )
    }

    #[test]
    fn merging_slices_does_not_cap_early() {
        // Limit 2. Hours: h1 A3 B2, h2 C1, h3 C5. Over the whole run C=6 is a top-2 statement;
        // capping after each merge would drop it while it is small.
        let mut records = Vec::new();
        for (hour, sql, n) in [
            (1, "SELECT * FROM a", 3),
            (1, "SELECT * FROM b", 2),
            (2, "SELECT * FROM c", 1),
            (3, "SELECT * FROM c", 5),
        ] {
            records.extend((0..n).map(|_| record(hour, sql)));
        }
        let mut slices = Vec::new();
        for hour in 1..=3 {
            // As ingest leaves them: each hour capped at the run's limit.
            let mut acc = Accumulators::new(2, 60);
            for r in records.iter().filter(|r| r.ts.as_second() / 3_600 == hour) {
                acc.add_record(r);
            }
            slices.push(acc);
        }
        let merged = merge_slices(slices, 2);
        let mut whole = Accumulators::new(100, 60);
        for r in &records {
            whole.add_record(r);
        }
        whole.recap(2);
        assert_eq!(merged.queries, whole.queries);
        assert_eq!(merged.overflow, whole.overflow);
    }

    #[test]
    fn build_parts_returns_what_build_reports() {
        let d = dir("parts");
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/logs/sample.json");
        let mut store = Store::create(&d.join("s.db")).unwrap();
        ingest(
            &mut store,
            std::slice::from_ref(&fixture),
            &pipeline::Options::new(),
            false,
        );
        let path = d.join("s.db");
        let built = build(&store, &path, None, None, &OPTIONS, 60).unwrap();
        let (report, acc) = build_parts(&store, &path, None, None, &OPTIONS, 60).unwrap();
        assert_eq!(report.to_json().unwrap(), built.to_json().unwrap());
        assert_eq!(acc.totals.records, report.overview.records);
    }
}
