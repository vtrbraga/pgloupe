//! The human-readable report.

use std::time::Duration;

use super::report::{
    CheckpointRow, ErrorRow, Quantiles, Report, StatementRow, TableRow, TimelineRow,
};

/// Longest query or message text shown.
const MAX_TEXT_CHARS: usize = 80;

const STATEMENT_HEADERS: [&str; 9] = [
    "calls", "total_ms", "mean_ms", "p50_ms", "p95_ms", "p99_ms", "max_ms", "share%", "query",
];
const STATEMENT_RIGHT: [bool; 9] = [true, true, true, true, true, true, true, true, false];

/// Renders `report` as text. `elapsed` is shown in the header when given (tests pass `None` to
/// keep the output deterministic). The result ends with a newline.
#[must_use]
pub fn text(report: &Report, elapsed: Option<Duration>) -> String {
    let mut lines = header(report, elapsed);
    section(&mut lines, "diagnostics", diagnostics(report));
    section(&mut lines, "overview", overview(report));
    section(
        &mut lines,
        &format!("top statements (by {})", report.statements_by),
        statements(&report.statements),
    );
    section(&mut lines, "errors", errors(&report.errors));
    section(&mut lines, "locks", locks(report));
    section(&mut lines, "temp files", temp_files(report));
    section(&mut lines, "connections", connections(report));
    section(&mut lines, "checkpoints", checkpoints(&report.checkpoints));
    section(&mut lines, "autovacuum", autovacuum(&report.autovacuum));
    section(
        &mut lines,
        &format!(
            "timeline (bucket {})",
            bucket_label(report.timeline.bucket_secs)
        ),
        timeline(&report.timeline.buckets),
    );
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn section(lines: &mut Vec<String>, title: &str, body: Vec<String>) {
    lines.push(String::new());
    lines.push(title.to_owned());
    if body.is_empty() {
        lines.push("  none".to_owned());
    } else {
        lines.extend(body.into_iter().map(|line| format!("  {line}")));
    }
}

fn header(report: &Report, elapsed: Option<Duration>) -> Vec<String> {
    let mut lines = match &report.store {
        Some(store) => vec![
            "pgloupe logs report".to_owned(),
            format!("store:      {}", store.path),
            format!(
                "range:      {} -> {}",
                store.since.as_deref().unwrap_or("all"),
                store.until.as_deref().unwrap_or("all")
            ),
            format!("inputs:     {}", report.files.len()),
        ],
        None => vec![
            "pgloupe logs analyze".to_owned(),
            format!("files:      {}", report.files.len()),
        ],
    };
    for file in &report.files {
        let prefix = match (&file.prefix, file.prefix_source.as_str(), file.prefix_match) {
            (Some(prefix), "inferred", Some(percent)) => {
                format!(" prefix {prefix} (inferred {percent:.1}%)")
            }
            (Some(prefix), source, _) => format!(" prefix {prefix} ({source})"),
            (None, _, _) => String::new(),
        };
        let line = format!("  {} {} {} B{prefix}", file.path, file.format, file.bytes);
        lines.push(line);
    }
    lines.push(match (&report.window.first, &report.window.last) {
        (Some(first), Some(last)) => format!("window:     {first} -> {last}"),
        _ => "window:     none".to_owned(),
    });
    if let Some(elapsed) = elapsed {
        lines.push(format!("elapsed:    {:.1}s", elapsed.as_secs_f64()));
    }
    lines
}

fn diagnostics(report: &Report) -> Vec<String> {
    let d = &report.diagnostics;
    let mut lines = vec![format!(
        "lines {}, records {}, skipped {}, unparsed statements {}",
        d.lines, d.records, d.skipped, d.unparsed_statements
    )];
    if !d.unmatched_shapes.is_empty() {
        lines.push("unmatched line shapes:".to_owned());
        for shape in &d.unmatched_shapes {
            lines.push(format!(
                "  {}x  line {}  {}",
                shape.count, shape.first_line, shape.shape
            ));
        }
    }
    lines.extend(
        d.warnings
            .iter()
            .map(|warning| format!("warning: {warning}")),
    );
    lines
}

fn overview(report: &Report) -> Vec<String> {
    let counts = |map: &std::collections::BTreeMap<String, u64>| {
        if map.is_empty() {
            "none".to_owned()
        } else {
            map.iter()
                .map(|(name, n)| format!("{name}={n}"))
                .collect::<Vec<_>>()
                .join(" ")
        }
    };
    vec![
        format!("records {}", report.overview.records),
        format!("events: {}", counts(&report.overview.events)),
        format!("levels: {}", counts(&report.overview.levels)),
    ]
}

fn num(value: f64) -> String {
    format!("{value:.2}")
}

fn opt(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), num)
}

fn cut(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_TEXT_CHARS {
        collapsed
    } else {
        let mut short: String = collapsed.chars().take(MAX_TEXT_CHARS - 1).collect();
        short.push('…');
        short
    }
}

fn statements(rows: &[StatementRow]) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut cells: Vec<Vec<String>> =
        vec![STATEMENT_HEADERS.iter().map(|h| (*h).to_owned()).collect()];
    for row in rows {
        cells.push(vec![
            row.calls.to_string(),
            num(row.total_ms),
            opt(row.mean_ms),
            opt(row.p50_ms),
            opt(row.p95_ms),
            opt(row.p99_ms),
            opt(row.max_ms),
            format!("{:.1}", row.share_pct),
            cut(&row.text),
        ]);
    }
    align(&cells, &STATEMENT_RIGHT)
}

/// Pads columns to their widest cell; numeric columns are right-aligned, lines are trimmed.
fn align(rows: &[Vec<String>], right: &[bool]) -> Vec<String> {
    let mut widths = vec![0_usize; right.len()];
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    rows.iter()
        .map(|row| {
            let parts: Vec<String> = row
                .iter()
                .zip(&widths)
                .zip(right)
                .map(|((cell, &width), &right_aligned)| {
                    if right_aligned {
                        format!("{cell:>width$}")
                    } else {
                        format!("{cell:<width$}")
                    }
                })
                .collect();
            parts.join("  ").trim_end().to_owned()
        })
        .collect()
}

fn errors(rows: &[ErrorRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            format!(
                "{}x  {} {} first {}  {}",
                row.count,
                row.level,
                row.sqlstate.as_deref().unwrap_or("-"),
                row.first_seen,
                cut(row.first_message.lines().next().unwrap_or(""))
            )
        })
        .collect()
}

fn locks(report: &Report) -> Vec<String> {
    let l = &report.locks;
    vec![format!(
        "waits {}, acquired {}, total wait {} ms, longest {} ms, deadlocks {}",
        l.waits,
        l.acquired,
        num(l.total_wait_ms),
        num(l.max_wait_ms),
        l.deadlocks
    )]
}

fn temp_files(report: &Report) -> Vec<String> {
    let t = &report.temp_files;
    vec![format!(
        "files {}, bytes {}, largest {}",
        t.files, t.bytes, t.max_bytes
    )]
}

fn quantile_line(label: &str, q: &Quantiles) -> String {
    format!(
        "{label} p50 {} ms, p95 {} ms, max {} ms",
        opt(q.p50),
        opt(q.p95),
        opt(q.max)
    )
}

fn connections(report: &Report) -> Vec<String> {
    let c = &report.connections;
    let mut lines = vec![format!(
        "received {}, authenticated {}, authorized {}, disconnections {}",
        c.received, c.authenticated, c.authorized, c.disconnections
    )];
    if c.session_ms.count > 0 {
        lines.push(quantile_line("session time", &c.session_ms));
    }
    lines
}

fn checkpoints(rows: &[CheckpointRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let reasons = row
                .reasons
                .iter()
                .map(|(reason, n)| format!("{reason}={n}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "{}: started {}, completed {}, buffers {}; total p50 {} ms, p95 {} ms; reasons: {}",
                row.kind,
                row.started,
                row.completed,
                row.buffers,
                opt(row.total_ms.p50),
                opt(row.total_ms.p95),
                if reasons.is_empty() { "-" } else { &reasons }
            )
        })
        .collect()
}

fn autovacuum(rows: &[TableRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            format!(
                "{}  vacuums {}, analyzes {}, total {} ms, longest {} ms",
                row.table,
                row.vacuums,
                row.analyzes,
                num(row.total_ms),
                num(row.max_ms)
            )
        })
        .collect()
}

/// Picks one counter out of a timeline row.
type Count = fn(&TimelineRow) -> u64;

fn timeline(rows: &[TimelineRow]) -> Vec<String> {
    let series: [(&str, Count); 4] = [
        ("statements", |r| r.statements),
        ("errors", |r| r.errors),
        ("connections", |r| r.connections),
        ("lock waits", |r| r.lock_waits),
    ];
    series
        .iter()
        .filter_map(|(name, count)| {
            let busiest = rows
                .iter()
                .rev()
                .max_by_key(|row| count(row))
                .filter(|row| count(row) > 0)?;
            Some(format!(
                "busiest {name}: {} ({})",
                busiest.start,
                count(busiest)
            ))
        })
        .collect()
}

fn bucket_label(secs: i64) -> String {
    if secs >= 3_600 && secs % 3_600 == 0 {
        format!("{}h", secs / 3_600)
    } else if secs >= 60 && secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jiff::Timestamp;

    use super::*;
    use crate::{
        analyze::{
            acc::Accumulators,
            pipeline::{FileSummary, Output, PrefixSource},
            report::{ReportOptions, SortBy},
        },
        log::{DecodeStats, Format, LogRecord, Severity},
    };

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(1_760_000_000 + second).unwrap()
    }

    fn report(top: usize) -> Report {
        let mut acc = Accumulators::new(100, 60);
        for ms in [10.0, 20.0, 30.0] {
            acc.add_record(&LogRecord::new(
                at(0),
                Severity::Log,
                format!("duration: {ms} ms  statement: SELECT * FROM film WHERE film_id = 1"),
            ));
        }
        let mut err = LogRecord::new(at(1), Severity::Error, "relation \"x\" does not exist");
        err.sqlstate = Some("42P01".to_owned());
        acc.add_record(&err);
        acc.add_record(&LogRecord::new(
            at(2),
            Severity::Log,
            "temporary file: path \"p\", size 4096",
        ));
        acc.add_record(&LogRecord::new(
            at(3),
            Severity::Log,
            "checkpoint starting: time",
        ));
        let mut stats = DecodeStats {
            lines: 6,
            records: 6,
            ..DecodeStats::default()
        };
        stats.skip_with(9, "no match", Some("garbage 12"));
        let files = vec![FileSummary {
            path: PathBuf::from("a.log"),
            bytes: 321,
            format: Format::Stderr,
            prefix: Some("%m [%p] ".to_owned()),
            prefix_source: PrefixSource::Inferred {
                matched: 99,
                sampled: 100,
            },
            stats,
            content: None,
        }];
        Report::build(
            &Output { acc, files },
            &ReportOptions {
                by: SortBy::Total,
                top,
            },
        )
    }

    /// Whitespace-collapsed lines, so tests do not depend on column widths.
    fn squash(text: &str) -> Vec<String> {
        text.lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    fn has(lines: &[String], expected: &str) -> bool {
        lines.iter().any(|l| l == expected)
    }

    #[test]
    fn store_header_names_store_range_and_inputs() {
        let mut report = report(10);
        report.store = Some(crate::analyze::report::StoreInfo {
            path: "logs.db".to_owned(),
            since: Some("2026-10-14T11:00:00Z".to_owned()),
            until: None,
            inputs: 1,
        });
        let lines = squash(&text(&report, None));
        assert_eq!(lines[0], "pgloupe logs report");
        assert_eq!(lines[1], "store: logs.db");
        assert_eq!(lines[2], "range: 2026-10-14T11:00:00Z -> all");
        assert_eq!(lines[3], "inputs: 1");
        assert!(
            has(&lines, "a.log stderr 321 B prefix %m [%p] (inferred 99.0%)"),
            "{lines:#?}"
        );
    }

    #[test]
    fn header_lists_files_prefix_and_window() {
        let lines = squash(&text(&report(10), None));
        assert_eq!(lines[0], "pgloupe logs analyze");
        assert!(
            has(&lines, "a.log stderr 321 B prefix %m [%p] (inferred 99.0%)"),
            "{lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("window: 2025-")),
            "{lines:#?}"
        );
        assert!(!lines.iter().any(|l| l.starts_with("elapsed:")));
        let timed = squash(&text(
            &report(10),
            Some(std::time::Duration::from_millis(1_500)),
        ));
        assert!(has(&timed, "elapsed: 1.5s"), "{timed:#?}");
    }

    #[test]
    fn diagnostics_show_counts_shapes_and_warnings() {
        let lines = squash(&text(&report(10), None));
        assert!(
            has(
                &lines,
                "lines 6, records 6, skipped 1, unparsed statements 0"
            ),
            "{lines:#?}"
        );
        assert!(has(&lines, "1x line 9 garbage #"), "{lines:#?}");
        assert!(
            lines.iter().any(|l| l.starts_with("warning: not seen:")),
            "{lines:#?}"
        );
    }

    #[test]
    fn the_statement_table_is_formatted() {
        let lines = squash(&text(&report(10), None));
        assert!(has(&lines, "top statements (by total)"), "{lines:#?}");
        assert!(
            has(
                &lines,
                "calls total_ms mean_ms p50_ms p95_ms p99_ms max_ms share% query"
            ),
            "{lines:#?}"
        );
        let row = lines
            .iter()
            .find(|l| l.ends_with("SELECT * FROM film WHERE film_id = $1"))
            .unwrap();
        assert!(row.starts_with("3 60.00 20.00 "), "{row}");
        assert!(row.contains(" 30.00 100.0 "), "{row}");
    }

    #[test]
    fn long_query_text_is_cut() {
        let mut acc = Accumulators::new(10, 60);
        let long = format!("SELECT {}", "x".repeat(300));
        acc.add_record(&LogRecord::new(
            at(0),
            Severity::Log,
            format!("duration: 1 ms  statement: {long}"),
        ));
        let r = Report::build(
            &Output { acc, files: vec![] },
            &ReportOptions {
                by: SortBy::Total,
                top: 1,
            },
        );
        let out = text(&r, None);
        let row = out.lines().find(|l| l.contains("SELECT")).unwrap();
        let cell = row.rsplit("  ").next().unwrap();
        assert_eq!(cell.chars().count(), 80);
        assert!(cell.ends_with('…'));
    }

    #[test]
    fn sections_report_their_numbers() {
        let lines = squash(&text(&report(10), None));
        assert!(
            has(
                &lines,
                "1x ERROR 42P01 first 2025-10-09T08:53:21Z relation \"x\" does not exist"
            ) || lines.iter().any(|l| l.starts_with("1x ERROR 42P01 first ")),
            "{lines:#?}"
        );
        assert!(
            has(&lines, "files 1, bytes 4096, largest 4096"),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("checkpoint: started 1, completed 0")),
            "{lines:#?}"
        );
        assert!(
            has(
                &lines,
                "waits 0, acquired 0, total wait 0.00 ms, longest 0.00 ms, deadlocks 0"
            ),
            "{lines:#?}"
        );
    }

    #[test]
    fn empty_sections_say_none() {
        let empty = Report::build(
            &Output {
                acc: Accumulators::new(10, 60),
                files: vec![],
            },
            &ReportOptions {
                by: SortBy::Total,
                top: 10,
            },
        );
        let lines = squash(&text(&empty, None));
        let none = lines.iter().filter(|l| *l == "none").count();
        assert!(none >= 5, "{lines:#?}");
    }

    #[test]
    fn top_zero_still_prints_the_table_header_section() {
        let lines = squash(&text(&report(0), None));
        assert!(has(&lines, "top statements (by total)"), "{lines:#?}");
    }
}
