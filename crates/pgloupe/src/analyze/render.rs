//! The human-readable report.

use std::time::Duration;

use super::{
    histogram::HistogramBucket,
    report::{
        CancelledReport, CheckpointRow, DdlReport, DimensionRow, DisconnectsReport, ErrorRow,
        KindRow, MessageRow, PoolerReport, Quantiles, Report, SlowestReport, StatementRow,
        TableRow, TimelineRow,
    },
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
    lines.extend(sections(report, true));
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The sections of the text report after its header.
///
/// Each is preceded by a blank line: diagnostics, overview, the statement table (when
/// `statements_table`), errors, locks, temp files, connections, checkpoints, autovacuum and the
/// timeline.
#[must_use]
pub fn sections(report: &Report, statements_table: bool) -> Vec<String> {
    let mut lines = Vec::new();
    section(&mut lines, "diagnostics", diagnostics(report));
    section(&mut lines, "overview", overview(report));
    if statements_table {
        section(
            &mut lines,
            &format!("top statements (by {})", report.statements_by),
            statements(&report.statements),
        );
    }
    let phase_lines = phases(&report.statements);
    if !phase_lines.is_empty() {
        section(&mut lines, "phases", phase_lines);
    }
    section(&mut lines, "errors", errors(&report.errors));
    section(&mut lines, "cancelled", cancelled(&report.cancelled));
    section(&mut lines, "slowest executions", slowest(&report.slowest));
    section(&mut lines, "locks", locks(report));
    section(&mut lines, "temp files", temp_files(report));
    section(&mut lines, "connections", connections(report));
    section(&mut lines, "checkpoints", checkpoints(&report.checkpoints));
    section(&mut lines, "autovacuum", autovacuum(&report.autovacuum));
    let abrupt = disconnects(&report.disconnects);
    if !abrupt.is_empty() {
        section(&mut lines, "abrupt endings", abrupt);
    }
    let schema = ddl(&report.ddl);
    if !schema.is_empty() {
        section(&mut lines, "schema changes", schema);
    }
    if let Some(pooler) = &report.pooler {
        section(&mut lines, "pgbouncer (experimental)", pooler_lines(pooler));
    }
    let chatter = messages(&report.messages);
    if !chatter.is_empty() {
        section(&mut lines, "most frequent other messages", chatter);
    }
    section(&mut lines, "query types", kinds(&report.kinds));
    section(
        &mut lines,
        "statement durations",
        histogram(&report.histograms.statements),
    );
    section(
        &mut lines,
        "session durations",
        histogram(&report.histograms.sessions),
    );
    for (title, rows) in [
        ("by database", &report.dimensions.database),
        ("by user", &report.dimensions.user),
        ("by application", &report.dimensions.application),
        ("by host", &report.dimensions.host),
    ] {
        section(&mut lines, title, dimension(rows));
    }
    section(
        &mut lines,
        &format!(
            "timeline (bucket {})",
            bucket_label(report.timeline.bucket_secs)
        ),
        timeline(&report.timeline.buckets),
    );
    lines
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
        ]
        .into_iter()
        .chain(
            store
                .filter
                .iter()
                .map(|filter| format!("filter:     {filter} (applied when ingested)")),
        )
        .collect(),
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
    if d.filtered > 0 {
        lines.push(format!(
            "filtered out {} records: this report covers only what the filters kept",
            d.filtered
        ));
    }
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

pub(crate) fn num(value: f64) -> String {
    format!("{value:.2}")
}

pub(crate) fn opt(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), num)
}

pub(crate) fn cut(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_TEXT_CHARS {
        collapsed
    } else {
        let mut short: String = collapsed.chars().take(MAX_TEXT_CHARS - 1).collect();
        short.push('…');
        short
    }
}

/// Statements that spent time parsing, binding or fetching, the costliest first.
fn phases(rows: &[StatementRow]) -> Vec<String> {
    let mut with: Vec<&StatementRow> = rows.iter().filter(|r| r.phases.is_some()).collect();
    if with.is_empty() {
        return Vec::new();
    }
    with.sort_by(|a, b| {
        let cost = |r: &StatementRow| {
            r.phases.map_or(0.0, |p| {
                p.parse.total_ms + p.bind.total_ms + p.fetch.total_ms
            })
        };
        cost(b).total_cmp(&cost(a))
    });
    let mut cells = vec![
        ["parse_ms", "bind_ms", "fetch_ms", "run_ms", "query"]
            .iter()
            .map(|h| (*h).to_owned())
            .collect::<Vec<_>>(),
    ];
    for row in with.into_iter().take(10) {
        if let Some(p) = row.phases {
            cells.push(vec![
                num(p.parse.total_ms),
                num(p.bind.total_ms),
                num(p.fetch.total_ms),
                num(row.total_ms),
                cut(&row.text),
            ]);
        }
    }
    align(&cells, &[true, true, true, true, false])
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
pub(crate) fn align(rows: &[Vec<String>], right: &[bool]) -> Vec<String> {
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
    let mut lines = vec![format!(
        "waits {}, acquired {}, total wait {} ms, longest {} ms, deadlocks {}",
        l.waits,
        l.acquired,
        num(l.total_wait_ms),
        num(l.max_wait_ms),
        l.deadlocks
    )];
    if !l.blockers.is_empty() {
        lines.push("who blocks whom (holder -> waiter):".to_owned());
        let mut cells = vec![
            ["times", "wait_ms", "holder", "waiter"]
                .iter()
                .map(|h| (*h).to_owned())
                .collect::<Vec<_>>(),
        ];
        for b in l.blockers.iter().take(10) {
            let name = |text: &str| {
                if text.is_empty() {
                    "(not in the log)".to_owned()
                } else {
                    cut(text)
                }
            };
            cells.push(vec![
                b.count.to_string(),
                num(b.total_wait_ms),
                name(&b.blocker_text),
                name(&b.waiter_text),
            ]);
        }
        lines.extend(
            align(&cells, &[true, true, false, false])
                .into_iter()
                .map(|line| format!("  {line}")),
        );
    }
    lines
}

/// Statements listed in the temp-file section.
const TEMP_STATEMENTS_SHOWN: usize = 10;

fn temp_files(report: &Report) -> Vec<String> {
    let t = &report.temp_files;
    let mut lines = vec![format!(
        "files {}, bytes {}, largest {}",
        t.files, t.bytes, t.max_bytes
    )];
    if let Some(rest) = &t.unattributed {
        lines.push(format!(
            "unattributed (no statement in the log): {}, {}",
            files(rest.files),
            size(rest.bytes)
        ));
    }
    let mut spilling: Vec<&StatementRow> = report
        .statements
        .iter()
        .filter(|row| !row.spilled_by.is_empty())
        .collect();
    spilling.sort_by_key(|row| std::cmp::Reverse(row.temp_bytes));
    for row in spilling.into_iter().take(TEMP_STATEMENTS_SHOWN) {
        let note = if row.cursor_unresolved {
            "  (cursor query not in the log: the DECLARE was not logged; lower log_min_duration_statement or set log_statement=all to see it)"
        } else {
            ""
        };
        lines.push(format!(
            "{} in {}  {}{note}",
            size(row.temp_bytes),
            files(row.temp_files),
            cut(&row.text)
        ));
        for who in &row.spilled_by {
            let name = if who.other {
                "(other)".to_owned()
            } else {
                let part = |value: &Option<String>| value.as_deref().unwrap_or("(none)").to_owned();
                format!(
                    "{} @ {} ({}, {})",
                    part(&who.user),
                    part(&who.application),
                    part(&who.database),
                    part(&who.host)
                )
            };
            lines.push(format!(
                "  {name}: {}, {}",
                files(who.files),
                size(who.bytes)
            ));
        }
    }
    lines
}

fn files(count: u64) -> String {
    format!("{count} {}", if count == 1 { "file" } else { "files" })
}

#[expect(clippy::cast_precision_loss, reason = "sizes stay far below 2^53")]
fn size(bytes: u64) -> String {
    let value = bytes as f64;
    if bytes < 1_024 {
        format!("{bytes} B")
    } else if bytes < 1_048_576 {
        format!("{:.1} KiB", value / 1_024.0)
    } else if bytes < 1_073_741_824 {
        format!("{:.1} MiB", value / 1_048_576.0)
    } else {
        format!("{:.1} GiB", value / 1_073_741_824.0)
    }
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
            let distance = row.distance_kb.as_ref().map_or(String::new(), |d| {
                format!(
                    "; WAL between them mean {:.0} kB, max {} kB (estimate up to {} kB)",
                    d.mean_kb, d.max_kb, d.estimate_max_kb
                )
            });
            format!(
                "{}: started {}, completed {}, buffers {}; total p50 {} ms, p95 {} ms; reasons: {}{distance}",
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

fn pooler_lines(p: &PoolerReport) -> Vec<String> {
    let mut lines = vec![format!(
        "logins {}, peak {:.0} transactions/s, clients waited {:.2} ms for a server on average",
        p.logins, p.peak_xacts_s, p.mean_wait_ms
    )];
    lines.extend(
        p.close_reasons
            .iter()
            .take(10)
            .map(|r| format!("  {:>8}x  closed: {}", r.count, r.reason)),
    );
    lines
}

fn disconnects(report: &DisconnectsReport) -> Vec<String> {
    if report.reasons.is_empty() {
        return Vec::new();
    }
    let mut cells = vec![vec![
        "count".to_owned(),
        "reason".to_owned(),
        "application".to_owned(),
    ]];
    for row in report.by_application.iter().take(10) {
        cells.push(vec![
            row.count.to_string(),
            row.reason.clone(),
            row.application.clone(),
        ]);
    }
    align(&cells, &[true, false, false])
}

fn ddl(report: &DdlReport) -> Vec<String> {
    let mut lines: Vec<String> = report
        .tags
        .iter()
        .map(|row| format!("{:>6}x  {}  {} ms", row.calls, row.tag, num(row.total_ms)))
        .collect();
    if !report.temp_tables.is_empty() {
        lines.push("temporary tables created by application:".to_owned());
        lines.extend(
            report
                .temp_tables
                .iter()
                .take(10)
                .map(|row| format!("  {:>6}x  {}", row.count, row.name)),
        );
    }
    lines
}

fn messages(rows: &[MessageRow]) -> Vec<String> {
    rows.iter()
        .take(10)
        .map(|row| format!("{:>8}x  {}", row.count, cut(&row.shape)))
        .collect()
}

fn autovacuum(rows: &[TableRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            format!(
                "{}  vacuums {}, analyzes {}, total {} ms, longest {} ms; removed {} tuples, {} pages, {} index passes",
                row.table,
                row.vacuums,
                row.analyzes,
                num(row.total_ms),
                num(row.max_ms),
                row.tuples_removed,
                row.pages_removed,
                row.index_scans
            )
        })
        .collect()
}

fn cancelled(report: &CancelledReport) -> Vec<String> {
    if report.total == 0 {
        return Vec::new();
    }
    let reasons = report
        .by_reason
        .iter()
        .map(|row| format!("{}={}", row.reason, row.count))
        .collect::<Vec<_>>()
        .join(" ");
    let mut lines = vec![format!("{} cancelled: {reasons}", report.total)];
    lines.extend(report.statements.iter().map(|row| {
        format!(
            "{}x of {} calls  {}  {}",
            row.cancelled,
            row.calls,
            &row.fingerprint[..8],
            cut(&row.text)
        )
    }));
    lines
}

fn slowest(report: &SlowestReport) -> Vec<String> {
    let mut lines = Vec::new();
    for row in &report.rows {
        let who = [&row.database, &row.user, &row.application, &row.host]
            .into_iter()
            .map(|value| value.as_deref().unwrap_or("-"))
            .collect::<Vec<_>>()
            .join("/");
        lines.push(format!(
            "{} ms  {}  {}  {}  {}",
            num(row.duration_ms),
            row.timestamp,
            &row.fingerprint[..8],
            who,
            cut(&row.text)
        ));
        if let Some(params) = &row.params {
            lines.push(format!("    parameters: {}", cut(params)));
        }
    }
    if !report.rows.is_empty() && !report.has_params {
        lines.push("(no bind parameters: kept only with --params, and only extended-protocol statements log them)".to_owned());
    }
    lines
}

fn kinds(rows: &[KindRow]) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut cells = vec![
        ["kind", "calls", "total_ms", "mean_ms", "share%"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for row in rows {
        cells.push(vec![
            row.kind.to_owned(),
            row.calls.to_string(),
            num(row.total_ms),
            opt(row.mean_ms),
            format!("{:.1}", row.share_pct),
        ]);
    }
    align(&cells, &[false, true, true, true, true])
}

/// A label for the range ending at `le_ms`, in the unit that reads best.
fn edge_label(le_ms: f64) -> String {
    if le_ms >= 3_600_000.0 {
        format!("{} h", le_ms / 3_600_000.0)
    } else if le_ms >= 60_000.0 {
        format!("{} min", le_ms / 60_000.0)
    } else if le_ms >= 1_000.0 {
        format!("{} s", le_ms / 1_000.0)
    } else {
        format!("{le_ms} ms")
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a bar of at most 30 cells"
)]
fn histogram(rows: &[HistogramBucket]) -> Vec<String> {
    let total: u64 = rows.iter().map(|row| row.count).sum();
    if total == 0 {
        return Vec::new();
    }
    let busiest = rows.iter().map(|row| row.count).max().unwrap_or(1).max(1);
    let mut previous = None;
    rows.iter()
        .map(|row| {
            let range = match (previous, row.le_ms) {
                (None, Some(hi)) => format!("<= {}", edge_label(hi)),
                (Some(lo), Some(hi)) => format!("{} - {}", edge_label(lo), edge_label(hi)),
                (Some(lo), None) => format!("> {}", edge_label(lo)),
                (None, None) => "all".to_owned(),
            };
            previous = row.le_ms.or(previous);
            let bar = "#".repeat((row.count as f64 * 30.0 / busiest as f64).ceil() as usize);
            format!(
                "{range:<16} {:>8}  {:>5.1}%  {bar}",
                row.count,
                row.count as f64 * 100.0 / total as f64
            )
            .trim_end()
            .to_owned()
        })
        .collect()
}

fn dimension(rows: &[DimensionRow]) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let temp = rows.iter().any(|row| row.temp_bytes > 0);
    let mut head = vec![
        "value",
        "statements",
        "total_ms",
        "mean_ms",
        "errors",
        "conns",
        "sessions",
        "session_ms",
    ];
    let mut right = vec![false, true, true, true, true, true, true, true];
    if temp {
        head.extend(["temp_files", "temp"]);
        right.extend([true, true]);
    }
    let mut cells = vec![head.into_iter().map(str::to_owned).collect::<Vec<_>>()];
    for row in rows {
        let mut cell = vec![
            row.value.clone(),
            row.statements.to_string(),
            num(row.total_ms),
            opt(row.mean_ms),
            row.errors.to_string(),
            row.connections.to_string(),
            row.sessions.to_string(),
            opt(row.mean_session_ms),
        ];
        if temp {
            cell.extend([row.temp_files.to_string(), size(row.temp_bytes)]);
        }
        cells.push(cell);
    }
    align(&cells, &right)
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
            filtered: 0,
            locale: None,
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
            filter: None,
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
