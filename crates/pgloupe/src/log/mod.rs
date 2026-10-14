//! Decoding Postgres server logs into typed records.

use std::{fmt, io::BufRead};

use jiff::tz::Offset;
use thiserror::Error;

pub mod csvlog;
pub mod jsonlog;
pub mod prefix;
pub mod record;
pub mod source;
pub mod stderr;
pub mod time;

pub use prefix::PrefixError;
pub use record::{LogRecord, Severity};

/// The three log formats pgloupe reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
    Stderr,
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

/// What a decoder read, kept and skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecodeStats {
    /// Physical lines read (csv: records read).
    pub lines: u64,
    pub records: u64,
    pub skipped: u64,
    pub first_skipped: Option<Skipped>,
}

impl DecodeStats {
    /// Counts a skipped record; only the first one keeps its line and reason.
    pub fn skip(&mut self, line: u64, reason: impl Into<String>) {
        self.skipped += 1;
        if self.first_skipped.is_none() {
            self.first_skipped = Some(Skipped {
                line,
                reason: reason.into(),
            });
        }
    }
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
    #[error("this looks like a stderr log; pass --prefix with the server's log_line_prefix")]
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
    reader: Box<dyn BufRead>,
    options: &Options,
) -> Result<Box<dyn Decoder>, SetupError> {
    Ok(match format {
        Format::Csv => Box::new(csvlog::CsvDecoder::new(reader, options.log_tz)),
        Format::Json => Box::new(jsonlog::JsonDecoder::new(reader, options.log_tz)),
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
}
