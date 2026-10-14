//! Text and JSON output for a [`Delta`].

use jiff::{SignedDuration, Timestamp};
use serde::Serialize;

use crate::stats::{
    delta::{Counts, Delta, Reset, Row, SortBy, Status},
    snapshot::Counters,
};

/// Longest query text shown in the table unless `wide`.
const MAX_QUERY_CHARS: usize = 80;

const HEADERS: [&str; 8] = [
    "Δtotal_ms",
    "share%",
    "Δcalls",
    "mean_ms",
    "Δrows",
    "status",
    "queryid",
    "query",
];
/// Right-aligned columns are the numeric ones.
const RIGHT_ALIGNED: [bool; 8] = [true, true, true, true, true, false, true, false];

/// What to show: which ranking, how many rows, and how much query text.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub by: SortBy,
    pub top: usize,
    pub min_calls: u64,
    /// Full query text instead of a cut at 80 characters (text output only).
    pub wide: bool,
}

/// Renders the human-readable report. The result ends with a newline.
#[must_use]
pub fn text(delta: &Delta, view: &View) -> String {
    let mut lines = header(delta);
    lines.push(String::new());
    let rows = delta.ranked(view.by, view.top, view.min_calls);
    if rows.is_empty() {
        lines.push("no queries with activity in this window".to_owned());
    } else {
        lines.extend(table(&rows, view.wide));
    }
    let notes = notes(delta);
    if !notes.is_empty() {
        lines.push(String::new());
        lines.extend(notes);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn header(delta: &Delta) -> Vec<String> {
    let counts = &delta.counts;
    let window = delta.after_at.duration_since(delta.before_at);
    vec![
        format!(
            "pgloupe stats diff (PostgreSQL {})",
            delta.server_version_num / 10_000
        ),
        format!(
            "window:    {} -> {} ({})",
            fmt_time(delta.before_at),
            fmt_time(delta.after_at),
            fmt_duration(window)
        ),
        "units:     time in ms; mean_ms is before→after (cumulative means)".to_owned(),
        format!("reset:     {}", reset_label(delta)),
        format!("evictions: {}", evictions_label(delta.evictions)),
        format!(
            "entries:   {} in after: {} new, {} restarted, {} unchanged; {} gone; {} skipped (no queryid)",
            counts.entries,
            counts.new,
            counts.restarted,
            counts.unchanged,
            counts.gone,
            counts.skipped_no_queryid
        ),
    ]
}

fn reset_label(delta: &Delta) -> String {
    match (delta.reset, delta.reset_at) {
        (Reset::None, _) => "none".to_owned(),
        (Reset::Proven, Some(at)) => {
            format!("proven; window starts at stats_reset {}", fmt_time(at))
        }
        (Reset::Proven, None) => "proven".to_owned(),
        (Reset::Inferred, _) => {
            "inferred; every shared entry restarted, reset time unknown".to_owned()
        }
    }
}

fn evictions_label(evictions: Option<u64>) -> String {
    match evictions {
        None => "unknown (no info files)".to_owned(),
        Some(0) => "none".to_owned(),
        Some(n) => n.to_string(),
    }
}

fn notes(delta: &Delta) -> Vec<String> {
    let mut notes = Vec::new();
    if delta.reset != Reset::None {
        notes.push(
            "note: counters were reset inside the window, so every value is the activity since the reset (a lower bound), not the full window."
                .to_owned(),
        );
    }
    if delta.blind_spot {
        notes.push(
            "note: without info files or stats_since, a reset followed by more calls than before the reset looks like normal growth and cannot be detected."
                .to_owned(),
        );
    }
    if delta.evictions.is_some_and(|n| n > 0) {
        notes.push(
            "note: entries were evicted during the window; rows marked evicted-suspect may be incomplete."
                .to_owned(),
        );
    }
    notes
}

fn table(rows: &[&Row], wide: bool) -> Vec<String> {
    let body: Vec<[String; 8]> = rows.iter().map(|row| cells(row, wide)).collect();
    let head: [String; 8] = HEADERS.map(String::from);
    let mut widths: [usize; 8] = head.clone().map(|cell| cell.chars().count());
    for row in &body {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut lines = vec![align(&head, &widths)];
    lines.extend(body.iter().map(|row| align(row, &widths)));
    lines
}

fn align(cells: &[String; 8], widths: &[usize; 8]) -> String {
    let parts: Vec<String> = cells
        .iter()
        .zip(widths)
        .zip(RIGHT_ALIGNED)
        .map(|((cell, &width), right)| {
            if right {
                format!("{cell:>width$}")
            } else {
                format!("{cell:<width$}")
            }
        })
        .collect();
    parts.join("  ").trim_end().to_owned()
}

fn cells(row: &Row, wide: bool) -> [String; 8] {
    [
        format!("{:.2}", row.delta.total_exec_time),
        format!("{:.1}", row.share_pct),
        row.delta.calls.to_string(),
        format!("{}→{}", fmt_mean(row.mean_before), fmt_mean(row.mean_after)),
        row.delta.rows.to_string(),
        status_label(row),
        row.key.queryid.to_string(),
        query_cell(row.query.as_deref(), wide),
    ]
}

fn fmt_mean(mean: Option<f64>) -> String {
    mean.map_or_else(|| "-".to_owned(), |ms| format!("{ms:.3}"))
}

fn status_label(row: &Row) -> String {
    let mut parts = vec![match row.status {
        Status::Ok => "ok",
        Status::New => "new",
        Status::Restarted => "restarted",
    }];
    if row.partial {
        parts.push("partial");
    }
    if row.evicted_suspect {
        parts.push("evicted-suspect");
    }
    parts.join(",")
}

/// One line of query text: whitespace collapsed, cut with `…` unless `wide`.
fn query_cell(query: Option<&str>, wide: bool) -> String {
    let Some(query) = query else {
        return "-".to_owned();
    };
    let collapsed = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if wide || collapsed.chars().count() <= MAX_QUERY_CHARS {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(MAX_QUERY_CHARS - 1).collect();
    cut.push('…');
    cut
}

fn fmt_time(at: Timestamp) -> String {
    at.strftime("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn fmt_duration(duration: SignedDuration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{:.1}s", duration.as_secs_f64())
    } else if secs < 3_600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3_600, secs % 3_600 / 60)
    }
}

/// Version of the JSON layout below; bump when a field changes meaning.
const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
struct Report<'a> {
    schema_version: u32,
    window: Window,
    verdict: Verdict,
    counts: &'a Counts,
    rows: Vec<JsonRow<'a>>,
}

#[derive(Serialize)]
struct Window {
    before: String,
    after: String,
    duration_secs: f64,
    server_version_num: u32,
}

#[derive(Serialize)]
struct Verdict {
    reset: Reset,
    reset_at: Option<String>,
    evictions: Option<u64>,
    blind_spot: bool,
}

#[derive(Serialize)]
struct JsonRow<'a> {
    dbid: u32,
    userid: u32,
    /// A string: queryid is 64-bit and JSON consumers often read numbers as doubles.
    queryid: String,
    toplevel: bool,
    datname: Option<&'a str>,
    rolname: Option<&'a str>,
    query: Option<&'a str>,
    status: Status,
    partial: bool,
    evicted_suspect: bool,
    delta: &'a Counters,
    share_pct: f64,
    mean_before_ms: Option<f64>,
    mean_after_ms: Option<f64>,
    mean_window_ms: Option<f64>,
}

/// Renders the machine-readable report (schema version 1), pretty-printed.
///
/// # Errors
///
/// Fails only if serialization fails, which the types above do not produce in practice.
pub fn json(delta: &Delta, view: &View) -> Result<String, serde_json::Error> {
    let report = Report {
        schema_version: SCHEMA_VERSION,
        window: Window {
            before: delta.before_at.to_string(),
            after: delta.after_at.to_string(),
            duration_secs: delta.after_at.duration_since(delta.before_at).as_secs_f64(),
            server_version_num: delta.server_version_num,
        },
        verdict: Verdict {
            reset: delta.reset,
            reset_at: delta.reset_at.map(|at| at.to_string()),
            evictions: delta.evictions,
            blind_spot: delta.blind_spot,
        },
        counts: &delta.counts,
        rows: delta
            .ranked(view.by, view.top, view.min_calls)
            .into_iter()
            .map(json_row)
            .collect(),
    };
    serde_json::to_string_pretty(&report)
}

fn json_row(row: &Row) -> JsonRow<'_> {
    JsonRow {
        dbid: row.key.dbid,
        userid: row.key.userid,
        queryid: row.key.queryid.to_string(),
        toplevel: row.key.toplevel,
        datname: row.datname.as_deref(),
        rolname: row.rolname.as_deref(),
        query: row.query.as_deref(),
        status: row.status,
        partial: row.partial,
        evicted_suspect: row.evicted_suspect,
        delta: &row.delta,
        share_pct: row.share_pct,
        mean_before_ms: row.mean_before,
        mean_after_ms: row.mean_after,
        mean_window_ms: row.window_mean(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        stats::delta::diff,
        testutil::{entry, info, snap},
    };

    const T0: &str = "2026-10-14 10:00:00+00";
    const T1: &str = "2026-10-14 10:10:00+00";
    const V17: u32 = 170_011;

    fn view() -> View {
        View {
            by: SortBy::Total,
            top: 20,
            min_calls: 1,
            wide: false,
        }
    }

    /// Whitespace-collapsed lines, so tests do not depend on column widths.
    fn squash(text: &str) -> Vec<String> {
        text.lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    fn named(calls: u64, total: f64, query: &str) -> crate::stats::snapshot::Entry {
        crate::stats::snapshot::Entry {
            query: Some(query.to_owned()),
            ..entry(calls, total)
        }
    }

    fn clean_delta() -> Delta {
        let before = snap(
            T0,
            V17,
            vec![
                (1, named(100, 50.0, "SELECT * FROM film WHERE film_id = $1")),
                (2, named(10, 200.0, "UPDATE film SET rental_rate = $1")),
                (3, named(5, 5.0, "SELECT count(*) FROM actor")),
                (4, named(20, 2.0, "SELECT 1")),
            ],
        );
        let after = snap(
            T1,
            V17,
            vec![
                (1, named(160, 80.0, "SELECT * FROM film WHERE film_id = $1")),
                (2, named(14, 600.0, "UPDATE film SET rental_rate = $1")),
                (4, named(20, 2.0, "SELECT 1")),
                (5, named(2, 70.0, "DELETE FROM rental WHERE rental_id = $1")),
            ],
        );
        diff(&before, &after).unwrap()
    }

    #[test]
    fn header_states_window_units_and_verdict_before_any_number() {
        let out = text(&clean_delta(), &view());
        let lines = squash(&out);
        assert_eq!(lines[0], "pgloupe stats diff (PostgreSQL 17)");
        assert_eq!(
            lines[1],
            "window: 2026-10-14 10:00:00 UTC -> 2026-10-14 10:10:00 UTC (10m 00s)"
        );
        assert_eq!(
            lines[2],
            "units: time in ms; mean_ms is before→after (cumulative means)"
        );
        assert_eq!(lines[3], "reset: none");
        assert_eq!(lines[4], "evictions: unknown (no info files)");
        assert_eq!(
            lines[5],
            "entries: 4 in after: 1 new, 0 restarted, 1 unchanged; 1 gone; 0 skipped (no queryid)"
        );
        assert_eq!(lines[6], "");
    }

    #[test]
    fn table_rows_are_ranked_and_formatted() {
        let lines = squash(&text(&clean_delta(), &view()));
        assert_eq!(
            lines[7],
            "Δtotal_ms share% Δcalls mean_ms Δrows status queryid query"
        );
        assert_eq!(
            lines[8],
            "400.00 80.0 4 20.000→42.857 4 ok 2 UPDATE film SET rental_rate = $1"
        );
        assert_eq!(
            lines[9],
            "70.00 14.0 2 -→35.000 2 new 5 DELETE FROM rental WHERE rental_id = $1"
        );
        assert_eq!(
            lines[10],
            "30.00 6.0 60 0.500→0.500 60 ok 1 SELECT * FROM film WHERE film_id = $1"
        );
    }

    #[test]
    fn columns_line_up() {
        let out = text(&clean_delta(), &view());
        let table: Vec<&str> = out.lines().skip(7).take(4).collect();
        // Query text is whitespace-collapsed, so the last double space precedes the query cell.
        let query_start: Vec<usize> = table
            .iter()
            .map(|line| {
                let (_, cell) = line.rsplit_once("  ").unwrap();
                line.chars().count() - cell.chars().count()
            })
            .collect();
        assert!(
            query_start.windows(2).all(|pair| pair[0] == pair[1]),
            "{query_start:?}\n{out}"
        );
        assert!(table.iter().all(|line| *line == line.trim_end()));
    }

    #[test]
    fn blind_spot_note_is_printed_when_applicable() {
        let lines = squash(&text(&clean_delta(), &view()));
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("note: without info files or stats_since")),
            "{lines:#?}"
        );
    }

    #[test]
    fn proven_reset_header_and_notes() {
        let before = snap(T0, V17, vec![(1, named(100, 50.0, "SELECT 1"))])
            .with_info(info(0, "2026-10-14 09:00:00+00"));
        let after = snap(
            T1,
            V17,
            vec![
                (1, named(7, 3.5, "SELECT 1")),
                (3, named(3, 9.0, "SELECT 3")),
            ],
        )
        .with_info(info(2, "2026-10-14 10:05:00+00"));
        let delta = diff(&before, &after).unwrap();
        let lines = squash(&text(&delta, &view()));
        assert!(lines.contains(
            &"reset: proven; window starts at stats_reset 2026-10-14 10:05:00 UTC".to_owned()
        ));
        assert!(lines.contains(&"evictions: 2".to_owned()));
        assert!(
            lines
                .iter()
                .any(|l| l == "9.00 72.0 3 -→3.000 3 new,partial,evicted-suspect 3 SELECT 3")
        );
        assert!(
            lines
                .iter()
                .any(|l| l == "3.50 28.0 7 0.500→0.500 7 restarted,partial 1 SELECT 1")
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("note: counters were reset"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("note: entries were evicted"))
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.starts_with("note: without info files"))
        );
    }

    #[test]
    fn inferred_reset_is_labelled() {
        let before = snap(T0, V17, vec![(1, named(100, 50.0, "SELECT 1"))]);
        let after = snap(T1, V17, vec![(1, named(3, 1.5, "SELECT 1"))]);
        let lines = squash(&text(&diff(&before, &after).unwrap(), &view()));
        assert!(lines.contains(
            &"reset: inferred; every shared entry restarted, reset time unknown".to_owned()
        ));
    }

    #[test]
    fn query_text_is_collapsed_and_cut_unless_wide() {
        let long = format!("SELECT\n    {}", "x".repeat(200));
        let before = snap(T0, V17, vec![(1, named(1, 1.0, &long))]);
        let after = snap(T1, V17, vec![(1, named(2, 2.0, &long))]);
        let delta = diff(&before, &after).unwrap();
        let short = text(&delta, &view());
        let row = short.lines().find(|l| l.contains("SELECT x")).unwrap();
        let cell = row.split("  ").last().unwrap();
        assert_eq!(cell.chars().count(), 80);
        assert!(cell.ends_with('…'));
        let wide = text(
            &delta,
            &View {
                wide: true,
                ..view()
            },
        );
        assert!(wide.contains(&format!("SELECT {}", "x".repeat(200))));
    }

    #[test]
    fn missing_query_text_shows_a_dash() {
        let before = snap(T0, V17, vec![(1, entry(1, 1.0))]);
        let after = snap(T1, V17, vec![(1, entry(2, 2.0))]);
        let lines = squash(&text(&diff(&before, &after).unwrap(), &view()));
        assert!(lines.iter().any(|l| l.ends_with(" ok 1 -")), "{lines:#?}");
    }

    #[test]
    fn top_zero_prints_the_empty_message() {
        let out = text(&clean_delta(), &View { top: 0, ..view() });
        assert!(
            out.contains("no queries with activity in this window"),
            "{out}"
        );
    }

    #[test]
    fn durations_are_human_sized() {
        use jiff::SignedDuration;
        assert_eq!(fmt_duration(SignedDuration::from_millis(500)), "0.5s");
        assert_eq!(fmt_duration(SignedDuration::from_secs(59)), "59.0s");
        assert_eq!(fmt_duration(SignedDuration::from_secs(600)), "10m 00s");
        assert_eq!(fmt_duration(SignedDuration::from_secs(3_725)), "1h 02m");
    }

    fn json_of(delta: &Delta, view: &View) -> serde_json::Value {
        serde_json::from_str(&json(delta, view).unwrap()).unwrap()
    }

    #[test]
    fn json_has_schema_version_window_verdict_and_counts() {
        let value = json_of(&clean_delta(), &view());
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["window"]["before"], "2026-10-14T10:00:00Z");
        assert_eq!(value["window"]["after"], "2026-10-14T10:10:00Z");
        assert_eq!(value["window"]["duration_secs"], 600.0);
        assert_eq!(value["window"]["server_version_num"], 170_011);
        assert_eq!(value["verdict"]["reset"], "none");
        assert!(value["verdict"]["reset_at"].is_null());
        assert!(value["verdict"]["evictions"].is_null());
        assert_eq!(value["verdict"]["blind_spot"], true);
        assert_eq!(value["counts"]["entries"], 4);
        assert_eq!(value["counts"]["gone"], 1);
        assert_eq!(value["counts"]["unchanged"], 1);
    }

    #[test]
    fn json_rows_are_ranked_typed_and_complete() {
        let value = json_of(&clean_delta(), &view());
        let rows = value["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        let top = &rows[0];
        assert_eq!(top["queryid"], "2");
        assert_eq!(top["status"], "ok");
        assert_eq!(top["partial"], false);
        assert_eq!(top["evicted_suspect"], false);
        assert_eq!(top["delta"]["calls"], 4);
        assert_eq!(top["delta"]["total_exec_time"], 400.0);
        assert_eq!(top["share_pct"], 80.0);
        assert_eq!(top["mean_before_ms"], 20.0);
        assert_eq!(top["mean_window_ms"], 100.0);
        assert_eq!(top["query"], "UPDATE film SET rental_rate = $1");
        assert_eq!(rows[1]["status"], "new");
        assert!(rows[1]["mean_before_ms"].is_null());
    }

    #[test]
    fn json_honours_by_and_top() {
        let view = View {
            by: SortBy::Calls,
            top: 1,
            ..view()
        };
        let value = json_of(&clean_delta(), &view);
        let rows = value["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["queryid"], "1");
    }

    #[test]
    fn json_queryid_is_a_string() {
        let before = snap(T0, V17, vec![(i64::MAX, named(1, 1.0, "SELECT 1"))]);
        let after = snap(T1, V17, vec![(i64::MAX, named(2, 2.0, "SELECT 1"))]);
        let value = json_of(&diff(&before, &after).unwrap(), &view());
        assert_eq!(value["rows"][0]["queryid"], "9223372036854775807");
    }

    #[test]
    fn json_reports_proven_reset_with_its_time() {
        let before = snap(T0, V17, vec![(1, named(100, 50.0, "SELECT 1"))])
            .with_info(info(0, "2026-10-14 09:00:00+00"));
        let after = snap(T1, V17, vec![(1, named(7, 3.5, "SELECT 1"))])
            .with_info(info(2, "2026-10-14 10:05:00+00"));
        let value = json_of(&diff(&before, &after).unwrap(), &view());
        assert_eq!(value["verdict"]["reset"], "proven");
        assert_eq!(value["verdict"]["reset_at"], "2026-10-14T10:05:00Z");
        assert_eq!(value["verdict"]["evictions"], 2);
        assert_eq!(value["verdict"]["blind_spot"], false);
        assert_eq!(value["rows"][0]["partial"], true);
        assert_eq!(value["rows"][0]["status"], "restarted");
    }
}
