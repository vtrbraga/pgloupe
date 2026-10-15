//! The HTML form of the combined report: one self-contained file.

use std::fmt::Write as _;

use minijinja::{AutoEscape, Environment, Value, context, value::Serde};
use thiserror::Error;

use super::CombinedReport;
use crate::analyze::report::TimelineReport;

const TEMPLATE: &str = include_str!("templates/report.html");
const CSS: &str = concat!(
    include_str!("templates/report.css"),
    "\n",
    include_str!("templates/app.css")
);
/// The dashboard script: pure helpers, charts, then the application.
const JS: &str = concat!(
    include_str!("templates/lib.js"),
    "\n",
    include_str!("templates/charts.js"),
    "\n",
    include_str!("templates/app.js")
);
/// Bars in the timeline chart; longer timelines are merged into this many.
const MAX_BARS: usize = 240;
/// Statements listed in the server-rendered view; the dashboard and the JSON hold all of them.
const STATIC_STATEMENTS: usize = 500;

/// Why the HTML could not be produced.
#[derive(Debug, Error)]
pub enum HtmlError {
    #[error("could not render the HTML template")]
    Template(#[from] minijinja::Error),
    #[error("could not embed the report data")]
    Data(#[from] serde_json::Error),
}

/// Renders `report` as one HTML document with its styles, behaviour and data inlined.
///
/// Everything read from logs or statistics goes through the template engine's escaping; the
/// embedded JSON escapes `<`, `>` and `&` so it can never close its own `<script>` element.
///
/// # Errors
///
/// Fails if the template cannot be rendered or the data cannot be serialized.
pub fn html(report: &CombinedReport) -> Result<String, HtmlError> {
    let env = environment()?;
    let (svg, bucket) = report.logs.as_ref().map_or_else(
        || (String::new(), String::new()),
        |logs| {
            (
                timeline_svg(&logs.timeline),
                bucket_label(logs.timeline.bucket_secs),
            )
        },
    );
    let (hist_statements, hist_sessions) = report.logs.as_ref().map_or_else(
        || (Vec::new(), Vec::new()),
        |logs| {
            (
                bucket_rows(&logs.histograms.statements),
                bucket_rows(&logs.histograms.sessions),
            )
        },
    );
    let page = env.get_template("report.html")?.render(context! {
        report => Value::from(Serde(report)),
        static_limit => STATIC_STATEMENTS,
        hist_statements => Value::from(Serde(&hist_statements)),
        hist_sessions => Value::from(Serde(&hist_sessions)),
        css => Value::from_safe_string(CSS.to_owned()),
        js => Value::from_safe_string(JS.to_owned()),
        data_json => Value::from_safe_string(embedded_json(report)?),
        timeline_svg => Value::from_safe_string(svg),
        bucket_label => bucket,
    })?;
    Ok(page)
}

/// The template environment: every template is HTML, whatever it is called.
fn environment() -> Result<Environment<'static>, minijinja::Error> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.add_template("report.html", TEMPLATE)?;
    env.add_filter("ms", fmt_ms);
    env.add_filter("bytes", fmt_bytes);
    env.add_filter("n", fmt_int);
    env.add_filter("pct", fmt_pct);
    env.add_filter("coverage", fmt_coverage);
    env.add_filter("short", |text: &str| short(text));
    env.add_filter("cut", |text: &str, max: usize| cut(text, max));
    Ok(env)
}

/// A histogram bucket as the static view lists it.
#[derive(serde::Serialize)]
struct BucketRow {
    label: String,
    count: u64,
}

/// Labels the buckets `up to X`, `X to Y`, `above Z`; nothing at all for an empty histogram.
fn bucket_rows(buckets: &[crate::analyze::histogram::HistogramBucket]) -> Vec<BucketRow> {
    if buckets.iter().all(|bucket| bucket.count == 0) {
        return Vec::new();
    }
    let mut previous: Option<f64> = None;
    buckets
        .iter()
        .map(|bucket| {
            let label = match (previous, bucket.le_ms) {
                (None, Some(hi)) => format!("up to {}", fmt_ms(Some(hi))),
                (Some(lo), Some(hi)) => format!("{} to {}", fmt_ms(Some(lo)), fmt_ms(Some(hi))),
                (Some(lo), None) => format!("above {}", fmt_ms(Some(lo))),
                (None, None) => "all".to_owned(),
            };
            previous = bucket.le_ms.or(previous);
            BucketRow {
                label,
                count: bucket.count,
            }
        })
        .collect()
}

/// The report as JSON that is safe inside a `<script>` element.
fn embedded_json(report: &CombinedReport) -> Result<String, serde_json::Error> {
    let json = serde_json::to_string(report)?;
    Ok(json
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029"))
}

fn fmt_ms(value: Option<f64>) -> String {
    match value {
        None => "—".to_owned(),
        Some(ms) if ms >= 1_000.0 => format!("{:.2} s", ms / 1_000.0),
        Some(ms) if ms >= 10.0 => format!("{ms:.1} ms"),
        Some(ms) if ms >= 1.0 => format!("{ms:.2} ms"),
        Some(ms) => format!("{ms:.3} ms"),
    }
}

#[expect(clippy::cast_precision_loss, reason = "a size shown with one decimal")]
fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1_024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1_024.0 && unit < UNITS.len() - 1 {
        value /= 1_024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn fmt_int(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn fmt_pct(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |pct| format!("{pct:.1}%"))
}

fn fmt_coverage(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_owned(), |ratio| format!("{:.0}%", ratio * 100.0))
}

/// The first eight characters of a fingerprint.
fn short(text: &str) -> String {
    text.chars().take(8).collect()
}

fn cut(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let mut cut: String = collapsed.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

fn bucket_label(secs: i64) -> String {
    match secs {
        s if s % 3_600 == 0 => format!("{}h", s / 3_600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// An inline SVG bar chart of statements per bucket with errors overlaid. More than
/// [`MAX_BARS`] buckets are merged so the chart stays readable; the counts are preserved.
fn timeline_svg(timeline: &TimelineReport) -> String {
    if timeline.buckets.is_empty() {
        return "<p class=\"none\">no events</p>".to_owned();
    }
    let group = timeline.buckets.len().div_ceil(MAX_BARS);
    let bars: Vec<(&str, u64, u64)> = timeline
        .buckets
        .chunks(group)
        .map(|chunk| {
            (
                chunk[0].start.as_str(),
                chunk.iter().map(|b| b.statements).sum(),
                chunk.iter().map(|b| b.errors).sum(),
            )
        })
        .collect();
    let peak = bars.iter().map(|bar| bar.1).max().unwrap_or(1).max(1);
    let height = 100_u64;
    let scale = |n: u64| (n * 90).div_ceil(peak).min(90);
    let mut svg = format!(
        "<svg class=\"timeline\" viewBox=\"0 0 {} {height}\" preserveAspectRatio=\"none\" role=\"img\" aria-label=\"statements and errors over time\">",
        bars.len() * 4
    );
    for (i, (start, statements, errors)) in bars.iter().enumerate() {
        let x = i * 4;
        let bar = scale(*statements);
        let _ = write!(
            svg,
            "<rect class=\"bar\" x=\"{x}\" y=\"{}\" width=\"3\" height=\"{bar}\" data-statements=\"{statements}\" data-errors=\"{errors}\"><title>{}: {statements} statements, {errors} errors</title></rect>",
            height - bar,
            xml_escape(start)
        );
        if *errors > 0 {
            let overlay = scale(*errors).min(bar).max(2);
            let _ = write!(
                svg,
                "<rect class=\"err\" x=\"{x}\" y=\"{}\" width=\"3\" height=\"{overlay}\"/>",
                height - overlay
            );
        }
    }
    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        analyze::report::{SortBy, TimelineRow},
        combined::testkit::fixture_report,
    };

    const HOSTILE: &str = "</script><img src=x onerror=alert(1)>";

    #[test]
    fn the_dashboard_is_embedded_and_uses_no_dangerous_apis() {
        let page = fixture_html();
        assert!(
            page.contains("id=\"app\"")
                && page.contains("var PgLib")
                && page.contains("var PgCharts")
        );
        assert!(
            page.contains("class=\"static\""),
            "the static view stays for print and no-script use"
        );
        for forbidden in [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "new Function",
            "XMLHttpRequest",
            "fetch(",
            "WebSocket",
            "import(",
            "sendBeacon",
        ] {
            assert!(!page.contains(forbidden), "the page uses {forbidden}");
        }
    }

    #[test]
    fn the_static_view_lists_a_bounded_number_of_statements() {
        let mut report = fixture_report(SortBy::Total, 20, true, true);
        let row = report.statements[0].clone();
        report.statements = vec![row; STATIC_STATEMENTS + 50];
        let page = html(&report).unwrap();
        assert!(
            page.matches("<details>").count() <= STATIC_STATEMENTS,
            "the static table is bounded"
        );
        assert!(page.contains("lists the first 500 of 550 statements"));
    }

    #[test]
    fn a_filtered_report_says_so_in_the_header() {
        let mut report = fixture_report(SortBy::Total, 20, true, false);
        if let Some(logs) = report.logs.as_mut() {
            logs.diagnostics.filtered = 7;
        }
        let page = html(&report).unwrap();
        assert!(page.contains("filtered: 7 records dropped"), "header");
        assert!(page.contains("Filtered out 7 records"), "diagnostics");
    }

    #[test]
    fn every_template_is_escaped_whatever_its_name() {
        let env = environment().unwrap();
        let out = env
            .render_str("{{ x }}", context! { x => HOSTILE })
            .unwrap();
        assert!(!out.contains('<') && out.contains("&lt;"), "{out}");
    }

    fn fixture_html() -> String {
        html(&fixture_report(SortBy::Total, 20, true, true)).unwrap()
    }

    #[test]
    fn html_is_one_self_contained_document() {
        let page = fixture_html();
        assert!(page.starts_with("<!doctype html>"), "{}", &page[..40]);
        assert!(page.contains("<meta charset=\"utf-8\">"));
        assert!(page.contains("name=\"viewport\""));
        assert!(page.contains("<style>") && page.contains("<script"));
        for forbidden in [
            "http://",
            "https://",
            " src=",
            "href=\"http",
            "url(",
            "@import",
            "//cdn",
        ] {
            assert!(!page.contains(forbidden), "found {forbidden:?}");
        }
    }

    #[test]
    fn every_section_is_rendered_without_javascript() {
        let page = fixture_html();
        for heading in [
            "What to fix first",
            "Data caveats",
            "Statements",
            "Errors",
            "Locks",
            "Temp files",
            "Connections",
            "Checkpoints",
            "Autovacuum",
            "Timeline",
        ] {
            assert!(page.contains(&format!(">{heading}<")), "missing {heading}");
        }
        assert!(page.contains("UPDATE film SET rental_rate = $1"));
        assert!(page.contains("stats_blind_spot"));
        assert!(page.contains("42P01"));
        for link in ["both", "stats", "log"] {
            assert!(
                page.contains(&format!("class=\"link {link}\"")),
                "no {link} row"
            );
        }
    }

    #[test]
    fn hostile_text_is_inert_everywhere_it_can_appear() {
        let mut report = fixture_report(SortBy::Total, 20, true, true);
        report.statements[0].text = HOSTILE.to_owned();
        if let Some(log) = report.statements[0].log.as_mut() {
            log.text = HOSTILE.to_owned();
        }
        let logs = report.logs.as_mut().unwrap();
        logs.errors[0].first_message = HOSTILE.to_owned();
        logs.files[0].path = HOSTILE.to_owned();
        report.findings[0].message = HOSTILE.to_owned();
        let page = html(&report).unwrap();
        assert!(!page.contains(HOSTILE), "raw hostile text in the page");
        assert!(!page.contains("<img src=x"));
        // The only closing script tags are our own two (data and behaviour).
        assert_eq!(page.matches("</script>").count(), 2);
        assert!(page.contains("&lt;&#x2f;script&gt;&lt;img src=x onerror=alert(1)&gt;"));
        // The embedded data parses back to the report.
        let start = page.find("id=\"pgloupe-data\">").unwrap() + "id=\"pgloupe-data\">".len();
        let end = start + page[start..].find("</script>").unwrap();
        let embedded: serde_json::Value = serde_json::from_str(&page[start..end]).unwrap();
        assert_eq!(embedded, serde_json::to_value(&report).unwrap());
        assert_eq!(page.matches("id=\"pgloupe-data\"").count(), 1);
    }

    #[test]
    fn numbers_are_formatted_for_reading() {
        assert_eq!(fmt_ms(Some(1234.5)), "1.23 s");
        assert_eq!(fmt_ms(Some(12.3456)), "12.3 ms");
        assert_eq!(fmt_ms(Some(0.456)), "0.456 ms");
        assert_eq!(fmt_ms(None), "—");
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(1536), "1.5 KiB");
        assert_eq!(fmt_bytes(1_048_576), "1.0 MiB");
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(999), "999");
        assert_eq!(fmt_int(1_234_567), "1,234,567");
        assert_eq!(fmt_pct(Some(12.345)), "12.3%");
        assert_eq!(fmt_pct(None), "—");
        assert_eq!(short("0123456789abcdef"), "01234567");
        assert_eq!(short("q12"), "q12");
    }

    fn timeline(buckets: usize) -> TimelineReport {
        TimelineReport {
            bucket_secs: 60,
            buckets: (0..buckets)
                .map(|i| TimelineRow {
                    start: format!("2026-10-14T{:02}:{:02}:00Z", i / 60 % 24, i % 60),
                    statements: u64::try_from(i % 7 + 1).unwrap(),
                    errors: u64::from(i % 11 == 0),
                    connections: 0,
                    lock_waits: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn a_long_timeline_is_downsampled_without_losing_counts() {
        let long = timeline(1_000);
        let svg = timeline_svg(&long);
        let bars = svg.matches("class=\"bar\"").count();
        assert!(bars > 0 && bars <= 240, "{bars} bars");
        let shown: u64 = svg
            .split("data-statements=\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap().parse::<u64>().unwrap())
            .sum();
        assert_eq!(
            shown,
            long.buckets.iter().map(|b| b.statements).sum::<u64>()
        );
        assert!(timeline_svg(&timeline(0)).contains("no events"));
    }

    #[test]
    fn an_empty_report_renders() {
        let report = fixture_report(SortBy::Total, 0, true, true);
        let page = html(&report).unwrap();
        assert!(page.contains("No statements."));
        let nothing = fixture_report(SortBy::Total, 20, false, true);
        html(&nothing).unwrap();
    }
}
