//! jsonlog decoder: one JSON object per line (PG15+).

use std::io::BufRead;

use jiff::tz::Offset;
use serde::Deserialize;

use super::{
    DecodeError, DecodeStats, Decoder,
    record::{LogRecord, Severity, host, query_id, sqlstate, text},
    time::parse_log_time,
};

/// The keys of a jsonlog line that pgloupe reads; absent or `null` keys are `None`.
#[derive(Debug, Deserialize)]
struct RawLine {
    timestamp: Option<String>,
    error_severity: Option<String>,
    message: Option<String>,
    user: Option<String>,
    dbname: Option<String>,
    pid: Option<u32>,
    remote_host: Option<String>,
    remote_port: Option<u16>,
    session_id: Option<String>,
    line_num: Option<u64>,
    ps: Option<String>,
    session_start: Option<String>,
    vxid: Option<String>,
    txid: Option<u64>,
    state_code: Option<String>,
    detail: Option<String>,
    hint: Option<String>,
    internal_query: Option<String>,
    context: Option<String>,
    statement: Option<String>,
    cursor_position: Option<u32>,
    application_name: Option<String>,
    backend_type: Option<String>,
    leader_pid: Option<u32>,
    query_id: Option<i64>,
}

/// Reads jsonlog records.
#[derive(Debug)]
pub struct JsonDecoder<R: BufRead> {
    reader: R,
    fallback: Option<Offset>,
    buf: Vec<u8>,
    line_no: u64,
    stats: DecodeStats,
}

impl<R: BufRead> JsonDecoder<R> {
    /// `fallback` is the `--log-tz` offset for timezone abbreviations the log does not spell out.
    pub fn new(reader: R, fallback: Option<Offset>) -> Self {
        JsonDecoder {
            reader,
            fallback,
            buf: Vec::new(),
            line_no: 0,
            stats: DecodeStats::default(),
        }
    }
}

impl<R: BufRead> Decoder for JsonDecoder<R> {
    fn next_record(&mut self) -> Result<Option<LogRecord>, DecodeError> {
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
            let Ok(line) = std::str::from_utf8(&self.buf) else {
                self.stats.skip(self.line_no, "line is not valid UTF-8");
                continue;
            };
            let line = line.trim_start_matches('\u{feff}');
            if line.trim().is_empty() {
                continue;
            }
            match convert(line, self.fallback) {
                Ok(record) => {
                    self.stats.records += 1;
                    return Ok(Some(record));
                }
                Err(reason) => self.stats.skip(self.line_no, reason),
            }
        }
    }

    fn stats(&self) -> &DecodeStats {
        &self.stats
    }
}

fn convert(line: &str, fallback: Option<Offset>) -> Result<LogRecord, String> {
    let raw: RawLine =
        serde_json::from_str(line).map_err(|err| format!("not a jsonlog line: {err}"))?;
    let timestamp = raw.timestamp.as_deref().ok_or("missing `timestamp`")?;
    let ts = parse_log_time(timestamp, fallback).map_err(|err| format!("timestamp: {err}"))?;
    let label = raw
        .error_severity
        .as_deref()
        .ok_or("missing `error_severity`")?;
    let level = Severity::from_label(label)
        .ok_or_else(|| format!("error_severity: unknown severity {label:?}"))?;
    let message = raw.message.ok_or("missing `message`")?;
    let mut record = LogRecord::new(ts, level, message);
    record.pid = raw.pid;
    record.session_id = text(raw.session_id.as_deref());
    record.session_line = raw.line_num;
    record.session_start = raw
        .session_start
        .as_deref()
        .map(|value| parse_log_time(value, fallback).map_err(|err| format!("session_start: {err}")))
        .transpose()?;
    record.user = text(raw.user.as_deref());
    record.database = text(raw.dbname.as_deref());
    record.application = text(raw.application_name.as_deref());
    record.host = host(raw.remote_host.as_deref());
    record.port = raw.remote_port;
    record.backend_type = text(raw.backend_type.as_deref());
    record.command_tag = text(raw.ps.as_deref());
    record.vxid = text(raw.vxid.as_deref());
    record.txid = raw.txid;
    record.leader_pid = raw.leader_pid;
    record.query_id = query_id(raw.query_id);
    record.sqlstate = sqlstate(raw.state_code.as_deref());
    record.detail = text(raw.detail.as_deref());
    record.hint = text(raw.hint.as_deref());
    record.context = text(raw.context.as_deref());
    record.internal_query = text(raw.internal_query.as_deref());
    record.statement = text(raw.statement.as_deref());
    record.cursor_position = raw.cursor_position;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_LINE: &str = r#"{"timestamp":"2026-10-14 10:20:10.300 UTC","user":"postgres","dbname":"pagila","pid":96,"remote_host":"172.17.0.1","remote_port":38590,"session_id":"6acf5759.60","line_num":6,"ps":"SELECT","session_start":"2026-10-14 10:20:09 UTC","vxid":"3/9","txid":0,"error_severity":"ERROR","state_code":"42703","message":"column \"titel\" does not exist","hint":"Perhaps you meant to reference the column \"film.title\".","statement":"SELECT titel FROM film","cursor_position":8,"application_name":"synth:demo","backend_type":"client backend","query_id":0}"#;
    const POSTMASTER_LINE: &str = r#"{"timestamp":"2026-10-14 10:20:09.302 UTC","pid":42,"session_id":"6acf5759.2a","line_num":3,"session_start":"2026-10-14 10:20:09 UTC","txid":0,"error_severity":"LOG","message":"database system is ready to accept connections","backend_type":"postmaster","query_id":0}"#;

    fn decode(text: &str) -> (Vec<LogRecord>, DecodeStats) {
        let mut decoder = JsonDecoder::new(text.as_bytes(), None);
        let mut records = Vec::new();
        while let Some(record) = decoder.next_record().unwrap() {
            records.push(record);
        }
        (records, decoder.stats().clone())
    }

    fn at(text: &str) -> jiff::Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn decodes_every_field() {
        let (records, stats) = decode(SESSION_LINE);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        let r = &records[0];
        assert_eq!(r.ts, at("2026-10-14T10:20:10.300Z"));
        assert_eq!(r.level, Severity::Error);
        assert_eq!(r.message, "column \"titel\" does not exist");
        assert_eq!(r.pid, Some(96));
        assert_eq!(r.user.as_deref(), Some("postgres"));
        assert_eq!(r.database.as_deref(), Some("pagila"));
        assert_eq!(r.host.as_deref(), Some("172.17.0.1"));
        assert_eq!(r.port, Some(38590));
        assert_eq!(r.session_line, Some(6));
        assert_eq!(r.command_tag.as_deref(), Some("SELECT"));
        assert_eq!(r.session_start, Some(at("2026-10-14T10:20:09Z")));
        assert_eq!(r.sqlstate.as_deref(), Some("42703"));
        assert_eq!(r.statement.as_deref(), Some("SELECT titel FROM film"));
        assert_eq!(r.cursor_position, Some(8));
        assert_eq!(r.application.as_deref(), Some("synth:demo"));
        assert_eq!(r.query_id, None);
    }

    #[test]
    fn absent_and_null_keys_are_none() {
        let line = r#"{"timestamp":"2026-10-14 10:20:09.302 UTC","error_severity":"LOG","message":"hi","user":null,"state_code":"00000","remote_host":"[local]","query_id":-7}"#;
        let (records, stats) = decode(line);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        let r = &records[0];
        assert_eq!(r.user, None);
        assert_eq!(r.pid, None);
        assert_eq!(r.sqlstate, None);
        assert_eq!(r.host, None, "[local] carries no host");
        assert_eq!(r.query_id, Some(-7));
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let line = r#"{"timestamp":"2026-10-14 10:20:09.302 UTC","error_severity":"LOG","message":"hi","func_name":"x","file_name":"y.c","file_line_num":10}"#;
        let (records, stats) = decode(line);
        assert_eq!(records.len(), 1);
        assert_eq!(stats.skipped, 0);
    }

    #[test]
    fn bad_lines_are_skipped_and_counted() {
        let text = format!(
            "not json\n[1,2]\n{{\"message\":\"no time\",\"error_severity\":\"LOG\"}}\n{{\"timestamp\":\"2026-10-14 10:20:09.302 UTC\",\"error_severity\":\"SHOUT\",\"message\":\"x\"}}\n{POSTMASTER_LINE}\n"
        );
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 1);
        assert_eq!(stats.skipped, 4);
        assert_eq!(stats.lines, 5);
        assert_eq!(stats.first_skipped.as_ref().unwrap().line, 1);
    }

    #[test]
    fn blank_lines_are_ignored_and_crlf_and_bom_are_accepted() {
        let text = format!("\u{feff}{POSTMASTER_LINE}\r\n\r\n{SESSION_LINE}\r\n");
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 2, "{:?}", stats.first_skipped);
        assert_eq!(stats.skipped, 0);
    }

    #[test]
    fn invalid_utf8_is_skipped_not_fatal() {
        let mut bytes = b"{\"message\":\"\xFF\"}\n".to_vec();
        bytes.extend_from_slice(POSTMASTER_LINE.as_bytes());
        bytes.push(b'\n');
        let mut decoder = JsonDecoder::new(bytes.as_slice(), None);
        let record = decoder.next_record().unwrap().unwrap();
        assert_eq!(record.pid, Some(42));
        assert_eq!(decoder.stats().skipped, 1);
    }

    #[test]
    fn one_megabyte_statement_survives() {
        let statement = "x".repeat(1 << 20);
        let line = SESSION_LINE.replacen("SELECT titel FROM film", &statement, 1);
        let (records, stats) = decode(&line);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        assert_eq!(records[0].statement.as_deref().map(str::len), Some(1 << 20));
    }

    #[test]
    fn unknown_timezone_abbreviation_is_skipped_with_the_hint() {
        let line = POSTMASTER_LINE.replacen("10:20:09.302 UTC", "10:20:09.302 BRT", 1);
        let (records, stats) = decode(&line);
        assert_eq!(records, []);
        assert!(stats.first_skipped.unwrap().reason.contains("--log-tz"));
    }
}
