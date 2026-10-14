//! Typed events recognized in log records. Classification is pure: one record in, one event out.

use crate::log::{LogRecord, Severity};

/// How a statement reached the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    /// Simple protocol (`statement:`).
    Statement,
    /// Extended protocol execute.
    Execute,
    Parse,
    Bind,
}

/// What a log record means.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Statement {
        kind: StatementKind,
        duration_ms: Option<f64>,
        text: String,
        name: Option<String>,
    },
    /// `duration: X ms` with no statement text (from `log_duration`).
    Duration {
        duration_ms: f64,
    },
    LockWait {
        pid: u32,
        lock: String,
        target: String,
        waited_ms: f64,
    },
    LockAcquired {
        pid: u32,
        lock: String,
        target: String,
        waited_ms: f64,
    },
    Deadlock,
    TempFile {
        path: String,
        bytes: u64,
    },
    Error {
        level: Severity,
        sqlstate: Option<String>,
    },
    CheckpointStart {
        restart: bool,
        reason: String,
    },
    CheckpointDone {
        restart: bool,
        buffers: u64,
        write_ms: f64,
        sync_ms: f64,
        total_ms: f64,
    },
    Autovacuum {
        analyze: bool,
        table: String,
        elapsed_ms: Option<f64>,
    },
    ConnectionReceived {
        host: Option<String>,
    },
    ConnectionAuthenticated,
    ConnectionAuthorized,
    Disconnection {
        session_ms: f64,
    },
    Other,
}

impl Event {
    /// A stable `snake_case` name, used as the key in event counts.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Event::Statement { kind, .. } => match kind {
                StatementKind::Statement => "statement",
                StatementKind::Execute => "execute",
                StatementKind::Parse => "parse",
                StatementKind::Bind => "bind",
            },
            Event::Duration { .. } => "duration",
            Event::LockWait { .. } => "lock_wait",
            Event::LockAcquired { .. } => "lock_acquired",
            Event::Deadlock => "deadlock",
            Event::TempFile { .. } => "temp_file",
            Event::Error { .. } => "error",
            Event::CheckpointStart { .. } => "checkpoint_start",
            Event::CheckpointDone { .. } => "checkpoint_done",
            Event::Autovacuum { .. } => "autovacuum",
            Event::ConnectionReceived { .. } => "connection_received",
            Event::ConnectionAuthenticated => "connection_authenticated",
            Event::ConnectionAuthorized => "connection_authorized",
            Event::Disconnection { .. } => "disconnection",
            Event::Other => "other",
        }
    }
}

/// Recognizes the event a record describes (English `lc_messages`). Unknown records are
/// [`Event::Other`]; nothing is dropped.
#[must_use]
pub fn classify(record: &LogRecord) -> Event {
    match record.level {
        Severity::Log => classify_log(&record.message),
        _ => Event::Other,
    }
}

fn classify_log(message: &str) -> Event {
    statement_event(message)
        .or_else(|| connection_event(message))
        .unwrap_or(Event::Other)
}

fn statement_event(message: &str) -> Option<Event> {
    if let Some(rest) = message.strip_prefix("duration: ") {
        let (number, after) = rest.split_once(" ms")?;
        let duration_ms: f64 = number.parse().ok()?;
        if after.is_empty() {
            return Some(Event::Duration { duration_ms });
        }
        let (kind, name, text) = statement_body(after.strip_prefix("  ")?)?;
        return Some(Event::Statement {
            kind,
            duration_ms: Some(duration_ms),
            text,
            name,
        });
    }
    let (kind, name, text) = statement_body(message)?;
    Some(Event::Statement {
        kind,
        duration_ms: None,
        text,
        name,
    })
}

fn statement_body(text: &str) -> Option<(StatementKind, Option<String>, String)> {
    if let Some(sql) = text.strip_prefix("statement: ") {
        return Some((StatementKind::Statement, None, sql.to_owned()));
    }
    for (word, kind) in [
        ("execute ", StatementKind::Execute),
        ("parse ", StatementKind::Parse),
        ("bind ", StatementKind::Bind),
    ] {
        if let Some(rest) = text.strip_prefix(word)
            && let Some((name, sql)) = rest.split_once(": ")
        {
            return Some((kind, Some(name.to_owned()), sql.to_owned()));
        }
    }
    None
}

fn connection_event(message: &str) -> Option<Event> {
    if let Some(rest) = message.strip_prefix("connection received: ") {
        let host = rest
            .strip_prefix("host=")
            .and_then(|value| value.split(' ').next())
            .filter(|host| !host.is_empty() && *host != "[local]")
            .map(str::to_owned);
        return Some(Event::ConnectionReceived { host });
    }
    if message.starts_with("connection authenticated: ") {
        return Some(Event::ConnectionAuthenticated);
    }
    if message.starts_with("connection authorized: ") {
        return Some(Event::ConnectionAuthorized);
    }
    let rest = message.strip_prefix("disconnection: session time: ")?;
    let time = rest.split(' ').next()?;
    Some(Event::Disconnection {
        session_ms: session_ms(time)?,
    })
}

/// `H:MM:SS.mmm` in milliseconds.
fn session_ms(time: &str) -> Option<f64> {
    let mut parts = time.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((hours * 3600.0 + minutes * 60.0 + seconds) * 1000.0)
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;

    fn record(level: Severity, message: &str) -> LogRecord {
        LogRecord::new(Timestamp::UNIX_EPOCH, level, message)
    }

    fn log(message: &str) -> Event {
        classify(&record(Severity::Log, message))
    }

    #[test]
    fn duration_with_statement() {
        assert_eq!(
            log("duration: 12.345 ms  statement: SELECT 1"),
            Event::Statement {
                kind: StatementKind::Statement,
                duration_ms: Some(12.345),
                text: "SELECT 1".to_owned(),
                name: None
            }
        );
    }

    #[test]
    fn extended_protocol_statements_carry_the_name() {
        assert_eq!(
            log("duration: 0.5 ms  execute <unnamed>: SELECT $1"),
            Event::Statement {
                kind: StatementKind::Execute,
                duration_ms: Some(0.5),
                text: "SELECT $1".to_owned(),
                name: Some("<unnamed>".to_owned())
            }
        );
        assert!(matches!(
            log("duration: 0.1 ms  parse S_1: SELECT $1"),
            Event::Statement { kind: StatementKind::Parse, name: Some(ref n), .. } if n == "S_1"
        ));
        assert!(matches!(
            log("duration: 0.1 ms  bind S_1: SELECT $1"),
            Event::Statement {
                kind: StatementKind::Bind,
                ..
            }
        ));
    }

    #[test]
    fn statements_without_a_duration() {
        assert_eq!(
            log("statement: SELECT 1"),
            Event::Statement {
                kind: StatementKind::Statement,
                duration_ms: None,
                text: "SELECT 1".to_owned(),
                name: None
            }
        );
        assert!(matches!(
            log("execute <unnamed>: SELECT 1"),
            Event::Statement {
                kind: StatementKind::Execute,
                duration_ms: None,
                ..
            }
        ));
    }

    #[test]
    fn multi_line_statement_text_is_kept() {
        let Event::Statement { text, .. } =
            log("duration: 3.5 ms  statement: SELECT title\n    FROM film")
        else {
            panic!("not a statement");
        };
        assert_eq!(text, "SELECT title\n    FROM film");
    }

    #[test]
    fn a_bare_duration_has_no_statement() {
        assert_eq!(
            log("duration: 1.250 ms"),
            Event::Duration { duration_ms: 1.25 }
        );
    }

    #[test]
    fn near_misses_are_other() {
        assert_eq!(log("duration: soon ms  statement: SELECT 1"), Event::Other);
        assert_eq!(
            log("duration: 1 ms  plan:\nQuery Text: SELECT 1"),
            Event::Other
        );
        assert_eq!(log("parse error somewhere"), Event::Other);
        assert_eq!(log("statement:SELECT 1"), Event::Other);
    }

    #[test]
    fn connection_family() {
        assert_eq!(
            log("connection received: host=172.17.0.1 port=38590"),
            Event::ConnectionReceived {
                host: Some("172.17.0.1".to_owned())
            }
        );
        assert_eq!(
            log("connection received: host=[local]"),
            Event::ConnectionReceived { host: None }
        );
        assert_eq!(
            log(
                "connection authenticated: identity=\"postgres\" method=trust (/pgdata/pg_hba.conf:117)"
            ),
            Event::ConnectionAuthenticated
        );
        assert_eq!(
            log(
                "connection authenticated: user=\"postgres\" method=trust (/pgdata/pg_hba.conf:117)"
            ),
            Event::ConnectionAuthenticated
        );
        assert_eq!(
            log("connection authorized: user=postgres database=pagila application_name=psql"),
            Event::ConnectionAuthorized
        );
        assert_eq!(
            log(
                "connection authorized: user=u database=d SSL enabled (protocol=TLSv1.3, cipher=X, bits=256)"
            ),
            Event::ConnectionAuthorized
        );
    }

    #[test]
    fn disconnection_session_time_is_parsed() {
        assert_eq!(
            log(
                "disconnection: session time: 0:00:00.002 user=app_ro database=pagila host=172.17.0.1 port=38590"
            ),
            Event::Disconnection { session_ms: 2.0 }
        );
        assert_eq!(
            log("disconnection: session time: 1:02:03.500 user=u database=d host=h port=1"),
            Event::Disconnection {
                session_ms: 3_723_500.0
            }
        );
        assert_eq!(log("disconnection: session time: soon"), Event::Other);
    }

    #[test]
    fn only_log_level_records_are_matched_by_text() {
        assert_eq!(
            classify(&record(
                Severity::Warning,
                "duration: 1 ms  statement: SELECT 1"
            )),
            Event::Other
        );
    }

    #[test]
    fn kinds_have_stable_names() {
        assert_eq!(log("statement: SELECT 1").kind(), "statement");
        assert_eq!(log("execute x: SELECT 1").kind(), "execute");
        assert_eq!(log("duration: 1 ms").kind(), "duration");
        assert_eq!(log("whatever").kind(), "other");
    }
}
