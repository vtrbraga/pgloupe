//! The text form of the combined report.

use super::{CombinedReport, JoinedRow, Span};
use crate::analyze::render::{align, cut, num, opt, sections};

const HEADERS: [&str; 11] = [
    "link", "log", "stats", "cover", "total_ms", "mean_ms", "p95_ms", "max_ms", "share%", "errors",
    "query",
];
const RIGHT: [bool; 11] = [
    false, true, true, true, true, true, true, true, true, true, false,
];

/// Renders `report` as text: the inputs, the findings, the joined statement table, then the log
/// sections. The result ends with a newline.
#[must_use]
pub fn text(report: &CombinedReport) -> String {
    let mut lines = vec![
        "pgloupe report".to_owned(),
        format!("generator: {}", report.generator),
        format!("logs:      {}", logs_line(report)),
        format!("stats:     {}", stats_line(report)),
    ];
    if let Some(overlap) = report.windows.overlap_pct {
        lines.push(format!("overlap:   {overlap:.0}%"));
    }
    if !report.findings.is_empty() {
        lines.push(String::new());
        lines.push("findings".to_owned());
        for finding in &report.findings {
            lines.push(format!(
                "  [{}] {}: {}",
                finding.severity, finding.code, finding.message
            ));
        }
    }
    lines.push(String::new());
    lines.push(format!("statements (by {})", report.statements_by));
    let table = statements(&report.statements);
    if table.is_empty() {
        lines.push("  none".to_owned());
    } else {
        lines.extend(table.into_iter().map(|line| format!("  {line}")));
    }
    if let Some(logs) = &report.logs {
        lines.extend(sections(logs, false));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn span(span: &Span) -> String {
    format!("{} -> {}", span.from, span.to)
}

fn logs_line(report: &CombinedReport) -> String {
    let Some(input) = &report.inputs.logs else {
        return "none".to_owned();
    };
    let plural = if input.inputs == 1 { "" } else { "s" };
    let mut line = format!("{} ({} input{plural})", input.source, input.inputs);
    if let Some(store) = &input.store {
        line.push(' ');
        line.push_str(store);
    }
    if let Some(window) = &report.windows.logs {
        line.push(' ');
        line.push_str(&span(window));
    }
    line
}

fn stats_line(report: &CombinedReport) -> String {
    let Some(input) = &report.inputs.stats else {
        return "none".to_owned();
    };
    let evictions = input
        .evictions
        .map_or_else(|| "unknown".to_owned(), |n| n.to_string());
    format!(
        "{} -> {} (server {}), reset {}, evictions {evictions}",
        input.before_at,
        input.after_at,
        input.server_version_num,
        input.reset.as_str()
    )
}

fn statements(rows: &[JoinedRow]) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let dash = || "-".to_owned();
    let mut cells: Vec<Vec<String>> = vec![HEADERS.iter().map(|h| (*h).to_owned()).collect()];
    for row in rows {
        let (log, stats) = (row.log.as_ref(), row.stats.as_ref());
        cells.push(vec![
            row.link.to_owned(),
            log.map_or_else(dash, |log| log.calls.to_string()),
            stats.map_or_else(dash, |stats| stats.calls.to_string()),
            row.coverage
                .map_or_else(dash, |coverage| format!("{:.0}%", coverage * 100.0)),
            stats
                .map(|stats| num(stats.total_exec_ms))
                .or_else(|| log.map(|log| num(log.total_ms)))
                .unwrap_or_else(dash),
            opt(stats
                .and_then(|stats| stats.mean_ms)
                .or_else(|| log.and_then(|log| log.mean_ms))),
            opt(log.and_then(|log| log.p95_ms)),
            opt(log.and_then(|log| log.max_ms)),
            stats
                .map(|stats| stats.share_pct)
                .or_else(|| log.map(|log| log.share_pct))
                .map_or_else(dash, |share| format!("{share:.1}")),
            log.map_or_else(dash, |log| log.errors.to_string()),
            cut(&row.text),
        ]);
    }
    align(&cells, &RIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analyze::report::SortBy, combined::testkit::fixture_report};

    fn squash(text: &str) -> Vec<String> {
        text.lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect()
    }

    fn has(lines: &[String], expected: &str) -> bool {
        lines.iter().any(|l| l == expected)
    }

    #[test]
    fn text_report_shows_findings_windows_and_the_table() {
        let lines = squash(&text(&fixture_report(SortBy::Total, 20, true, true)));
        assert_eq!(lines[0], "pgloupe report");
        assert!(
            lines.iter().any(|l| l.starts_with("logs: files (1 input)")),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("stats: 2026-10-14T10:00:00Z -> 2026-10-14T10:10:00Z"))
        );
        assert!(has(&lines, "overlap: 100%"));
        assert!(has(&lines, "findings"));
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("[info] stats_blind_spot:"))
        );
        assert!(has(&lines, "statements (by total)"));
        assert!(
            has(
                &lines,
                "both 2 4 50% 400.00 100.00 100.49 120.00 80.0 0 UPDATE film SET rental_rate = $1"
            ),
            "{lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("stats - 2 -")
                && l.ends_with("DELETE FROM rental WHERE rental_id = $1")),
            "{lines:#?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("log 2 - -") && l.ends_with("SELECT * FROM actor"))
        );
        // The log sections follow, without the table the join replaced.
        assert!(
            has(&lines, "diagnostics")
                && has(&lines, "errors")
                && has(&lines, "timeline (bucket 1m)")
        );
        assert!(!lines.iter().any(|l| l.starts_with("top statements")));
    }

    #[test]
    fn a_statistics_only_text_has_no_log_sections() {
        let lines = squash(&text(&fixture_report(SortBy::Total, 20, false, true)));
        assert!(has(&lines, "logs: none"), "{lines:#?}");
        assert!(!has(&lines, "diagnostics") && !has(&lines, "overview"));
        assert!(has(&lines, "statements (by total)"));
    }

    #[test]
    fn a_logs_only_text_has_no_statistics_line() {
        let lines = squash(&text(&fixture_report(SortBy::Total, 20, true, false)));
        assert!(has(&lines, "stats: none"));
        assert!(!lines.iter().any(|l| l.starts_with("overlap:")));
    }

    #[test]
    fn text_matches_the_golden_file() {
        let rendered = text(&fixture_report(SortBy::Total, 20, true, true));
        // The elapsed-time-free text still names the generator version in its first lines.
        let rendered =
            rendered.replace(&format!("pgloupe {}", env!("CARGO_PKG_VERSION")), "pgloupe");
        let golden = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/combined/report.golden.txt");
        if std::env::var_os("PGLOUPE_BLESS").is_some() {
            std::fs::write(&golden, &rendered).unwrap();
        }
        assert_eq!(rendered, std::fs::read_to_string(golden).unwrap());
    }
}
