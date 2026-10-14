//! stderr decoder: groups the physical lines of one entry into one record.

use std::io::BufRead;

use jiff::{Timestamp, tz::Offset};

use super::{
    DecodeError, DecodeStats, Decoder,
    prefix::{Label, Prefix},
    record::{LogRecord, Severity},
};

/// Which field of the open entry a tab-indented continuation line extends.
#[derive(Debug, Clone, Copy)]
enum Field {
    Message,
    Detail,
    Hint,
    Context,
    Statement,
    InternalQuery,
    /// Read and discarded on purpose (`LOCATION:`).
    Ignored,
    /// A line after this entry was skipped; its continuations must not be glued on.
    Dropped,
}

#[derive(Debug)]
struct Pending {
    record: LogRecord,
    field: Field,
}

/// Reads stderr logs written with a known `log_line_prefix`.
#[derive(Debug)]
pub struct StderrDecoder<R: BufRead> {
    reader: R,
    prefix: Prefix,
    fallback: Option<Offset>,
    pending: Option<Pending>,
    buf: Vec<u8>,
    line_no: u64,
    finished: bool,
    /// A read error found while an entry was still open: reported after that entry.
    deferred: Option<DecodeError>,
    stats: DecodeStats,
}

impl<R: BufRead> StderrDecoder<R> {
    /// `fallback` is the `--log-tz` offset for timezone abbreviations the log does not spell out.
    pub fn new(reader: R, prefix: Prefix, fallback: Option<Offset>) -> Self {
        StderrDecoder {
            reader,
            prefix,
            fallback,
            pending: None,
            buf: Vec::new(),
            line_no: 0,
            finished: false,
            deferred: None,
            stats: DecodeStats::default(),
        }
    }

    /// Counts a skipped line and stops its continuation lines from extending the open entry.
    fn skip_line(&mut self, reason: impl Into<String>) {
        self.stats.skip(self.line_no, reason);
        if let Some(pending) = self.pending.as_mut() {
            pending.field = Field::Dropped;
        }
    }

    fn emit(&mut self, mut record: LogRecord) -> LogRecord {
        self.stats.records += 1;
        split_error_position(&mut record);
        record
    }

    /// The next non-blank line, or `None` at the end of input. Invalid UTF-8 is skipped.
    fn read_line(&mut self) -> Result<Option<String>, DecodeError> {
        loop {
            self.buf.clear();
            if self.reader.read_until(b'\n', &mut self.buf)? == 0 {
                return Ok(None);
            }
            self.line_no += 1;
            self.stats.lines += 1;
            while matches!(self.buf.last(), Some(b'\n' | b'\r')) {
                self.buf.pop();
            }
            match String::from_utf8(std::mem::take(&mut self.buf)) {
                Ok(line) => {
                    let line = if self.line_no == 1 {
                        line.trim_start_matches('\u{feff}').to_owned()
                    } else {
                        line
                    };
                    if line.is_empty() {
                        continue;
                    }
                    return Ok(Some(line));
                }
                Err(_) => self.skip_line("line is not valid UTF-8"),
            }
        }
    }

    fn continue_field(&mut self, extra: &str) {
        let Some(pending) = self.pending.as_mut() else {
            self.stats
                .skip(self.line_no, "continuation line without a log entry");
            return;
        };
        let target = match pending.field {
            Field::Message => Some(&mut pending.record.message),
            Field::Detail => pending.record.detail.as_mut(),
            Field::Hint => pending.record.hint.as_mut(),
            Field::Context => pending.record.context.as_mut(),
            Field::Statement => pending.record.statement.as_mut(),
            Field::InternalQuery => pending.record.internal_query.as_mut(),
            Field::Ignored => None,
            Field::Dropped => {
                self.stats
                    .skip(self.line_no, "continuation of a skipped line");
                None
            }
        };
        if let Some(target) = target {
            target.push('\n');
            target.push_str(extra);
        }
    }

    fn attach(&mut self, label: Label, text: &str, pid: Option<u32>) {
        let Some(pending) = self.pending.as_mut().filter(|p| p.record.pid == pid) else {
            self.skip_line("detail or statement line without a matching log entry");
            return;
        };
        let record = &mut pending.record;
        pending.field = match label {
            Label::Detail => {
                record.detail = Some(text.to_owned());
                Field::Detail
            }
            Label::Hint => {
                record.hint = Some(text.to_owned());
                Field::Hint
            }
            Label::Context => {
                record.context = Some(text.to_owned());
                Field::Context
            }
            Label::Statement => {
                record.statement = Some(text.to_owned());
                Field::Statement
            }
            Label::Query => {
                record.internal_query = Some(text.to_owned());
                Field::InternalQuery
            }
            Label::Location | Label::Level(_) => Field::Ignored,
        };
    }
}

/// Postgres's stderr logger appends ` at character N` to error messages; csvlog and jsonlog carry
/// the number in a separate field. Move it there so the formats agree. `LOG` and `DEBUG` messages
/// are left alone: their text can legitimately end that way.
fn split_error_position(record: &mut LogRecord) {
    if matches!(
        record.level,
        Severity::Log
            | Severity::Debug1
            | Severity::Debug2
            | Severity::Debug3
            | Severity::Debug4
            | Severity::Debug5
    ) {
        return;
    }
    let Some((message, position)) = record.message.rsplit_once(" at character ") else {
        return;
    };
    let Ok(position) = position.parse::<u32>() else {
        return;
    };
    record.cursor_position = Some(position);
    record.message.truncate(message.len());
}

impl<R: BufRead> Decoder for StderrDecoder<R> {
    fn next_record(&mut self) -> Result<Option<LogRecord>, DecodeError> {
        loop {
            if let Some(error) = self.deferred.take() {
                return Err(error);
            }
            if self.finished {
                return Ok(self.pending.take().map(|p| self.emit(p.record)));
            }
            let line = match self.read_line() {
                Ok(Some(line)) => line,
                Ok(None) => {
                    self.finished = true;
                    continue;
                }
                Err(error) => match self.pending.take() {
                    // Hand out the complete entry first; the error follows on the next call.
                    Some(pending) => {
                        self.deferred = Some(error);
                        return Ok(Some(self.emit(pending.record)));
                    }
                    None => return Err(error),
                },
            };
            if let Some(rest) = line.strip_prefix('\t') {
                self.continue_field(rest);
                continue;
            }
            let mut record = LogRecord::new(Timestamp::UNIX_EPOCH, Severity::Log, String::new());
            match self.prefix.parse_line(&line, &mut record, self.fallback) {
                Err(reason) => self.skip_line(reason),
                Ok((Label::Level(level), text)) => {
                    record.level = level;
                    text.clone_into(&mut record.message);
                    let done = self.pending.replace(Pending {
                        record,
                        field: Field::Message,
                    });
                    if let Some(done) = done {
                        return Ok(Some(self.emit(done.record)));
                    }
                }
                Ok((label, text)) => self.attach(label, text, record.pid),
            }
        }
    }

    fn stats(&self) -> &DecodeStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYNTH: &str = "%m [%p] %q%u@%d/%a %Q ";
    const SESSION: &str = "2026-10-14 10:20:12.500 UTC [103] postgres@pagila/synth:deadlock 0";

    fn decode_with(prefix: &str, text: &str) -> (Vec<LogRecord>, DecodeStats) {
        let mut decoder =
            StderrDecoder::new(text.as_bytes(), Prefix::compile(prefix).unwrap(), None);
        let mut records = Vec::new();
        while let Some(record) = decoder.next_record().unwrap() {
            records.push(record);
        }
        (records, decoder.stats().clone())
    }

    fn decode(text: &str) -> (Vec<LogRecord>, DecodeStats) {
        decode_with(SYNTH, text)
    }

    #[test]
    fn single_line_entries() {
        let text = "2026-10-14 10:20:09.302 UTC [42] LOG:  database system is ready\n\
                    2026-10-14 10:20:09.410 UTC [96] postgres@pagila/synth:demo 0 LOG:  connection authorized\n";
        let (records, stats) = decode(text);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].message, "database system is ready");
        assert_eq!(records[1].user.as_deref(), Some("postgres"));
        assert_eq!(stats.lines, 2);
        assert_eq!(stats.records, 2);
        assert_eq!(stats.skipped, 0);
    }

    #[test]
    fn detail_hint_context_and_statement_attach_to_the_entry() {
        let text = format!(
            "{SESSION} ERROR:  deadlock detected\n\
             {SESSION} DETAIL:  Process 103 waits for ShareLock.\n\
             \tProcess 102 waits for ShareLock.\n\
             {SESSION} HINT:  See server log.\n\
             {SESSION} CONTEXT:  while locking tuple\n\
             {SESSION} STATEMENT:  UPDATE film SET x = 1\n\
             2026-10-14 10:20:13.000 UTC [44] LOG:  checkpoint starting\n"
        );
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 2, "{:?}", stats.first_skipped);
        let r = &records[0];
        assert_eq!(r.level, Severity::Error);
        assert_eq!(r.message, "deadlock detected");
        assert_eq!(
            r.detail.as_deref(),
            Some("Process 103 waits for ShareLock.\nProcess 102 waits for ShareLock.")
        );
        assert_eq!(r.hint.as_deref(), Some("See server log."));
        assert_eq!(r.context.as_deref(), Some("while locking tuple"));
        assert_eq!(r.statement.as_deref(), Some("UPDATE film SET x = 1"));
        assert_eq!(records[1].message, "checkpoint starting");
        assert_eq!(stats.skipped, 0);
    }

    #[test]
    fn tab_continuation_extends_the_message_and_keeps_indentation() {
        let text = format!(
            "{SESSION} LOG:  duration: 3.500 ms  statement: SELECT title\n\t    FROM film\n\t    WHERE film_id = 1\n"
        );
        let (records, _) = decode(&text);
        assert_eq!(
            records[0].message,
            "duration: 3.500 ms  statement: SELECT title\n    FROM film\n    WHERE film_id = 1"
        );
    }

    #[test]
    fn location_lines_are_read_and_dropped() {
        let text =
            format!("{SESSION} ERROR:  boom\n{SESSION} LOCATION:  parse.c:12\n\tmore location\n");
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 1);
        assert_eq!(stats.skipped, 0);
        assert_eq!(records[0].message, "boom");
    }

    #[test]
    fn secondary_line_of_another_pid_is_skipped() {
        let other = "2026-10-14 10:20:12.500 UTC [999] postgres@pagila/x 0";
        let text = format!("{SESSION} ERROR:  boom\n{other} DETAIL:  not mine\n");
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].detail, None);
        assert_eq!(stats.skipped, 1);
        assert!(
            stats
                .first_skipped
                .unwrap()
                .reason
                .contains("matching log entry")
        );
    }

    #[test]
    fn file_starting_mid_entry_skips_orphans() {
        let text = format!(
            "\tfragment of an earlier line\n{SESSION} DETAIL:  orphan detail\n{SESSION} LOG:  first real entry\n"
        );
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].message, "first real entry");
        assert_eq!(stats.skipped, 2);
        assert_eq!(stats.first_skipped.unwrap().line, 1);
    }

    #[test]
    fn lines_that_do_not_match_are_skipped_with_the_first_reason_kept() {
        let text = format!("garbage line\n{SESSION} LOG:  fine\nmore garbage\n");
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 1);
        assert_eq!(stats.skipped, 2);
        let first = stats.first_skipped.unwrap();
        assert_eq!(first.line, 1);
        assert!(first.reason.contains("log_line_prefix"), "{}", first.reason);
    }

    #[test]
    fn the_last_entry_is_emitted_at_end_of_input() {
        let (records, stats) = decode(&format!("{SESSION} LOG:  last one"));
        assert_eq!(records.len(), 1, "no trailing newline");
        assert_eq!(stats.records, 1);
    }

    #[test]
    fn blank_lines_crlf_and_bom_are_accepted() {
        let text = format!("\u{feff}{SESSION} LOG:  one\r\n\r\n{SESSION} LOG:  two\r\n");
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 2, "{:?}", stats.first_skipped);
        assert_eq!(records[0].message, "one");
        assert_eq!(stats.skipped, 0);
    }

    #[test]
    fn invalid_utf8_is_skipped_not_fatal() {
        let mut bytes = format!("{SESSION} LOG:  one\n").into_bytes();
        bytes.extend_from_slice(b"\xFF\xFE broken\n");
        bytes.extend_from_slice(format!("{SESSION} LOG:  two\n").as_bytes());
        let mut decoder =
            StderrDecoder::new(bytes.as_slice(), Prefix::compile(SYNTH).unwrap(), None);
        let mut messages = Vec::new();
        while let Some(record) = decoder.next_record().unwrap() {
            messages.push(record.message);
        }
        assert_eq!(messages, ["one", "two"]);
        assert_eq!(decoder.stats().skipped, 1);
    }

    #[test]
    fn unknown_timezone_abbreviation_is_skipped_unless_log_tz_is_given() {
        let text = "2026-10-14 07:20:09.289 BRT [42] LOG:  hi\n";
        let (records, stats) = decode(text);
        assert_eq!(records, []);
        assert!(stats.first_skipped.unwrap().reason.contains("--log-tz"));
        let minus_three = Offset::from_seconds(-3 * 3600).unwrap();
        let mut decoder = StderrDecoder::new(
            text.as_bytes(),
            Prefix::compile(SYNTH).unwrap(),
            Some(minus_three),
        );
        let record = decoder.next_record().unwrap().unwrap();
        assert_eq!(record.ts, "2026-10-14T10:20:09.289Z".parse().unwrap());
    }

    #[test]
    fn rds_prefix_end_to_end() {
        let text = "2026-10-14 10:20:09 UTC:10.0.0.1(5432):alice@app:[123]:LOG:  connection authorized\n\
                    2026-10-14 10:20:09 UTC:10.0.0.1(5432):alice@app:[123]:ERROR:  relation \"x\" does not exist\n\
                    2026-10-14 10:20:09 UTC:10.0.0.1(5432):alice@app:[123]:STATEMENT:  SELECT * FROM x\n";
        let (records, stats) = decode_with("%t:%r:%u@%d:[%p]:", text);
        assert_eq!(records.len(), 2, "{:?}", stats.first_skipped);
        assert_eq!(records[1].statement.as_deref(), Some("SELECT * FROM x"));
        assert_eq!(records[1].host.as_deref(), Some("10.0.0.1"));
    }

    #[test]
    fn one_megabyte_statement_and_long_lines_survive() {
        let statement = "x".repeat(1 << 20);
        let text = format!("{SESSION} ERROR:  boom\n{SESSION} STATEMENT:  {statement}\n");
        let (records, stats) = decode(&text);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        assert_eq!(records[0].statement.as_deref().map(str::len), Some(1 << 20));
    }

    #[test]
    fn decoder_for_builds_each_format_and_demands_a_prefix_for_stderr() {
        use crate::log::{Format, Options, SetupError, decoder_for};
        let reader = |text: &'static str| -> Box<dyn BufRead> { Box::new(text.as_bytes()) };
        let options = Options::default();
        decoder_for(Format::Csv, reader(""), &options).unwrap();
        decoder_for(Format::Json, reader(""), &options).unwrap();
        assert!(matches!(
            decoder_for(Format::Stderr, reader(""), &options),
            Err(SetupError::MissingPrefix)
        ));
        let with_prefix = Options {
            prefix: Some(SYNTH.to_owned()),
            ..Options::default()
        };
        decoder_for(Format::Stderr, reader(""), &with_prefix).unwrap();
        let bad = Options {
            prefix: Some("%z".to_owned()),
            ..Options::default()
        };
        assert!(matches!(
            decoder_for(Format::Stderr, reader(""), &bad),
            Err(SetupError::Prefix(_))
        ));
    }

    #[test]
    fn error_position_suffix_becomes_cursor_position() {
        let text = format!(
            "{SESSION} ERROR:  syntax error at or near \"SELEC\" at character 1\n\
             {SESSION} STATEMENT:  SELEC 1\n\
             {SESSION} WARNING:  something odd at character 12\n\
             {SESSION} LOG:  duration: 1.0 ms  statement: select 'x' -- at character 7\n\
             {SESSION} ERROR:  not a position at character x\n"
        );
        let (records, _) = decode(&text);
        assert_eq!(records[0].message, "syntax error at or near \"SELEC\"");
        assert_eq!(records[0].cursor_position, Some(1));
        assert_eq!(records[1].message, "something odd");
        assert_eq!(records[1].cursor_position, Some(12));
        assert_eq!(
            records[2].message, "duration: 1.0 ms  statement: select 'x' -- at character 7",
            "LOG messages are left alone"
        );
        assert_eq!(records[2].cursor_position, None);
        assert_eq!(records[3].message, "not a position at character x");
        assert_eq!(records[3].cursor_position, None);
    }

    #[test]
    fn continuation_after_a_skipped_line_is_not_glued_onto_the_open_entry() {
        let text = format!(
            "{SESSION} LOG:  first\ngarbage line\n\tFROM secret\n\
             {SESSION} LOG:  second\n\
             2026-10-14 10:20:12.500 UTC [999] postgres@pagila/x 0 DETAIL:  not mine\n\
             \tmore of not mine\n"
        );
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].message, "first");
        assert_eq!(records[1].message, "second");
        assert_eq!(records[1].detail, None);
        assert_eq!(
            stats.skipped, 4,
            "garbage, its continuation, foreign detail, its continuation"
        );
    }

    #[test]
    fn truncated_input_keeps_the_last_complete_entry_and_reports_the_error() {
        use std::io::{BufReader, Cursor, Read};
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
            }
        }
        let data = format!("{SESSION} LOG:  one\n{SESSION} LOG:  two\n{SESSION} LOG:  three\n");
        let reader = BufReader::new(Cursor::new(data.into_bytes()).chain(Broken));
        let mut decoder = StderrDecoder::new(reader, Prefix::compile(SYNTH).unwrap(), None);
        let mut messages = Vec::new();
        let error = loop {
            match decoder.next_record() {
                Ok(Some(record)) => messages.push(record.message),
                Ok(None) => panic!("expected the read error"),
                Err(error) => break error,
            }
        };
        assert_eq!(messages, ["one", "two", "three"]);
        assert!(matches!(error, DecodeError::Io(_)));
    }
}
