//! The pgBouncer log: `2026-10-14 10:00:00.123 UTC [1234] LOG C-0x5593: db/user@10.0.0.1:50000 closing because: ...`.
//!
//! EXPERIMENTAL: written from pgBouncer's documentation and tested on invented lines; not yet
//! confirmed against a real pooler.

use std::io::BufRead;

use jiff::{Timestamp, tz::Offset};

use super::{DecodeError, DecodeStats, Decoder, LogRecord, Severity, time::parse_log_time};

/// Reads a pgBouncer log, one line per record.
#[derive(Debug)]
pub struct PgbouncerDecoder<R> {
    reader: R,
    fallback: Option<Offset>,
    stats: DecodeStats,
    line_no: u64,
}

impl<R: BufRead> PgbouncerDecoder<R> {
    /// A decoder over `reader`; `fallback` reads zone abbreviations other than `UTC`.
    #[must_use]
    pub fn new(reader: R, fallback: Option<Offset>) -> Self {
        PgbouncerDecoder {
            reader,
            fallback,
            stats: DecodeStats::default(),
            line_no: 0,
        }
    }
}

/// The severity pgBouncer names, as ours.
fn severity(word: &str) -> Option<Severity> {
    Some(match word {
        "LOG" => Severity::Log,
        "WARNING" => Severity::Warning,
        "ERROR" => Severity::Error,
        "FATAL" => Severity::Fatal,
        "DEBUG" | "NOISE" => Severity::Debug,
        _ => return None,
    })
}

/// Whether `line` is a pgBouncer log line: a timestamp, `[pid]`, then a bare severity word.
#[must_use]
pub fn looks_like(line: &str) -> bool {
    // Only the shape matters here: a zone name the parser does not know still parses with a
    // stand-in offset, and the real one comes from `--log-tz`.
    parse_line(line, Some(Offset::UTC)).is_some()
}

/// `db/user@host:port` -> `(db, user, host)`.
fn identity(token: &str) -> Option<(&str, &str, &str)> {
    let (db, rest) = token.split_once('/')?;
    let (user, address) = rest.split_once('@')?;
    let host = address.rsplit_once(':').map_or(address, |(h, _)| h);
    (!db.is_empty() && !user.is_empty()).then_some((db, user, host))
}

fn parse_line(line: &str, fallback: Option<Offset>) -> Option<LogRecord> {
    // `YYYY-MM-DD HH:MM:SS[.fff] ZONE [pid] LEVEL rest`
    let bracket = line.find(" [")?;
    let ts: Timestamp = parse_log_time(&line[..bracket], fallback).ok()?;
    let rest = &line[bracket + 2..];
    let (pid, after) = rest.split_once("] ")?;
    let pid: u32 = pid.parse().ok()?;
    let (word, text) = after.split_once(' ').unwrap_or((after, ""));
    let level = severity(word)?;
    let mut record = LogRecord::new(ts, level, text);
    record.pid = Some(pid);
    // Client and server lines start with `C-0x…:` or `S-0x…:` and then name the session.
    if let Some((connection, tail)) = text.split_once(": ")
        && (connection.starts_with("C-0x") || connection.starts_with("S-0x"))
    {
        let (token, message) = tail.split_once(' ').unwrap_or((tail, ""));
        let token = token.trim_end_matches(':');
        if let Some((db, user, host)) = identity(token) {
            record.database = Some(db.to_owned());
            record.user = Some(user.to_owned());
            record.host = Some(host.to_owned());
            message.clone_into(&mut record.message);
            if connection.starts_with("S-0x") {
                // The server side connection is the pooler's own; the host is the Postgres server.
                record.message = format!("server: {message}");
            }
        }
    }
    Some(record)
}

impl<R: BufRead> Decoder for PgbouncerDecoder<R> {
    fn next_record(&mut self) -> Result<Option<LogRecord>, DecodeError> {
        let mut buf = String::new();
        loop {
            buf.clear();
            match self.reader.read_line(&mut buf) {
                Ok(0) => return Ok(None),
                Ok(_) => {}
                // Invalid UTF-8 is a skipped line, not the end of the run.
                Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                    self.line_no += 1;
                    self.stats.lines += 1;
                    self.stats.skip(self.line_no, "line is not valid UTF-8");
                    continue;
                }
                Err(error) => return Err(DecodeError::Io(error)),
            }
            self.line_no += 1;
            self.stats.lines += 1;
            let line = buf.trim_end_matches(['\n', '\r']);
            if line.trim().is_empty() {
                continue;
            }
            match parse_line(line, self.fallback) {
                Some(record) => {
                    self.stats.records += 1;
                    return Ok(Some(record));
                }
                None => self
                    .stats
                    .skip_with(self.line_no, "not a pgBouncer log line", Some(line)),
            }
        }
    }

    fn stats(&self) -> &DecodeStats {
        &self.stats
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_zone_abbreviation_does_not_stop_the_format_being_recognized() {
        assert!(looks_like(
            "2026-10-14 10:00:00.123 CEST [1234] LOG C-0x5593a8: shop/app@10.0.0.1:50000 login attempt: db=shop user=app tls=no"
        ));
        assert!(!looks_like(
            "2026-10-14 10:00:00.123 CEST [1234] LOG:  hello"
        ));
    }

    use super::*;

    fn decode(text: &str) -> (Vec<LogRecord>, DecodeStats) {
        let mut decoder = PgbouncerDecoder::new(text.as_bytes(), None);
        let mut records = Vec::new();
        while let Some(r) = decoder.next_record().unwrap() {
            records.push(r);
        }
        (records, decoder.stats().clone())
    }

    #[test]
    fn client_lines_name_database_user_and_host() {
        let (records, stats) = decode(
            "2026-10-14 10:00:00.123 UTC [1234] LOG C-0x5593a8: shop/app@10.0.0.1:50000 login attempt: db=shop user=app tls=no\n\
             2026-10-14 10:00:09.000 UTC [1234] LOG C-0x5593a8: shop/app@10.0.0.1:50000 closing because: client close request (age=8s)\n",
        );
        assert_eq!(stats.skipped, 0);
        assert_eq!(records.len(), 2);
        let r = &records[0];
        assert_eq!(r.pid, Some(1234));
        assert_eq!(
            (r.database.as_deref(), r.user.as_deref(), r.host.as_deref()),
            (Some("shop"), Some("app"), Some("10.0.0.1"))
        );
        assert_eq!(r.message, "login attempt: db=shop user=app tls=no");
        assert_eq!(
            records[1].message,
            "closing because: client close request (age=8s)"
        );
    }

    #[test]
    fn stats_warnings_and_errors_keep_their_level() {
        let (records, _) = decode(
            "2026-10-14 10:01:00 UTC [1234] LOG stats: 12 xacts/s, 34 queries/s, in 1000 B/s, out 2000 B/s, xact 1200 us, query 800 us, wait 40 us\n\
             2026-10-14 10:01:05 UTC [1234] WARNING C-0x1: shop/app@10.0.0.1:50001 pooler error: no more connections allowed (max_client_conn)\n\
             2026-10-14 10:01:06 UTC [1234] ERROR S-0x2: shop/app@10.0.0.9:5432 server login failed\n",
        );
        assert_eq!(records[0].level, Severity::Log);
        assert!(records[0].message.starts_with("stats: 12 xacts/s"));
        assert_eq!(records[1].level, Severity::Warning);
        assert_eq!(
            records[1].message,
            "pooler error: no more connections allowed (max_client_conn)"
        );
        assert_eq!(records[2].level, Severity::Error);
        assert!(records[2].message.starts_with("server: "));
    }

    #[test]
    fn other_lines_are_skipped_and_counted() {
        let (records, stats) =
            decode("garbage\n2026-10-14 10:01:00 UTC [1] LOG stats: 1 xacts/s\n");
        assert_eq!(records.len(), 1);
        assert_eq!(stats.skipped, 1);
        assert!(looks_like(
            "2026-10-14 10:01:00 UTC [1] LOG stats: 1 xacts/s"
        ));
        assert!(!looks_like(
            "2026-10-14 10:01:00 UTC [1] LOG:  checkpoint starting: time"
        ));
    }
}
