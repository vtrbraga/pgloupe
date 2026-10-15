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
        /// WAL written since the previous checkpoint, in kilobytes.
        distance_kb: Option<u64>,
        /// What the server expects the next distance to be.
        estimate_kb: Option<u64>,
    },
    Autovacuum {
        analyze: bool,
        table: String,
        elapsed_ms: Option<f64>,
        pages_removed: Option<u64>,
        tuples_removed: Option<u64>,
        index_scans: Option<u64>,
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
        level if level.is_error() => classify_error(record),
        _ => Event::Other,
    }
}

fn classify_error(record: &LogRecord) -> Event {
    if record.sqlstate.as_deref() == Some("40P01") || record.message == "deadlock detected" {
        Event::Deadlock
    } else {
        Event::Error {
            level: record.level,
            sqlstate: record.sqlstate.clone(),
        }
    }
}

fn classify_log(message: &str) -> Event {
    statement_event(message)
        .or_else(|| connection_event(message))
        .or_else(|| lock_event(message))
        .or_else(|| temp_file_event(message))
        .or_else(|| checkpoint_event(message))
        .or_else(|| autovacuum_event(message))
        .unwrap_or(Event::Other)
}

/// `process N still waiting for LOCK on TARGET after X ms` and `process N acquired LOCK on
/// TARGET after X ms`.
fn lock_event(message: &str) -> Option<Event> {
    // A wait while a statement is being analysed carries its position: `… after 200 ms at
    // character 22`.
    let message = message
        .rsplit_once(" at character ")
        .filter(|(_, position)| {
            !position.is_empty() && position.bytes().all(|b| b.is_ascii_digit())
        })
        .map_or(message, |(before, _)| before);
    let rest = message.strip_prefix("process ")?;
    let (pid, rest) = rest.split_once(' ')?;
    let pid: u32 = pid.parse().ok()?;
    let (acquired, rest) = if let Some(rest) = rest.strip_prefix("still waiting for ") {
        (false, rest)
    } else {
        (true, rest.strip_prefix("acquired ")?)
    };
    let (lock, rest) = rest.split_once(" on ")?;
    let (target, waited) = rest.rsplit_once(" after ")?;
    let waited_ms: f64 = waited.strip_suffix(" ms")?.parse().ok()?;
    let (lock, target) = (lock.to_owned(), target.to_owned());
    Some(if acquired {
        Event::LockAcquired {
            pid,
            lock,
            target,
            waited_ms,
        }
    } else {
        Event::LockWait {
            pid,
            lock,
            target,
            waited_ms,
        }
    })
}

fn temp_file_event(message: &str) -> Option<Event> {
    let rest = message.strip_prefix("temporary file: path \"")?;
    let (path, size) = rest.split_once("\", size ")?;
    Some(Event::TempFile {
        path: path.to_owned(),
        bytes: size.trim().parse().ok()?,
    })
}

fn checkpoint_event(message: &str) -> Option<Event> {
    for (word, restart) in [("checkpoint", false), ("restartpoint", true)] {
        if let Some(reason) = message.strip_prefix(&format!("{word} starting: ")) {
            return Some(Event::CheckpointStart {
                restart,
                reason: reason.to_owned(),
            });
        }
        if let Some(rest) = message.strip_prefix(&format!("{word} complete: ")) {
            let after_wrote = rest.strip_prefix("wrote ")?;
            let buffers: u64 = after_wrote.split(' ').next()?.parse().ok()?;
            return Some(Event::CheckpointDone {
                restart,
                buffers,
                write_ms: seconds_ms(rest, "write=")?,
                sync_ms: seconds_ms(rest, "sync=")?,
                total_ms: seconds_ms(rest, "total=")?,
                distance_kb: number_after(rest, "distance="),
                estimate_kb: number_after(rest, "estimate="),
            });
        }
    }
    None
}

fn autovacuum_event(message: &str) -> Option<Event> {
    let rest = message.strip_prefix("automatic ")?;
    let analyze = rest.starts_with("analyze ");
    let (_, after) = rest.split_once(" of table \"")?;
    let (table, _) = after.split_once('"')?;
    Some(Event::Autovacuum {
        analyze,
        table: table.to_owned(),
        elapsed_ms: seconds_ms(message, "elapsed: "),
        pages_removed: number_after(message, "pages: "),
        tuples_removed: number_after(message, "tuples: "),
        index_scans: number_after(message, "index scans: "),
    })
}

/// The seconds value after `key` (`write=0.003 s`), in milliseconds.
/// The whole number right after `key`, if there is one.
fn number_after(text: &str, key: &str) -> Option<u64> {
    let start = text.find(key)? + key.len();
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn seconds_ms(text: &str, key: &str) -> Option<f64> {
    let start = text.find(key)? + key.len();
    let number: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    number.parse::<f64>().ok().map(|seconds| seconds * 1000.0)
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

/// Why a statement was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// `statement_timeout` expired.
    StatementTimeout,
    /// A user or application cancelled it (Ctrl-C, `pg_cancel_backend`).
    UserRequest,
    /// `lock_timeout` expired.
    LockTimeout,
    /// A statement that asked not to wait (`NOWAIT`, `SKIP LOCKED`, `lock_timeout = 0` style
    /// failures) found the lock taken: SQLSTATE `55P03` without a lock timeout.
    LockNotAvailable,
    /// Any other cancellation (recovery conflict, autovacuum, ...).
    Other,
}

impl CancelReason {
    /// The name used in reports.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CancelReason::StatementTimeout => "statement_timeout",
            CancelReason::UserRequest => "user_request",
            CancelReason::LockTimeout => "lock_timeout",
            CancelReason::LockNotAvailable => "lock_not_available",
            CancelReason::Other => "other",
        }
    }
}

/// Why `record` says a statement was cancelled, if it does: SQLSTATE `57014` (`query_canceled`) or
/// `55P03` (`lock_not_available`), or the server's wording when the log carries no SQLSTATE.
#[must_use]
pub fn cancel_reason(record: &LogRecord) -> Option<CancelReason> {
    let message = record.message.as_str();
    let cancelled = matches!(record.sqlstate.as_deref(), Some("57014" | "55P03"))
        || message.starts_with("canceling statement due to");
    if !cancelled {
        return None;
    }
    Some(if message.contains("statement timeout") {
        CancelReason::StatementTimeout
    } else if message.contains("user request") {
        CancelReason::UserRequest
    } else if message.contains("lock timeout") {
        CancelReason::LockTimeout
    } else if record.sqlstate.as_deref() == Some("55P03") {
        CancelReason::LockNotAvailable
    } else {
        CancelReason::Other
    })
}

/// The bind parameters in the DETAIL of an `execute` record (`parameters: $1 = '5', ...`).
#[must_use]
pub fn parameters(record: &LogRecord) -> Option<&str> {
    record
        .detail
        .as_deref()?
        .strip_prefix("parameters: ")
        .map(str::trim)
        .filter(|params| !params.is_empty())
}

/// Why a client or backend went away abruptly, from the log record that says so. These are the
/// signs of crashed applications, dropped networks and a server ending sessions.
#[must_use]
pub fn disconnect_reason(record: &LogRecord) -> Option<&'static str> {
    let message = record.message.as_str();
    let reasons: [(&str, &str); 11] = [
        ("could not receive data from client", "client_reset"),
        (
            "unexpected EOF on client connection with an open transaction",
            "client_gone_in_transaction",
        ),
        ("unexpected EOF on client connection", "client_gone"),
        ("could not send data to client", "client_unreachable"),
        ("connection to client lost", "client_lost"),
        (
            "terminating connection due to idle-in-transaction timeout",
            "idle_in_transaction_timeout",
        ),
        (
            "terminating connection due to idle-session timeout",
            "idle_session_timeout",
        ),
        (
            "terminating connection due to administrator command",
            "terminated_by_admin",
        ),
        (
            "terminating connection due to conflict with recovery",
            "recovery_conflict",
        ),
        (
            "terminating connection because protocol synchronization was lost",
            "protocol_error",
        ),
        (
            "canceling authentication due to timeout",
            "authentication_timeout",
        ),
    ];
    reasons
        .iter()
        .find(|(prefix, _)| message.starts_with(prefix))
        .map(|(_, reason)| *reason)
}

/// What makes two errors "the same".
///
/// The SQLSTATE and the message with its values taken out, so that `42 rows` and `43 rows` or `'a'`
/// and `'b'` group together. Constraint, table and column names stay, so foreign-key violations
/// of different constraints stay apart; for the classes where quoted text is a value (data
/// exceptions, authentication, syntax errors) it goes too.
#[must_use]
pub fn error_group(sqlstate: Option<&str>, message: &str) -> String {
    let drop_double =
        sqlstate.is_some_and(|c| c.starts_with("22") || c.starts_with("28") || c == "42601");
    let mut out = String::with_capacity(message.len());
    let mut chars = message
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                for next in chars.by_ref() {
                    if next == '\'' {
                        break;
                    }
                }
                out.push_str("'?'");
            }
            '"' if drop_double => {
                for next in chars.by_ref() {
                    if next == '"' {
                        break;
                    }
                }
                out.push_str("\"?\"");
            }
            '"' => {
                out.push('"');
                for next in chars.by_ref() {
                    out.push(next);
                    if next == '"' {
                        break;
                    }
                }
            }
            d if d.is_ascii_digit() => {
                while chars
                    .peek()
                    .is_some_and(|n| n.is_ascii_digit() || *n == '.')
                {
                    chars.next();
                }
                out.push('?');
            }
            other => out.push(other),
        }
    }
    out
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

    fn error_record(sqlstate: Option<&str>, message: &str) -> LogRecord {
        LogRecord {
            sqlstate: sqlstate.map(str::to_owned),
            ..record(Severity::Error, message)
        }
    }

    #[test]
    fn cancellations_are_recognised_with_their_reason() {
        let reason = |sqlstate, message| cancel_reason(&error_record(sqlstate, message));
        assert_eq!(
            reason(
                Some("57014"),
                "canceling statement due to statement timeout"
            ),
            Some(CancelReason::StatementTimeout)
        );
        assert_eq!(
            reason(Some("57014"), "canceling statement due to user request"),
            Some(CancelReason::UserRequest)
        );
        assert_eq!(
            reason(Some("55P03"), "canceling statement due to lock timeout"),
            Some(CancelReason::LockTimeout)
        );
        // No SQLSTATE in the log: the wording decides.
        assert_eq!(
            reason(None, "canceling statement due to user request"),
            Some(CancelReason::UserRequest)
        );
        assert_eq!(
            reason(Some("57014"), "canceling autovacuum task"),
            Some(CancelReason::Other)
        );
        assert_eq!(
            reason(None, "canceling statement due to conflict with recovery"),
            Some(CancelReason::Other)
        );
        assert_eq!(
            reason(
                Some("55P03"),
                "could not obtain lock on row in relation \"t\""
            ),
            Some(CancelReason::LockNotAvailable)
        );
        assert_eq!(reason(Some("23505"), "duplicate key"), None);
        assert_eq!(reason(Some("25P03"), "idle-in-transaction timeout"), None);
    }

    #[test]
    fn parameters_come_from_the_detail() {
        let with = |detail: &str| LogRecord {
            detail: Some(detail.to_owned()),
            ..record(
                Severity::Log,
                "duration: 1 ms  execute <unnamed>: SELECT $1",
            )
        };
        assert_eq!(
            parameters(&with("parameters: $1 = '5', $2 = 'x'")),
            Some("$1 = '5', $2 = 'x'")
        );
        assert_eq!(parameters(&with("something else")), None);
        assert_eq!(parameters(&record(Severity::Log, "x")), None);
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

    fn error(sqlstate: Option<&str>, level: Severity, message: &str) -> Event {
        let mut r = record(level, message);
        r.sqlstate = sqlstate.map(str::to_owned);
        classify(&r)
    }

    #[test]
    fn lock_waits_and_acquisitions() {
        assert_eq!(
            log("process 96 still waiting for ShareLock on transaction 1098 after 200.124 ms"),
            Event::LockWait {
                pid: 96,
                lock: "ShareLock".to_owned(),
                target: "transaction 1098".to_owned(),
                waited_ms: 200.124
            }
        );
        assert_eq!(
            log("process 96 acquired ShareLock on transaction 1098 after 411.197 ms"),
            Event::LockAcquired {
                pid: 96,
                lock: "ShareLock".to_owned(),
                target: "transaction 1098".to_owned(),
                waited_ms: 411.197
            }
        );
        assert_eq!(
            log("process 7 still waiting for AccessExclusiveLock on relation 16384 of database 16387 after 1000.5 ms")
                .kind(),
            "lock_wait"
        );
        assert_eq!(
            log("process x still waiting for ShareLock on y after 1 ms"),
            Event::Other
        );
    }

    #[test]
    fn a_wait_during_statement_analysis_carries_its_position() {
        assert_eq!(
            log(
                "process 180 still waiting for AccessShareLock on relation 16936 of database 16387 after 200.370 ms at character 22"
            ),
            Event::LockWait {
                pid: 180,
                lock: "AccessShareLock".to_owned(),
                target: "relation 16936 of database 16387".to_owned(),
                waited_ms: 200.37
            }
        );
        assert_eq!(
            log("process 180 acquired AccessShareLock on relation 16936 of database 16387 after 401.432 ms at character 22")
                .kind(),
            "lock_acquired"
        );
    }

    #[test]
    fn deadlock_by_sqlstate_or_message() {
        assert_eq!(
            error(Some("40P01"), Severity::Error, "deadlock detected"),
            Event::Deadlock
        );
        assert_eq!(
            error(None, Severity::Error, "deadlock detected"),
            Event::Deadlock
        );
    }

    #[test]
    fn deadlock_with_several_processes_is_one_event() {
        let mut r = record(Severity::Error, "deadlock detected");
        r.detail = Some(
            "Process 103 waits for ShareLock on transaction 1104; blocked by process 102.\nProcess 102 waits for ShareLock on transaction 1105; blocked by process 104.\nProcess 104 waits for ShareLock on transaction 1106; blocked by process 103."
                .to_owned(),
        );
        assert_eq!(classify(&r), Event::Deadlock);
    }

    #[test]
    fn errors_by_level_with_sqlstate() {
        assert_eq!(
            error(Some("23505"), Severity::Error, "duplicate key"),
            Event::Error {
                level: Severity::Error,
                sqlstate: Some("23505".to_owned())
            }
        );
        assert_eq!(
            error(None, Severity::Fatal, "terminating connection"),
            Event::Error {
                level: Severity::Fatal,
                sqlstate: None
            }
        );
        assert_eq!(error(None, Severity::Panic, "boom").kind(), "error");
        assert_eq!(
            error(Some("01000"), Severity::Warning, "careful"),
            Event::Other
        );
    }

    #[test]
    fn temp_files() {
        assert_eq!(
            log("temporary file: path \"base/pgsql_tmp/pgsql_tmp96.0\", size 1000000"),
            Event::TempFile {
                path: "base/pgsql_tmp/pgsql_tmp96.0".to_owned(),
                bytes: 1_000_000
            }
        );
        assert_eq!(log("temporary file: path \"x\", size big"), Event::Other);
    }

    #[test]
    fn checkpoints_and_restartpoints() {
        assert_eq!(
            log("checkpoint starting: immediate force wait"),
            Event::CheckpointStart {
                restart: false,
                reason: "immediate force wait".to_owned()
            }
        );
        assert_eq!(
            log("restartpoint starting: wal"),
            Event::CheckpointStart {
                restart: true,
                reason: "wal".to_owned()
            }
        );
        // PG15+ format
        assert_eq!(
            log(
                "checkpoint complete: wrote 3 buffers (0.0%); 0 WAL file(s) added, 0 removed, 0 recycled; write=0.003 s, sync=0.001 s, total=0.012 s; sync files=2, longest=0.001 s, average=0.001 s; distance=0 kB, estimate=0 kB; lsn=0/1A2B3C4D, redo lsn=0/1A2B3C00"
            ),
            Event::CheckpointDone {
                restart: false,
                buffers: 3,
                write_ms: 3.0,
                sync_ms: 1.0,
                total_ms: 12.0,
                distance_kb: Some(0),
                estimate_kb: Some(0),
            }
        );
        // PG13 format (no lsn)
        assert_eq!(
            log(
                "checkpoint complete: wrote 10 buffers (0.1%); 0 WAL file(s) added, 0 removed, 1 recycled; write=1.500 s, sync=0.010 s, total=1.520 s; sync files=2, longest=0.001 s, average=0.001 s; distance=0 kB, estimate=0 kB"
            ),
            Event::CheckpointDone {
                restart: false,
                buffers: 10,
                write_ms: 1_500.0,
                sync_ms: 10.0,
                total_ms: 1_520.0,
                distance_kb: Some(0),
                estimate_kb: Some(0),
            }
        );
        assert_eq!(
            log("restartpoint complete: wrote 5 buffers (0.0%); 0 WAL file(s) added, 0 removed, 0 recycled; write=0.500 s, sync=0.000 s, total=0.600 s; sync files=0, longest=0.000 s, average=0.000 s; distance=0 kB, estimate=0 kB").kind(),
            "checkpoint_done"
        );
        // PG18 adds a SLRU clause after the buffers
        assert_eq!(
            log(
                "checkpoint complete: wrote 3 buffers (0.0%), wrote 1 SLRU buffers; 0 WAL file(s) added, 0 removed, 0 recycled; write=0.003 s, sync=0.001 s, total=0.012 s; sync files=2, longest=0.001 s, average=0.001 s; distance=0 kB, estimate=0 kB"
            ),
            Event::CheckpointDone {
                restart: false,
                buffers: 3,
                write_ms: 3.0,
                sync_ms: 1.0,
                total_ms: 12.0,
                distance_kb: Some(0),
                estimate_kb: Some(0),
            }
        );
        assert_eq!(log("checkpoint complete: nothing useful"), Event::Other);
    }

    #[test]
    fn checkpoint_distance_and_estimate_are_read() {
        let done = log(
            "checkpoint complete: wrote 4679 buffers (28.6%); 0 WAL file(s) added, 0 removed, 2 recycled; write=0.016 s, sync=0.001 s, total=0.017 s; sync files=796, longest=0.001 s, average=0.000 s; distance=36899 kB, estimate=40000 kB; lsn=0/38F8528, redo lsn=0/38F84D0",
        );
        let Event::CheckpointDone {
            distance_kb,
            estimate_kb,
            ..
        } = done
        else {
            panic!("not a checkpoint: {done:?}");
        };
        assert_eq!((distance_kb, estimate_kb), (Some(36_899), Some(40_000)));
    }

    #[test]
    fn abrupt_endings_and_error_groups() {
        let reason = |level, message: &str| {
            disconnect_reason(&LogRecord::new(Timestamp::UNIX_EPOCH, level, message))
        };
        assert_eq!(
            reason(
                Severity::Log,
                "could not receive data from client: Connection reset by peer"
            ),
            Some("client_reset")
        );
        assert_eq!(
            reason(
                Severity::Log,
                "unexpected EOF on client connection with an open transaction"
            ),
            Some("client_gone_in_transaction")
        );
        assert_eq!(
            reason(
                Severity::Fatal,
                "terminating connection due to idle-in-transaction timeout"
            ),
            Some("idle_in_transaction_timeout")
        );
        assert_eq!(reason(Severity::Log, "checkpoint starting: time"), None);
        // Values go, constraint names stay.
        let group = error_group;
        assert_eq!(
            group(
                Some("23503"),
                "insert or update on table \"film_actor\" violates foreign key constraint \"film_actor_actor_id_fkey\""
            ),
            "insert or update on table \"film_actor\" violates foreign key constraint \"film_actor_actor_id_fkey\""
        );
        assert_ne!(
            group(Some("23503"), "violates foreign key constraint \"a\""),
            group(Some("23503"), "violates foreign key constraint \"b\"")
        );
        assert_eq!(
            group(
                Some("22P02"),
                "invalid input syntax for type integer: \"4111\""
            ),
            group(
                Some("22P02"),
                "invalid input syntax for type integer: \"abc\""
            )
        );
        assert_eq!(
            group(None, "process 123 acquired lock after 42.5 ms"),
            group(None, "process 456 acquired lock after 7 ms")
        );
    }

    #[test]
    fn autovacuum_and_autoanalyze() {
        let vacuum = "automatic vacuum of table \"pagila.public.film\": index scans: 1\npages: 0 removed, 100 remain, 100 scanned (100.00% of total)\ntuples: 5 removed, 1000 remain, 0 are dead but not yet removable\nbuffer usage: 50 hits, 0 misses, 0 dirtied\nsystem usage: CPU: user: 0.01 s, system: 0.00 s, elapsed: 0.05 s";
        assert_eq!(
            log(vacuum),
            Event::Autovacuum {
                analyze: false,
                table: "pagila.public.film".to_owned(),
                elapsed_ms: Some(50.0),
                pages_removed: Some(0),
                tuples_removed: Some(5),
                index_scans: Some(1),
            }
        );
        let analyze = "automatic analyze of table \"pagila.public.film\"\navg read rate: 0.000 MB/s, avg write rate: 0.000 MB/s\nsystem usage: CPU: user: 0.00 s, system: 0.00 s, elapsed: 0.01 s";
        assert_eq!(
            log(analyze),
            Event::Autovacuum {
                analyze: true,
                table: "pagila.public.film".to_owned(),
                elapsed_ms: Some(10.0),
                pages_removed: None,
                tuples_removed: None,
                index_scans: None,
            }
        );
        assert_eq!(
            log(
                "automatic aggressive vacuum to prevent wraparound of table \"d.s.t\": index scans: 0"
            ),
            Event::Autovacuum {
                analyze: false,
                table: "d.s.t".to_owned(),
                elapsed_ms: None,
                pages_removed: None,
                tuples_removed: None,
                index_scans: Some(0),
            }
        );
        assert_eq!(
            log("automatic vacuum of table without quotes"),
            Event::Other
        );
    }

    #[test]
    fn autovacuum_wording_of_postgres_14_to_18() {
        let table = |message: &str| match log(message) {
            Event::Autovacuum {
                analyze,
                table,
                elapsed_ms,
                ..
            } => (analyze, table, elapsed_ms),
            other => panic!("not an autovacuum event: {other:?}"),
        };
        // 14: aggressive vacuums are named as such.
        assert_eq!(
            table(
                "automatic aggressive vacuum of table \"pagila.public.film\": index scans: 0\npages: 0 removed, 10 remain\nsystem usage: CPU: user: 0.00 s, system: 0.00 s, elapsed: 0.02 s"
            ),
            (false, "pagila.public.film".to_owned(), Some(20.0))
        );
        // 15 and 16: I/O timings join the report when track_io_timing is on.
        assert_eq!(
            table(
                "automatic vacuum of table \"pagila.public.rental\": index scans: 1\nI/O timings: read: 0.123 ms, write: 0.000 ms\navg read rate: 1.5 MB/s, avg write rate: 0.0 MB/s\nsystem usage: CPU: user: 0.01 s, system: 0.00 s, elapsed: 1.50 s"
            ),
            (false, "pagila.public.rental".to_owned(), Some(1500.0))
        );
        // 17 and 18: new lines (visibility map, delay time, WAL) before the usual footer.
        assert_eq!(
            table(
                "automatic analyze of table \"pagila.public.film\"\ndelay time: 0.000 ms\nI/O timings: read: 0.100 ms, write: 0.000 ms\nsystem usage: CPU: user: 0.00 s, system: 0.00 s, elapsed: 0.03 s"
            ),
            (true, "pagila.public.film".to_owned(), Some(30.0))
        );
    }
}
