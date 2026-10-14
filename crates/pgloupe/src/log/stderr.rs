//! stderr decoder: groups the physical lines of one entry into one record.

use std::{
    collections::{HashMap, VecDeque},
    io::BufRead,
};

use jiff::{Timestamp, tz::Offset};

use super::{
    DecodeError, DecodeStats, Decoder,
    prefix::{Label, Prefix},
    record::{LogRecord, Severity},
};

/// More open entries than this are emitted, least recently touched first.
const DEFAULT_MAX_OPEN: usize = 10_000;

/// Which field of an open entry a tab-indented continuation line extends.
#[derive(Debug, Clone, Copy)]
enum Field {
    Message,
    Detail,
    Hint,
    Context,
    Statement,
    InternalQuery,
    /// Read and discarded on purpose (`LOCATION:`, `BACKTRACE:`).
    Ignored,
    /// A line after this entry was skipped; later lines must not be glued on.
    Dropped,
}

/// The backend an entry belongs to: its pid, or `None` when the prefix has no `%p`.
type Key = Option<u32>;

#[derive(Debug)]
struct Pending {
    record: LogRecord,
    field: Field,
    touched: u64,
}

/// Reads stderr logs written with a known `log_line_prefix`.
#[derive(Debug)]
pub struct StderrDecoder<R: BufRead> {
    reader: R,
    prefix: Prefix,
    fallback: Option<Offset>,
    open: HashMap<Key, Pending>,
    ready: VecDeque<LogRecord>,
    /// The entry tab-indented lines extend; `None` after a skipped line.
    continuation: Option<Key>,
    max_open: usize,
    clock: u64,
    buf: Vec<u8>,
    line_no: u64,
    finished: bool,
    /// A read error found while entries were open: reported after they are emitted.
    deferred: Option<DecodeError>,
    stats: DecodeStats,
}

impl<R: BufRead> StderrDecoder<R> {
    /// `fallback` is the `--log-tz` offset for timezone abbreviations the log does not spell out.
    pub fn new(reader: R, prefix: Prefix, fallback: Option<Offset>) -> Self {
        Self::with_max_open(reader, prefix, fallback, DEFAULT_MAX_OPEN)
    }

    /// Like [`StderrDecoder::new`] with an explicit bound on simultaneously open entries.
    pub fn with_max_open(
        reader: R,
        prefix: Prefix,
        fallback: Option<Offset>,
        max_open: usize,
    ) -> Self {
        StderrDecoder {
            reader,
            prefix,
            fallback,
            open: HashMap::new(),
            ready: VecDeque::new(),
            continuation: None,
            max_open: max_open.max(1),
            clock: 0,
            buf: Vec::new(),
            line_no: 0,
            finished: false,
            deferred: None,
            stats: DecodeStats::default(),
        }
    }

    fn emit(&mut self, mut record: LogRecord) -> LogRecord {
        self.stats.records += 1;
        split_error_position(&mut record);
        record
    }

    fn queue(&mut self, record: LogRecord) {
        let record = self.emit(record);
        self.ready.push_back(record);
    }

    /// Counts a skipped line. With a known pid only that backend's entry is closed; otherwise the
    /// last touched entry is (the line may have been its continuation or secondary line).
    fn skip_line(&mut self, reason: &str, pid: Option<u32>, text: Option<&str>) {
        self.stats.skip_with(self.line_no, reason, text);
        if reason.starts_with("no severity label") {
            self.stats.no_label += 1;
        }
        let target = pid.map(Some).or(self.continuation);
        if let Some(entry) = target.and_then(|key| self.open.get_mut(&key)) {
            entry.field = Field::Dropped;
        }
        self.continuation = None;
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
                Err(_err) => self.skip_line("line is not valid UTF-8", None, None),
            }
        }
    }

    fn continue_field(&mut self, extra: &str, line: &str) {
        let Some(key) = self.continuation else {
            self.stats.skip_with(
                self.line_no,
                "continuation of a skipped line or a line without an entry",
                Some(line),
            );
            return;
        };
        let Some(pending) = self.open.get_mut(&key) else {
            self.stats.skip_with(
                self.line_no,
                "continuation line without a log entry",
                Some(line),
            );
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
                    .skip_with(self.line_no, "continuation of a skipped line", Some(line));
                None
            }
        };
        if let Some(target) = target {
            target.push('\n');
            target.push_str(extra);
        }
    }

    fn start_entry(&mut self, mut record: LogRecord, level: Severity, text: &str) {
        record.level = level;
        text.clone_into(&mut record.message);
        let key = record.pid;
        self.clock += 1;
        let pending = Pending {
            record,
            field: Field::Message,
            touched: self.clock,
        };
        if let Some(old) = self.open.insert(key, pending) {
            self.queue(old.record);
        } else if self.open.len() > self.max_open {
            self.evict_oldest();
        }
        self.continuation = Some(key);
    }

    fn evict_oldest(&mut self) {
        let oldest = self
            .open
            .iter()
            .min_by_key(|(_, pending)| pending.touched)
            .map(|(key, _)| *key);
        if let Some(pending) = oldest.and_then(|key| self.open.remove(&key)) {
            self.queue(pending.record);
        }
    }

    fn attach(&mut self, label: Label, text: &str, pid: Option<u32>, line: &str) {
        let Some(pending) = self
            .open
            .get_mut(&pid)
            .filter(|p| !matches!(p.field, Field::Dropped))
        else {
            self.skip_line(
                "detail or statement line without a matching log entry",
                pid,
                Some(line),
            );
            return;
        };
        self.clock += 1;
        pending.touched = self.clock;
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
        self.continuation = Some(pid);
    }

    /// Emits every open entry, oldest first.
    fn flush_open(&mut self) {
        let mut entries: Vec<Pending> = self.open.drain().map(|(_, pending)| pending).collect();
        entries.sort_by_key(|pending| (pending.record.ts, pending.record.pid));
        for pending in entries {
            self.queue(pending.record);
        }
        self.continuation = None;
    }
}

/// Postgres's stderr logger appends ` at character N` to error messages; csvlog and jsonlog carry
/// the number in a separate field. Move it there so the formats agree. `LOG` and `DEBUG` messages
/// are left alone: their text can legitimately end that way.
fn split_error_position(record: &mut LogRecord) {
    if matches!(record.level, Severity::Log | Severity::Debug) {
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
            if let Some(record) = self.ready.pop_front() {
                return Ok(Some(record));
            }
            if let Some(error) = self.deferred.take() {
                return Err(error);
            }
            if self.finished {
                return Ok(None);
            }
            let line = match self.read_line() {
                Ok(Some(line)) => line,
                Ok(None) => {
                    self.finished = true;
                    self.flush_open();
                    continue;
                }
                Err(error) => {
                    // Hand out the complete entries first; the error follows.
                    self.finished = true;
                    self.deferred = Some(error);
                    self.flush_open();
                    continue;
                }
            };
            if let Some(rest) = line.strip_prefix('\t') {
                self.continue_field(rest, &line);
                continue;
            }
            let mut record = LogRecord::new(Timestamp::UNIX_EPOCH, Severity::Log, String::new());
            match self.prefix.parse_line(&line, &mut record, self.fallback) {
                Err(reason) => self.skip_line(&reason, record.pid, Some(&line)),
                Ok((Label::Level(level), text)) => self.start_entry(record, level, text),
                Ok((label, text)) => self.attach(label, text, record.pid, &line),
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

    #[test]
    fn debug_entries_are_decoded() {
        let text = format!("{SESSION} DEBUG:  a debug message\n{SESSION} LOG:  after\n");
        let (records, stats) = decode(&text);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        assert_eq!(records[0].level, Severity::Debug);
        assert_eq!(records[0].message, "a debug message");
    }

    #[test]
    fn secondary_lines_of_a_skipped_entry_do_not_attach_to_the_previous_one() {
        let mut bytes = format!("{SESSION} LOG:  first\n{SESSION} ERROR:  bad ").into_bytes();
        bytes.extend_from_slice(b"\xE9t\xE9\n");
        bytes.extend_from_slice(format!("{SESSION} STATEMENT:  SELECT bad\n").as_bytes());
        bytes.extend_from_slice(format!("{SESSION} LOG:  last\n").as_bytes());
        let mut decoder =
            StderrDecoder::new(bytes.as_slice(), Prefix::compile(SYNTH).unwrap(), None);
        let mut records = Vec::new();
        while let Some(record) = decoder.next_record().unwrap() {
            records.push(record);
        }
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].message, "first");
        assert_eq!(
            records[0].statement, None,
            "the skipped ERROR's STATEMENT must not land here"
        );
        assert_eq!(decoder.stats().skipped, 2);
    }

    #[test]
    fn an_unknown_label_closes_the_previous_entry_to_secondary_lines() {
        let text = format!(
            "{SESSION} LOG:  first\n{SESSION} SHOUT:  what\n{SESSION} DETAIL:  of the shout\n"
        );
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].detail, None);
        assert_eq!(stats.skipped, 2);
    }

    const SIMPLE: &str = "%m [%p] ";

    fn line(pid: u32, label: &str, text: &str) -> String {
        format!("2026-10-14 10:20:12.500 UTC [{pid}] {label}:  {text}\n")
    }

    #[test]
    fn interleaved_backends_keep_their_own_lines() {
        let text = [
            line(1, "ERROR", "boom one"),
            line(2, "ERROR", "boom two"),
            line(1, "STATEMENT", "SELECT a"),
            line(2, "DETAIL", "detail two"),
            "\tmore detail two\n".to_owned(),
        ]
        .concat();
        let (records, stats) = decode_with(SIMPLE, &text);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        assert_eq!(records.len(), 2);
        let one = records.iter().find(|r| r.pid == Some(1)).unwrap();
        let two = records.iter().find(|r| r.pid == Some(2)).unwrap();
        assert_eq!(one.statement.as_deref(), Some("SELECT a"));
        assert_eq!(one.detail, None);
        assert_eq!(two.detail.as_deref(), Some("detail two\nmore detail two"));
        assert_eq!(two.statement, None);
    }

    #[test]
    fn open_entries_are_flushed_in_time_order() {
        let text = [
            "2026-10-14 10:20:12.700 UTC [2] LOG:  later\n",
            "2026-10-14 10:20:12.100 UTC [1] LOG:  earlier\n",
        ]
        .concat();
        let (records, _) = decode_with(SIMPLE, &text);
        let messages: Vec<&str> = records.iter().map(|r| r.message.as_str()).collect();
        assert_eq!(messages, ["earlier", "later"]);
    }

    #[test]
    fn a_skipped_line_of_one_pid_leaves_other_pids_entries_open() {
        let mut bytes = line(1, "ERROR", "one").into_bytes();
        bytes.extend_from_slice(line(2, "ERROR", "two").as_bytes());
        bytes.extend_from_slice(b"2026-10-14 10:20:12.500 UTC [2] SHOUT:  what\n");
        bytes.extend_from_slice(line(1, "STATEMENT", "SELECT a").as_bytes());
        bytes.extend_from_slice(line(2, "STATEMENT", "SELECT b").as_bytes());
        let mut decoder =
            StderrDecoder::new(bytes.as_slice(), Prefix::compile(SIMPLE).unwrap(), None);
        let mut records = Vec::new();
        while let Some(record) = decoder.next_record().unwrap() {
            records.push(record);
        }
        let one = records.iter().find(|r| r.pid == Some(1)).unwrap();
        let two = records.iter().find(|r| r.pid == Some(2)).unwrap();
        assert_eq!(one.statement.as_deref(), Some("SELECT a"));
        assert_eq!(
            two.statement, None,
            "pid 2's entry was closed by its skipped line"
        );
        assert_eq!(decoder.stats().skipped, 2);
    }

    #[test]
    fn the_least_recently_touched_entry_is_evicted_over_the_cap() {
        let text = [
            line(1, "LOG", "a"),
            line(2, "LOG", "b"),
            line(3, "LOG", "c"),
        ]
        .concat();
        let mut decoder = StderrDecoder::with_max_open(
            text.as_bytes(),
            Prefix::compile(SIMPLE).unwrap(),
            None,
            2,
        );
        let first = decoder.next_record().unwrap().unwrap();
        assert_eq!(first.message, "a", "pid 1 was evicted when pid 3 arrived");
        let mut rest = Vec::new();
        while let Some(record) = decoder.next_record().unwrap() {
            rest.push(record.message);
        }
        assert_eq!(rest, ["b", "c"]);
    }

    #[test]
    fn prefixes_without_a_pid_use_one_shared_entry() {
        let text = "2026-10-14 10:20:12.500 UTC ERROR:  boom\n\
                    2026-10-14 10:20:12.500 UTC STATEMENT:  SELECT 1\n";
        let (records, stats) = decode_with("%m ", text);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].statement.as_deref(), Some("SELECT 1"));
    }

    #[test]
    fn backtrace_lines_and_their_continuations_are_dropped_silently() {
        let text =
            format!("{SESSION} ERROR:  boom\n{SESSION} BACKTRACE:  frame one\n\tframe two\n");
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 1);
        assert_eq!(stats.skipped, 0);
        assert_eq!(records[0].message, "boom");
    }

    #[test]
    fn unmatched_lines_are_aggregated_by_shape() {
        let text = format!("garbage 1\n{SESSION} LOG:  fine\ngarbage 22\n");
        let (_, stats) = decode(&text);
        let top = stats.top_shapes(5);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].text, "garbage #");
        assert_eq!(top[0].count, 2);
    }

    #[test]
    fn unknown_labels_are_counted_separately() {
        let text = format!("{SESSION} FEHLER:  etwas\n{SESSION} LOG:  fine\n");
        let (_, stats) = decode(&text);
        assert_eq!(stats.no_label, 1);
    }
}
