//! Text and JSON output for a [`Delta`].

use jiff::{SignedDuration, Timestamp};

use crate::stats::delta::{Delta, Reset, Row, SortBy, Status};

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
}
