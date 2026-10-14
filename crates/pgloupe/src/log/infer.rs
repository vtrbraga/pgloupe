//! Guessing the `log_line_prefix` of a stderr log from a sample of its lines.

use jiff::{Timestamp, tz::Offset};

use super::{
    prefix::{Prefix, leading_timestamp_escape},
    record::{LogRecord, Severity},
};

/// Lines examined when inferring.
pub const SAMPLE_LINES: usize = 2_000;
/// Share of the sample, in percent, a prefix must match to be accepted.
pub const ACCEPT_PERCENT: usize = 95;
/// Unmatched examples kept for the failure report.
const MAX_EXAMPLES: usize = 5;

const CANDIDATES: [&str; 8] = [
    "%t:%r:%u@%d:[%p]:",
    "%m [%p] %q%u@%d/%a %Q ",
    "%m [%p] %q%u@%d ",
    "%t [%p]: [%l-1] user=%u,db=%d,app=%a,client=%h ",
    "%m [%p] ",
    "%t [%p]: ",
    "%t ",
    "%m ",
];

const LABELS: [&str; 14] = [
    "DEBUG",
    "INFO",
    "NOTICE",
    "WARNING",
    "ERROR",
    "LOG",
    "FATAL",
    "PANIC",
    "DETAIL",
    "HINT",
    "QUERY",
    "CONTEXT",
    "STATEMENT",
    "LOCATION",
];

/// The outcome of [`infer_prefix`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inference {
    /// The accepted prefix, if one matched enough of the sample.
    pub prefix: Option<String>,
    /// Sample lines examined (continuation and blank lines excluded).
    pub sampled: usize,
    /// The best candidate and how many sample lines it matched.
    pub best: Option<(String, usize)>,
    /// Text before the first severity label of up to five lines the best candidate missed.
    pub unmatched: Vec<String>,
}

/// Picks the prefix that explains the sample best.
///
/// Candidates are a fixed list of common prefixes plus two composed from the first line's leading
/// timestamp (`<ts> [%p] ` and `<ts> `). The best one is accepted when it matches at least
/// [`ACCEPT_PERCENT`] percent of the sample.
#[must_use]
pub fn infer_prefix(lines: &[String], fallback: Option<Offset>) -> Inference {
    let sample: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|line| !line.is_empty() && !line.starts_with('\t'))
        .take(SAMPLE_LINES)
        .collect();
    let mut candidates: Vec<String> = CANDIDATES.iter().map(|c| (*c).to_owned()).collect();
    if let Some(first) = sample.first() {
        for composed in compose(first) {
            if !candidates.contains(&composed) {
                candidates.push(composed);
            }
        }
    }
    let mut best: Option<(Prefix, String, usize)> = None;
    for candidate in &candidates {
        let Ok(prefix) = Prefix::compile(candidate) else {
            continue;
        };
        let matched = sample
            .iter()
            .filter(|line| matches(&prefix, line, fallback))
            .count();
        if best.as_ref().is_none_or(|(_, _, most)| matched > *most) {
            best = Some((prefix, candidate.clone(), matched));
        }
    }
    let accepted = best
        .as_ref()
        .filter(|(_, _, matched)| {
            !sample.is_empty() && matched * 100 >= sample.len() * ACCEPT_PERCENT
        })
        .map(|(_, text, _)| text.clone());
    let unmatched = match (&accepted, &best) {
        (None, Some((prefix, _, _))) => sample
            .iter()
            .filter(|line| !matches(prefix, line, fallback))
            .take(MAX_EXAMPLES)
            .map(|line| pre_label_text(line))
            .collect(),
        _ => Vec::new(),
    };
    Inference {
        prefix: accepted,
        sampled: sample.len(),
        best: best.map(|(_, text, matched)| (text, matched)),
        unmatched,
    }
}

fn matches(prefix: &Prefix, line: &str, fallback: Option<Offset>) -> bool {
    let mut record = LogRecord::new(Timestamp::UNIX_EPOCH, Severity::Log, String::new());
    prefix.parse_line(line, &mut record, fallback).is_ok()
}

fn compose(line: &str) -> Vec<String> {
    let Some(escape) = leading_timestamp_escape(line) else {
        return Vec::new();
    };
    vec![format!("%{escape} [%p] "), format!("%{escape} ")]
}

/// The part of a line before its first severity label, at most 80 characters.
fn pre_label_text(line: &str) -> String {
    let cut = LABELS
        .iter()
        .filter_map(|label| line.find(&format!("{label}:  ")))
        .min()
        .unwrap_or(line.len());
    line[..cut].trim_end().chars().take(80).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    #[test]
    fn recognizes_the_synth_prefix() {
        let sample = lines(
            "2026-10-14 10:20:09.302 UTC [42] LOG:  ready\n\
             2026-10-14 10:20:09.410 UTC [96] postgres@pagila/app 0 LOG:  hello\n\
             2026-10-14 10:20:09.411 UTC [96] postgres@pagila/app 0 STATEMENT:  SELECT 1\n",
        );
        let result = infer_prefix(&sample, None);
        assert_eq!(result.prefix.as_deref(), Some("%m [%p] %q%u@%d/%a %Q "));
        assert_eq!(result.sampled, 3);
    }

    #[test]
    fn recognizes_the_rds_default() {
        let sample = lines(
            "2026-10-14 10:20:09 UTC:10.0.0.1(5432):alice@app:[123]:LOG:  connection authorized\n\
             2026-10-14 10:20:09 UTC::@:[44]:LOG:  checkpoint starting: time\n",
        );
        assert_eq!(
            infer_prefix(&sample, None).prefix.as_deref(),
            Some("%t:%r:%u@%d:[%p]:")
        );
    }

    #[test]
    fn recognizes_pgbadgers_recommended_prefix() {
        let sample = lines(
            "2026-10-14 10:20:09 UTC [123]: [1-1] user=alice,db=app,app=psql,client=10.0.0.1 LOG:  hi\n",
        );
        assert_eq!(
            infer_prefix(&sample, None).prefix.as_deref(),
            Some("%t [%p]: [%l-1] user=%u,db=%d,app=%a,client=%h ")
        );
    }

    #[test]
    fn composes_a_candidate_from_the_leading_timestamp() {
        let sample = lines("1760437209.289 [42] LOG:  hi\n1760437209.389 [42] LOG:  again\n");
        assert_eq!(
            infer_prefix(&sample, None).prefix.as_deref(),
            Some("%n [%p] ")
        );
    }

    #[test]
    fn continuation_and_blank_lines_are_not_sampled() {
        let sample = lines(
            "2026-10-14 10:20:09.302 UTC [42] LOG:  one\n\tcontinued\n\n2026-10-14 10:20:09.402 UTC [42] LOG:  two\n",
        );
        let result = infer_prefix(&sample, None);
        assert_eq!(result.sampled, 2);
        assert!(result.prefix.is_some());
    }

    #[test]
    fn needs_ninety_five_percent_of_the_sample() {
        let ok_lines: Vec<String> = (0..95)
            .map(|i| format!("2026-10-14 10:20:09.{i:03} UTC [42] LOG:  ok\n"))
            .collect();
        let mut text = ok_lines.concat();
        text.push_str(&"junk line\n".repeat(5));
        assert!(
            infer_prefix(&lines(&text), None).prefix.is_some(),
            "95/100 is enough"
        );
        text.push_str("more junk\n");
        assert_eq!(
            infer_prefix(&lines(&text), None).prefix,
            None,
            "95/101 is not"
        );
    }

    #[test]
    fn unknown_prefix_reports_the_closest_candidate() {
        let sample = lines(
            "<2026-10-14T10:20:09> pid=42 LOG:  hello\n<2026-10-14T10:20:10> pid=42 LOG:  again\n",
        );
        let result = infer_prefix(&sample, None);
        assert_eq!(result.prefix, None);
        assert!(result.best.is_some());
        assert_eq!(
            result.unmatched,
            [
                "<2026-10-14T10:20:09> pid=42",
                "<2026-10-14T10:20:10> pid=42"
            ]
        );
    }

    #[test]
    fn an_empty_sample_infers_nothing() {
        let result = infer_prefix(&[], None);
        assert_eq!(result.prefix, None);
        assert_eq!(result.sampled, 0);
    }

    #[test]
    fn unmatched_examples_are_limited_to_five() {
        let sample = lines(&"junk LOG:  x\n".repeat(20));
        assert_eq!(infer_prefix(&sample, None).unmatched.len(), 5);
    }
}
