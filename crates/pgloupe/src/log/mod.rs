//! Decoding Postgres server logs into typed records.

use std::{collections::HashMap, fmt, io::BufRead};

use jiff::tz::Offset;
use thiserror::Error;

pub mod csvlog;
pub mod infer;
pub mod jsonlog;
pub mod locale;
pub mod pgbouncer;
pub mod prefix;
pub mod record;
pub mod source;
pub mod stderr;
pub mod tee;
pub mod time;

pub use prefix::PrefixError;
pub use record::{BlockPair, Correlation, LogRecord, Phase, Severity};

/// The log formats pgloupe reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
    Stderr,
    /// The pgBouncer log (experimental).
    Pgbouncer,
}

/// Settings shared by the decoders.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The server's `log_line_prefix`; required for stderr logs.
    pub prefix: Option<String>,
    /// Offset for timezone abbreviations the log does not spell out (`BRT`).
    pub log_tz: Option<Offset>,
}

/// A skipped record: where it was and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub line: u64,
    pub reason: String,
}

/// Distinct skipped-line shapes tracked; further ones are only counted in `other_shapes`.
const MAX_SHAPES: usize = 1_000;

/// A kind of skipped line: its normalized text, how often it occurred and where it first did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    pub text: String,
    pub count: u64,
    pub first_line: u64,
}

/// What a decoder read, kept and skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecodeStats {
    /// Physical lines read (csv: records read).
    pub lines: u64,
    pub records: u64,
    pub skipped: u64,
    pub first_skipped: Option<Skipped>,
    /// Lines whose prefix matched but whose severity label is unknown (often a localized server).
    pub no_label: u64,
    /// Entries a server cut short and said so (Amazon RDS does this to very long ones).
    pub truncated: u64,
    /// Skipped lines whose shape was not tracked because `MAX_SHAPES` was reached.
    pub other_shapes: u64,
    pub(crate) shapes: HashMap<String, Shape>,
}

impl DecodeStats {
    /// Counts a skipped record by its reason alone.
    pub fn skip(&mut self, line: u64, reason: impl Into<String>) {
        let reason = reason.into();
        self.skip_with(line, &reason, None);
    }

    /// Counts a skipped record; `text` (the offending line) gives its shape, else the reason does.
    pub fn skip_with(&mut self, line: u64, reason: &str, text: Option<&str>) {
        self.skipped += 1;
        if self.first_skipped.is_none() {
            self.first_skipped = Some(Skipped {
                line,
                reason: reason.to_owned(),
            });
        }
        let key = shape(text.unwrap_or(reason));
        if let Some(entry) = self.shapes.get_mut(&key) {
            entry.count += 1;
        } else if self.shapes.len() < MAX_SHAPES {
            self.shapes.insert(
                key.clone(),
                Shape {
                    text: key,
                    count: 1,
                    first_line: line,
                },
            );
        } else {
            self.other_shapes += 1;
        }
    }

    /// Replaces the text of every shape and of the first skipped line with a placeholder, keeping
    /// the counts and line numbers: the lines held names the report must not show.
    pub fn redact(&mut self) {
        for shape in self.shapes.values_mut() {
            "<redacted>".clone_into(&mut shape.text);
        }
        if let Some(first) = &mut self.first_skipped {
            "<redacted>".clone_into(&mut first.reason);
        }
    }

    /// The `n` most frequent skipped-line shapes (ties: earliest first line, then text).
    #[must_use]
    pub fn top_shapes(&self, n: usize) -> Vec<Shape> {
        let mut shapes: Vec<Shape> = self.shapes.values().cloned().collect();
        shapes.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then(a.first_line.cmp(&b.first_line))
                .then_with(|| a.text.cmp(&b.text))
        });
        shapes.truncate(n);
        shapes
    }
}

/// Normalizes a line so similar ones compare equal: digit runs become `#`, double-quoted text
/// becomes `"…"`, and the result is cut at 80 characters.
#[must_use]
pub fn shape(text: &str) -> String {
    let mut out = String::new();
    let mut in_quote = false;
    let mut after_digit = false;
    for c in text.chars().take(400) {
        if in_quote {
            if c == '"' {
                in_quote = false;
            }
            continue;
        }
        if c == '"' {
            out.push_str("\"…\"");
            in_quote = true;
            after_digit = false;
        } else if c.is_ascii_digit() {
            if !after_digit {
                out.push('#');
            }
            after_digit = true;
        } else {
            out.push(c);
            after_digit = false;
        }
    }
    out.chars().take(80).collect()
}

impl fmt::Display for DecodeStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "decoded {} records from {} lines",
            self.records, self.lines
        )?;
        if let Some(first) = &self.first_skipped {
            write!(
                f,
                "; {} skipped (first at line {}: {})",
                self.skipped, first.line, first.reason
            )?;
        }
        Ok(())
    }
}

/// Reading the log failed; the run cannot continue.
#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("could not read the log input")]
    Io(#[from] std::io::Error),
}

/// A decoder could not be built.
#[derive(Debug, Error)]
pub enum SetupError {
    #[error(
        "this looks like a stderr log; pass --prefix with the server's log_line_prefix (or --format to force csv or json)"
    )]
    MissingPrefix,
    #[error("invalid log_line_prefix")]
    Prefix(#[from] PrefixError),
}

/// Builds the decoder for `format`.
///
/// # Errors
///
/// Fails for a stderr log without a prefix, or with a prefix that cannot be compiled.
pub fn decoder_for(
    format: Format,
    reader: Box<dyn BufRead + Send>,
    options: &Options,
) -> Result<Box<dyn Decoder + Send>, SetupError> {
    Ok(match format {
        Format::Csv => Box::new(csvlog::CsvDecoder::new(reader, options.log_tz)),
        Format::Json => Box::new(jsonlog::JsonDecoder::new(reader, options.log_tz)),
        Format::Pgbouncer => Box::new(pgbouncer::PgbouncerDecoder::new(reader, options.log_tz)),
        Format::Stderr => {
            let text = options.prefix.as_deref().ok_or(SetupError::MissingPrefix)?;
            let prefix = prefix::Prefix::compile(text)?;
            Box::new(stderr::StderrDecoder::new(reader, prefix, options.log_tz))
        }
    })
}

/// Turns a log into records, one at a time.
pub trait Decoder {
    /// The next record, or `None` at the end of the input. Bad rows are skipped and counted in
    /// [`Decoder::stats`].
    ///
    /// # Errors
    ///
    /// Fails only when reading the input fails.
    fn next_record(&mut self) -> Result<Option<LogRecord>, DecodeError>;

    /// What has been read, kept and skipped so far.
    fn stats(&self) -> &DecodeStats;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_keep_the_first_skip_only() {
        let mut stats = DecodeStats::default();
        stats.skip(3, "first");
        stats.skip(9, "second");
        assert_eq!(stats.skipped, 2);
        assert_eq!(
            stats.first_skipped,
            Some(Skipped {
                line: 3,
                reason: "first".to_owned()
            })
        );
    }

    #[test]
    fn stats_display_is_a_one_line_summary() {
        let mut stats = DecodeStats {
            lines: 10,
            records: 8,
            ..DecodeStats::default()
        };
        assert_eq!(stats.to_string(), "decoded 8 records from 10 lines");
        stats.skip(4, "bad row");
        assert_eq!(
            stats.to_string(),
            "decoded 8 records from 10 lines; 1 skipped (first at line 4: bad row)"
        );
    }

    #[test]
    fn shapes_collapse_digits_and_quotes() {
        assert_eq!(
            shape("2026-10-14 10:20:09 [123] relation \"film_12\" missing"),
            "#-#-# #:#:# [#] relation \"…\" missing"
        );
        assert_eq!(shape(&"x".repeat(200)).chars().count(), 80);
        assert_eq!(shape(""), "");
    }

    #[test]
    fn skipped_lines_are_counted_by_shape() {
        let mut stats = DecodeStats::default();
        stats.skip_with(3, "no match", Some("garbage 1 here"));
        stats.skip_with(9, "no match", Some("garbage 22 here"));
        stats.skip_with(5, "other", Some("something else"));
        stats.skip(7, "reason only 4");
        assert_eq!(stats.skipped, 4);
        let top = stats.top_shapes(2);
        assert_eq!(
            top,
            vec![
                Shape {
                    text: "garbage # here".to_owned(),
                    count: 2,
                    first_line: 3
                },
                Shape {
                    text: "something else".to_owned(),
                    count: 1,
                    first_line: 5
                },
            ]
        );
        assert_eq!(stats.top_shapes(10).len(), 3);
        assert_eq!(stats.first_skipped.as_ref().unwrap().line, 3);
    }

    #[test]
    fn shapes_are_bounded() {
        let mut stats = DecodeStats::default();
        for i in 0..1_500_u64 {
            // Digits collapse in a shape, so build 1,500 distinct letter-only texts.
            let word: String = (0..3)
                .map(|k| char::from(b'a' + u8::try_from((i / 26_u64.pow(k)) % 26).unwrap()))
                .collect();
            stats.skip_with(i, "x", Some(&format!("word {word}")));
        }
        assert_eq!(stats.skipped, 1_500);
        assert!(stats.top_shapes(usize::MAX).len() <= 1_000);
        assert!(stats.other_shapes > 0);
    }
}
