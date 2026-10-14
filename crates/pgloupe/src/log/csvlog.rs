//! csvlog decoder: headerless CSV whose layout depends on the Postgres version.

use std::io::BufRead;

use jiff::tz::Offset;

use super::{
    DecodeError, DecodeStats, Decoder,
    record::{LogRecord, Severity, parse_num, query_id, split_connection, sqlstate, text},
    time::parse_log_time,
};

/// Reads csvlog records.
#[derive(Debug)]
pub struct CsvDecoder<R: BufRead> {
    reader: csv::Reader<R>,
    row: csv::StringRecord,
    fallback: Option<Offset>,
    stats: DecodeStats,
}

impl<R: BufRead> CsvDecoder<R> {
    /// `fallback` is the `--log-tz` offset for timezone abbreviations the log does not spell out.
    pub fn new(reader: R, fallback: Option<Offset>) -> Self {
        CsvDecoder {
            reader: csv::ReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .from_reader(reader),
            row: csv::StringRecord::new(),
            fallback,
            stats: DecodeStats::default(),
        }
    }
}

impl<R: BufRead> Decoder for CsvDecoder<R> {
    fn next_record(&mut self) -> Result<Option<LogRecord>, DecodeError> {
        loop {
            match self.reader.read_record(&mut self.row) {
                Ok(false) => return Ok(None),
                Ok(true) => {}
                Err(err) => {
                    let line = err.position().map_or(0, csv::Position::line);
                    let reason = err.to_string();
                    if let csv::ErrorKind::Io(io) = err.into_kind() {
                        return Err(DecodeError::Io(io));
                    }
                    self.stats.lines += 1;
                    self.stats.skip(line, reason);
                    continue;
                }
            }
            self.stats.lines += 1;
            let line = self.row.position().map_or(0, csv::Position::line);
            match convert(&self.row, self.fallback) {
                Ok(record) => {
                    self.stats.records += 1;
                    return Ok(Some(record));
                }
                Err(reason) => self.stats.skip(line, reason),
            }
        }
    }

    fn stats(&self) -> &DecodeStats {
        &self.stats
    }
}

fn convert(row: &csv::StringRecord, fallback: Option<Offset>) -> Result<LogRecord, String> {
    if !matches!(row.len(), 23 | 24 | 26) {
        return Err(format!("expected 23, 24 or 26 columns, got {}", row.len()));
    }
    let field = |index: usize| row.get(index).filter(|value| !value.is_empty());
    let time = |index: usize, column: &str| {
        field(index)
            // A UTF-8 BOM at the start of the file lands in the first field.
            .map(|value| value.trim_start_matches('\u{feff}'))
            .map(|value| parse_log_time(value, fallback).map_err(|err| format!("{column}: {err}")))
            .transpose()
    };
    let ts = time(0, "log_time")?.ok_or("log_time is empty")?;
    let level_label = field(11).unwrap_or("");
    let level = Severity::from_label(level_label)
        .ok_or_else(|| format!("error_severity: unknown severity {level_label:?}"))?;
    let mut record = LogRecord::new(ts, level, row.get(13).unwrap_or(""));
    record.user = text(field(1));
    record.database = text(field(2));
    record.pid = parse_num(field(3), "process_id")?;
    (record.host, record.port) = split_connection(field(4))?;
    record.session_id = text(field(5));
    record.session_line = parse_num(field(6), "session_line_num")?;
    record.command_tag = text(field(7));
    record.session_start = time(8, "session_start_time")?;
    record.vxid = text(field(9));
    record.txid = parse_num(field(10), "transaction_id")?;
    record.sqlstate = sqlstate(field(12));
    record.detail = text(field(14));
    record.hint = text(field(15));
    record.internal_query = text(field(16));
    record.context = text(field(18));
    record.statement = text(field(19));
    record.cursor_position = parse_num(field(20), "query_pos")?;
    record.application = text(field(22));
    if row.len() >= 24 {
        record.backend_type = text(field(23));
    }
    if row.len() == 26 {
        record.leader_pid = parse_num(field(24), "leader_pid")?;
        record.query_id = query_id(parse_num(field(25), "query_id")?);
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ROW: &str = r#"2026-10-14 10:20:10.300 UTC,"postgres","pagila",96,"172.17.0.1:38590",6acf5759.60,6,"SELECT",2026-10-14 10:20:09 UTC,3/9,0,ERROR,42703,"column ""titel"" does not exist",,"Perhaps you meant to reference the column ""film.title"".",,,,"SELECT titel FROM film",8,,"synth:demo","client backend",,0"#;
    const POSTMASTER_ROW: &str = r#"2026-10-14 10:20:09.302 UTC,,,42,,6acf5759.2a,3,,2026-10-14 10:20:09 UTC,,0,LOG,00000,"database system is ready to accept connections",,,,,,,,,"","postmaster",,0"#;

    fn decode(text: &str) -> (Vec<LogRecord>, DecodeStats) {
        let mut decoder = CsvDecoder::new(text.as_bytes(), None);
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
    fn decodes_a_pg14_row_with_every_field() {
        let (records, stats) = decode(SESSION_ROW);
        assert_eq!(stats.records, 1);
        assert_eq!(stats.skipped, 0);
        let r = &records[0];
        assert_eq!(r.ts, at("2026-10-14T10:20:10.300Z"));
        assert_eq!(r.level, Severity::Error);
        assert_eq!(r.message, "column \"titel\" does not exist");
        assert_eq!(r.pid, Some(96));
        assert_eq!(r.user.as_deref(), Some("postgres"));
        assert_eq!(r.database.as_deref(), Some("pagila"));
        assert_eq!(r.host.as_deref(), Some("172.17.0.1"));
        assert_eq!(r.port, Some(38590));
        assert_eq!(r.session_id.as_deref(), Some("6acf5759.60"));
        assert_eq!(r.session_line, Some(6));
        assert_eq!(r.command_tag.as_deref(), Some("SELECT"));
        assert_eq!(r.session_start, Some(at("2026-10-14T10:20:09Z")));
        assert_eq!(r.vxid.as_deref(), Some("3/9"));
        assert_eq!(r.txid, Some(0));
        assert_eq!(r.sqlstate.as_deref(), Some("42703"));
        assert_eq!(r.detail, None);
        assert_eq!(
            r.hint.as_deref(),
            Some("Perhaps you meant to reference the column \"film.title\".")
        );
        assert_eq!(r.statement.as_deref(), Some("SELECT titel FROM film"));
        assert_eq!(r.application.as_deref(), Some("synth:demo"));
        assert_eq!(r.backend_type.as_deref(), Some("client backend"));
        assert_eq!(r.query_id, None, "query id 0 means not computed");
    }

    #[test]
    fn background_process_rows_have_no_session_fields() {
        let (records, _) = decode(POSTMASTER_ROW);
        let r = &records[0];
        assert_eq!(r.user, None);
        assert_eq!(r.database, None);
        assert_eq!(r.host, None);
        assert_eq!(r.application, None, "empty application_name is None");
        assert_eq!(r.sqlstate, None, "00000 is no error");
        assert_eq!(r.backend_type.as_deref(), Some("postmaster"));
        assert_eq!(r.pid, Some(42));
    }

    #[test]
    fn older_layouts_are_picked_by_column_count() {
        // PG13: 24 columns (backend_type, no leader_pid/query_id)
        let pg13 = r#"2026-10-14 10:20:09.302 UTC,,,42,,6acf5759.2a,3,,2026-10-14 10:20:09 UTC,,0,LOG,00000,"ready",,,,,,,,,"","postmaster""#;
        let (records, stats) = decode(pg13);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        assert_eq!(records[0].backend_type.as_deref(), Some("postmaster"));
        assert_eq!(records[0].query_id, None);
        // PG12: 23 columns (no backend_type)
        let pg12 = r#"2026-10-14 10:20:09.302 UTC,,,42,,6acf5759.2a,3,,2026-10-14 10:20:09 UTC,,0,LOG,00000,"ready",,,,,,,,,"""#;
        let (records, stats) = decode(pg12);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        assert_eq!(records[0].backend_type, None);
    }

    #[test]
    fn a_wrong_column_count_is_skipped_with_a_reason() {
        let (records, stats) = decode("a,b,c\n");
        assert_eq!(records, []);
        assert_eq!(stats.skipped, 1);
        let first = stats.first_skipped.unwrap();
        assert_eq!(first.line, 1);
        assert!(
            first.reason.contains("23, 24 or 26 columns, got 3"),
            "{}",
            first.reason
        );
    }

    #[test]
    fn bad_time_severity_and_numbers_are_skipped_and_decoding_continues() {
        let bad_time = POSTMASTER_ROW.replacen("2026-10-14 10:20:09.302 UTC", "yesterday", 1);
        let bad_level = POSTMASTER_ROW.replacen(",LOG,", ",SHOUT,", 1);
        let bad_pid = POSTMASTER_ROW.replacen(",42,", ",forty-two,", 1);
        let text = format!("{bad_time}\n{bad_level}\n{bad_pid}\n{SESSION_ROW}\n");
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 1);
        assert_eq!(stats.skipped, 3);
        assert_eq!(stats.lines, 4);
        assert!(stats.first_skipped.unwrap().reason.contains("log_time"));
    }

    #[test]
    fn quoted_multiline_fields_are_one_record() {
        let row = POSTMASTER_ROW.replacen(
            "database system is ready to accept connections",
            "line one\nline two, with \"\"quotes\"\"",
            1,
        );
        let (records, stats) = decode(&row);
        assert_eq!(stats.records, 1);
        assert_eq!(records[0].message, "line one\nline two, with \"quotes\"");
    }

    #[test]
    fn crlf_and_bom_are_accepted() {
        let text = format!("\u{feff}{POSTMASTER_ROW}\r\n{SESSION_ROW}\r\n");
        let (records, stats) = decode(&text);
        assert_eq!(records.len(), 2, "{:?}", stats.first_skipped);
        assert_eq!(records[1].pid, Some(96));
    }

    #[test]
    fn unknown_timezone_abbreviation_uses_log_tz_when_given() {
        let row = POSTMASTER_ROW.replacen("10:20:09.302 UTC", "10:20:09.302 BRT", 1);
        let (records, stats) = decode(&row);
        assert_eq!(records, []);
        assert!(
            stats
                .first_skipped
                .unwrap()
                .reason
                .contains("unknown timezone abbreviation")
        );
        let minus_three = jiff::tz::Offset::from_seconds(-3 * 3600).unwrap();
        let mut decoder = CsvDecoder::new(row.as_bytes(), Some(minus_three));
        let record = decoder.next_record().unwrap().unwrap();
        assert_eq!(record.ts, at("2026-10-14T13:20:09.302Z"));
    }

    #[test]
    fn one_megabyte_statement_survives() {
        let statement = "x".repeat(1 << 20);
        let row = SESSION_ROW.replacen("SELECT titel FROM film", &statement, 1);
        let (records, stats) = decode(&row);
        assert_eq!(stats.skipped, 0, "{:?}", stats.first_skipped);
        assert_eq!(records[0].statement.as_deref().map(str::len), Some(1 << 20));
    }

    #[test]
    fn empty_input_has_no_records() {
        let (records, stats) = decode("");
        assert_eq!(records, []);
        assert_eq!(stats.lines, 0);
    }

    #[test]
    fn skipped_rows_are_shaped_by_their_reason() {
        let (_, stats) = decode("a,b,c\nd,e,f\n");
        assert_eq!(stats.top_shapes(1)[0].count, 2);
    }
}
