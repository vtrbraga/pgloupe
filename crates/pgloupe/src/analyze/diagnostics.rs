//! The "why is this report empty or odd" block: counts, unmatched line shapes, setting gaps.

use serde::{Deserialize, Serialize};

use super::pipeline::{FileSummary, Output, PrefixSource};
use crate::log::Format;

/// How many unmatched shapes are listed.
const TOP_SHAPES: usize = 5;

/// One kind of skipped line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeRow {
    pub shape: String,
    pub count: u64,
    pub first_line: u64,
}

/// What the run could not understand, and settings that probably explain it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diagnostics {
    pub lines: u64,
    pub records: u64,
    pub skipped: u64,
    pub unparsed_statements: u64,
    /// Statements and attributions that landed in the fingerprint overflow bucket.
    pub overflow_calls: u64,
    pub unmatched_shapes: Vec<ShapeRow>,
    pub warnings: Vec<String>,
}

/// Builds the diagnostics of a finished run.
#[must_use]
pub fn build(output: &Output) -> Diagnostics {
    let lines: u64 = output.files.iter().map(|f| f.stats.lines).sum();
    let skipped: u64 = output.files.iter().map(|f| f.stats.skipped).sum();
    let totals = &output.acc.totals;
    let mut diagnostics = Diagnostics {
        lines,
        records: output.files.iter().map(|f| f.stats.records).sum(),
        skipped,
        unparsed_statements: totals.unparsed_statements,
        overflow_calls: output.acc.overflow.calls,
        unmatched_shapes: unmatched_shapes(&output.files),
        warnings: Vec::new(),
    };
    prefix_warnings(&output.files, &mut diagnostics.warnings);
    label_warning(&output.files, &mut diagnostics.warnings);
    inferred_prefix_warning(&output.files, &mut diagnostics.warnings);
    absence_warnings(output, &mut diagnostics.warnings);
    if output.acc.overflow.calls > 0 {
        diagnostics.warnings.push(format!(
            "{} statements fell into the overflow bucket — raise --max-fingerprints to keep more distinct statements",
            output.acc.overflow.calls
        ));
    }
    if totals.unparsed_statements > 0 {
        diagnostics.warnings.push(format!(
            "{} statements could not be parsed and were grouped by their text",
            totals.unparsed_statements
        ));
    }
    if output.files.iter().any(|f| {
        f.stats
            .first_skipped
            .as_ref()
            .is_some_and(|s| s.reason.starts_with("unknown timezone"))
    }) {
        diagnostics.warnings.push(
            "some lines have a timezone abbreviation pgloupe cannot resolve — pass --log-tz with a numeric offset"
                .to_owned(),
        );
    }
    diagnostics
}

fn unmatched_shapes(files: &[FileSummary]) -> Vec<ShapeRow> {
    let mut merged: std::collections::BTreeMap<String, ShapeRow> =
        std::collections::BTreeMap::new();
    for file in files {
        for shape in file.stats.top_shapes(usize::MAX) {
            let row = merged.entry(shape.text.clone()).or_insert(ShapeRow {
                shape: shape.text,
                count: 0,
                first_line: shape.first_line,
            });
            row.count += shape.count;
            row.first_line = row.first_line.min(shape.first_line);
        }
    }
    let mut rows: Vec<ShapeRow> = merged.into_values().collect();
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then(a.first_line.cmp(&b.first_line))
            .then_with(|| a.shape.cmp(&b.shape))
    });
    rows.truncate(TOP_SHAPES);
    rows
}

fn prefix_warnings(files: &[FileSummary], warnings: &mut Vec<String>) {
    let prefixes: Vec<&str> = files
        .iter()
        .filter(|f| f.format == Format::Stderr)
        .filter_map(|f| f.prefix.as_deref())
        .collect();
    if prefixes
        .iter()
        .any(|p| !p.contains("%p") && !p.contains("%c"))
    {
        warnings.push(
            "the log_line_prefix has no %p (process id) — entries of different backends cannot be told apart reliably"
                .to_owned(),
        );
    }
    if prefixes.iter().any(|p| !p.contains("%e")) {
        warnings.push(
            "the log_line_prefix has no %e (SQLSTATE) — errors cannot be told apart by code in this stderr log"
                .to_owned(),
        );
    }
    if prefixes
        .iter()
        .any(|p| !p.contains("%u") && !p.contains("%d") && !p.contains("%a"))
    {
        warnings.push(
            "the log_line_prefix has no %u, %d or %a — per-user, database and application breakdowns are unavailable"
                .to_owned(),
        );
    }
}

fn inferred_prefix_warning(files: &[FileSummary], warnings: &mut Vec<String>) {
    for file in files {
        let seen = file.stats.records + file.stats.skipped;
        if matches!(file.prefix_source, PrefixSource::Inferred { .. })
            && file.stats.skipped > 0
            && file.stats.skipped * 20 > seen
        {
            warnings.push(format!(
                "{}: the log_line_prefix was inferred and {} of {} lines did not fit it — pass --prefix with the server's setting",
                file.path.display(),
                file.stats.skipped,
                seen
            ));
        }
    }
}

fn label_warning(files: &[FileSummary], warnings: &mut Vec<String>) {
    let no_label: u64 = files.iter().map(|f| f.stats.no_label).sum();
    let seen: u64 = files
        .iter()
        .map(|f| f.stats.records + f.stats.skipped)
        .sum();
    if no_label > 0 && no_label * 20 >= seen {
        warnings.push(
            "many lines match the prefix but have an unknown severity label — lc_messages is probably not English; set lc_messages = 'C'"
                .to_owned(),
        );
    }
}

fn absence_warnings(output: &Output, warnings: &mut Vec<String>) {
    let totals = &output.acc.totals;
    let skipped: u64 = output.files.iter().map(|f| f.stats.skipped).sum();
    if totals.records == 0 {
        if skipped > 0 {
            warnings.push(format!(
                "nothing could be decoded: all {skipped} non-blank lines were skipped — check --format and --prefix (and lc_messages for non-English servers)"
            ));
        }
        return;
    }
    let (calls, timed) = output
        .acc
        .queries
        .values()
        .chain(std::iter::once(&output.acc.overflow))
        .fold((0, 0), |(calls, timed), q| {
            (calls + q.calls, timed + q.timed)
        });
    if timed < calls {
        warnings.push(format!(
            "{} of {calls} statements have no duration — log_min_duration_statement is needed for timings (log_statement alone logs statements without them)",
            calls - timed
        ));
    }
    let seen = |kind: &str| totals.events.get(kind).copied().unwrap_or(0) > 0;
    if !(seen("statement") || seen("execute") || seen("duration")) {
        warnings.push(
            "no statement durations were seen — log_min_duration_statement may be unset (or no statement was slow enough)"
                .to_owned(),
        );
    }
    let bare = totals.events.get("duration").copied().unwrap_or(0);
    if bare > 0 {
        warnings.push(format!(
            "{bare} durations were logged without their statement text (log_duration, or log_statement together with log_min_duration_statement = 0) — they cannot be attributed to a statement; use log_min_duration_statement alone"
        ));
    }
    let mut missing = Vec::new();
    if !(seen("connection_received") || seen("connection_authorized") || seen("disconnection")) {
        missing.push("connections (log_connections / log_disconnections)");
    }
    if !(seen("checkpoint_start") || seen("checkpoint_done")) {
        missing.push("checkpoints (log_checkpoints)");
    }
    if !seen("temp_file") {
        missing.push("temp files (log_temp_files)");
    }
    if !(seen("lock_wait") || seen("lock_acquired") || seen("deadlock")) {
        missing.push("lock waits (log_lock_waits)");
    }
    if !missing.is_empty() {
        warnings.push(format!(
            "not seen: {} — the setting may be off, or none happened",
            missing.join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jiff::Timestamp;

    use super::*;
    use crate::{
        analyze::acc::Accumulators,
        log::{DecodeStats, LogRecord, Severity},
    };

    fn summary(format: Format, prefix: Option<&str>, stats: DecodeStats) -> FileSummary {
        FileSummary {
            path: PathBuf::from("x.log"),
            bytes: 10,
            format,
            prefix: prefix.map(str::to_owned),
            prefix_source: if prefix.is_some() {
                PrefixSource::Given
            } else {
                PrefixSource::NotApplicable
            },
            stats,
            content: None,
        }
    }

    fn output(records: &[&str], files: Vec<FileSummary>) -> Output {
        let mut acc = Accumulators::new(100, 60);
        for message in records {
            acc.add_record(&LogRecord::new(
                Timestamp::UNIX_EPOCH,
                Severity::Log,
                *message,
            ));
        }
        Output { acc, files }
    }

    fn has(d: &Diagnostics, needle: &str) -> bool {
        d.warnings.iter().any(|w| w.contains(needle))
    }

    #[test]
    fn counts_and_shapes_come_from_the_files() {
        let mut stats = DecodeStats {
            lines: 10,
            records: 7,
            ..DecodeStats::default()
        };
        stats.skip_with(3, "no match", Some("garbage 1"));
        stats.skip_with(4, "no match", Some("garbage 22"));
        stats.skip_with(5, "no match", Some("other"));
        let d = build(&output(
            &["statement: SELECT 1"],
            vec![summary(Format::Stderr, Some("%m [%p] "), stats)],
        ));
        assert_eq!((d.lines, d.records, d.skipped), (10, 7, 3));
        assert_eq!(
            d.unmatched_shapes[0],
            ShapeRow {
                shape: "garbage #".to_owned(),
                count: 2,
                first_line: 3
            }
        );
        assert_eq!(d.unmatched_shapes.len(), 2);
    }

    #[test]
    fn a_prefix_without_pid_or_names_warns() {
        let d = build(&output(
            &["statement: SELECT 1"],
            vec![summary(Format::Stderr, Some("%m "), DecodeStats::default())],
        ));
        assert!(has(&d, "%p"), "{:?}", d.warnings);
        assert!(has(&d, "user"), "{:?}", d.warnings);
        let ok = build(&output(
            &["statement: SELECT 1"],
            vec![summary(
                Format::Stderr,
                Some("%m [%p] %u@%d/%a "),
                DecodeStats::default(),
            )],
        ));
        assert!(!has(&ok, "%p"), "{:?}", ok.warnings);
        assert!(!has(&ok, "user"), "{:?}", ok.warnings);
    }

    #[test]
    fn csv_and_json_do_not_get_prefix_warnings() {
        let d = build(&output(
            &["statement: SELECT 1"],
            vec![summary(Format::Json, None, DecodeStats::default())],
        ));
        assert!(!has(&d, "%p") && !has(&d, "%u"), "{:?}", d.warnings);
    }

    #[test]
    fn non_english_labels_are_detected() {
        let stats = DecodeStats {
            lines: 100,
            records: 80,
            skipped: 20,
            no_label: 20,
            ..DecodeStats::default()
        };
        let d = build(&output(
            &["statement: SELECT 1"],
            vec![summary(Format::Stderr, Some("%m [%p] "), stats)],
        ));
        assert!(has(&d, "lc_messages"), "{:?}", d.warnings);
        let few = DecodeStats {
            lines: 1000,
            records: 990,
            skipped: 10,
            no_label: 2,
            ..DecodeStats::default()
        };
        let d = build(&output(
            &["statement: SELECT 1"],
            vec![summary(Format::Stderr, Some("%m [%p] "), few)],
        ));
        assert!(!has(&d, "lc_messages"), "{:?}", d.warnings);
    }

    #[test]
    fn missing_durations_and_event_kinds_are_listed() {
        let d = build(&output(&["hello", "world"], vec![]));
        assert!(has(&d, "log_min_duration_statement"), "{:?}", d.warnings);
        assert!(has(&d, "connections"), "{:?}", d.warnings);
        assert!(has(&d, "checkpoints"), "{:?}", d.warnings);
        assert!(has(&d, "temp files"), "{:?}", d.warnings);
        assert!(has(&d, "lock waits"), "{:?}", d.warnings);
        assert!(has(&d, "or none happened"), "{:?}", d.warnings);
    }

    #[test]
    fn nothing_is_flagged_when_nothing_was_read() {
        let d = build(&output(&[], vec![]));
        assert_eq!(d.warnings, Vec::<String>::new());
    }

    #[test]
    fn present_event_kinds_are_not_flagged() {
        let d = build(&output(
            &[
                "duration: 1 ms  statement: SELECT 1",
                "connection authorized: user=u database=d",
                "checkpoint starting: time",
                "temporary file: path \"p\", size 5",
                "process 1 still waiting for ShareLock on transaction 2 after 200.0 ms",
            ],
            vec![],
        ));
        assert_eq!(d.warnings, Vec::<String>::new());
    }

    #[test]
    fn reports_the_overflow_warning() {
        let mut acc = Accumulators::new(1, 60);
        for sql in ["SELECT a FROM t1", "SELECT a FROM t2", "SELECT a FROM t3"] {
            acc.add_record(&LogRecord::new(
                Timestamp::UNIX_EPOCH,
                Severity::Log,
                format!("duration: 1 ms  statement: {sql}"),
            ));
        }
        let d = build(&Output { acc, files: vec![] });
        assert!(d.overflow_calls > 0);
        assert!(has(&d, "max-fingerprints"), "{:?}", d.warnings);
    }

    #[test]
    fn unparsed_statements_and_unknown_zones_warn() {
        let mut stats = DecodeStats::default();
        stats.skip_with(
            1,
            "unknown timezone abbreviation 'BRT'; pass --log-tz +HH[:MM]",
            None,
        );
        let d = build(&output(
            &["duration: 1 ms  statement: SELEC 1"],
            vec![summary(Format::Csv, None, stats)],
        ));
        assert_eq!(d.unparsed_statements, 1);
        assert!(has(&d, "could not be parsed"), "{:?}", d.warnings);
        assert!(has(&d, "--log-tz"), "{:?}", d.warnings);
    }

    #[test]
    fn an_inferred_prefix_that_leaves_many_lines_unread_is_questioned() {
        let mut stats = DecodeStats {
            lines: 100,
            records: 80,
            skipped: 20,
            ..DecodeStats::default()
        };
        stats.skip_with(3, "x", Some("alice@app LOG:  hello"));
        let mut file = summary(Format::Stderr, Some("%m [%p] "), stats);
        file.prefix_source = PrefixSource::Inferred {
            matched: 20,
            sampled: 20,
        };
        let d = build(&output(&["statement: SELECT 1"], vec![file]));
        assert!(has(&d, "inferred"), "{:?}", d.warnings);
        assert!(has(&d, "--prefix"), "{:?}", d.warnings);
        let mut ok = summary(
            Format::Stderr,
            Some("%m [%p] "),
            DecodeStats {
                lines: 100,
                records: 99,
                skipped: 1,
                ..DecodeStats::default()
            },
        );
        ok.prefix_source = PrefixSource::Inferred {
            matched: 20,
            sampled: 20,
        };
        let d = build(&output(&["statement: SELECT 1"], vec![ok]));
        assert!(!has(&d, "was inferred"), "{:?}", d.warnings);
    }

    #[test]
    fn a_stderr_prefix_without_sqlstate_warns_about_error_grouping() {
        let d = build(&output(
            &["statement: SELECT 1"],
            vec![summary(
                Format::Stderr,
                Some("%m [%p] "),
                DecodeStats::default(),
            )],
        ));
        assert!(has(&d, "%e"), "{:?}", d.warnings);
        let with = build(&output(
            &["statement: SELECT 1"],
            vec![summary(
                Format::Stderr,
                Some("%m [%p] %e "),
                DecodeStats::default(),
            )],
        ));
        assert!(!has(&with, "%e"), "{:?}", with.warnings);
        let csv = build(&output(
            &["statement: SELECT 1"],
            vec![summary(Format::Csv, None, DecodeStats::default())],
        ));
        assert!(!has(&csv, "%e"), "{:?}", csv.warnings);
    }

    #[test]
    fn durations_without_statement_text_are_explained() {
        let d = build(&output(
            &[
                "duration: 1.0 ms",
                "duration: 2.0 ms",
                "statement: SELECT 1",
            ],
            vec![],
        ));
        assert!(has(&d, "without their statement"), "{:?}", d.warnings);
        let none = build(&output(&["duration: 1 ms  statement: SELECT 1"], vec![]));
        assert!(
            !has(&none, "without their statement"),
            "{:?}",
            none.warnings
        );
    }

    #[test]
    fn nothing_decoded_from_a_non_empty_input_is_called_out() {
        let mut stats = DecodeStats {
            lines: 50,
            records: 0,
            ..DecodeStats::default()
        };
        for line in 1..=50 {
            stats.skip_with(line, "no match", Some("junk"));
        }
        let d = build(&output(
            &[],
            vec![summary(Format::Stderr, Some("%t [%p]: "), stats)],
        ));
        assert!(has(&d, "nothing could be decoded"), "{:?}", d.warnings);
        assert!(has(&d, "--prefix"), "{:?}", d.warnings);
    }

    #[test]
    fn statements_without_durations_are_called_out() {
        let d = build(&output(
            &["statement: SELECT 1", "statement: SELECT 2"],
            vec![],
        ));
        assert!(has(&d, "have no duration"), "{:?}", d.warnings);
        let timed = build(&output(&["duration: 1 ms  statement: SELECT 1"], vec![]));
        assert!(!has(&timed, "have no duration"), "{:?}", timed.warnings);
    }
}
