//! The typed log record shared by all decoders.

use std::str::FromStr;

use jiff::Timestamp;
use serde::Serialize;

/// Severity of a log entry (`error_severity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    /// Postgres prints `DEBUG` for every debug level.
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Log,
    Fatal,
    Panic,
}

impl Severity {
    /// Parses the label Postgres prints, e.g. `ERROR`. Secondary labels such as `DETAIL` are not
    /// severities.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Severity> {
        Some(match label {
            "DEBUG" => Severity::Debug,
            "INFO" => Severity::Info,
            "NOTICE" => Severity::Notice,
            "WARNING" => Severity::Warning,
            "ERROR" => Severity::Error,
            "LOG" => Severity::Log,
            "FATAL" => Severity::Fatal,
            "PANIC" => Severity::Panic,
            _ => return None,
        })
    }

    /// The label Postgres prints for this severity.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Severity::Debug => "DEBUG",
            Severity::Info => "INFO",
            Severity::Notice => "NOTICE",
            Severity::Warning => "WARNING",
            Severity::Error => "ERROR",
            Severity::Log => "LOG",
            Severity::Fatal => "FATAL",
            Severity::Panic => "PANIC",
        }
    }

    /// True for `ERROR`, `FATAL` and `PANIC`.
    #[must_use]
    pub fn is_error(self) -> bool {
        matches!(self, Severity::Error | Severity::Fatal | Severity::Panic)
    }
}

/// One log entry. Optional fields are `None` when the format or the prefix does not carry them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogRecord {
    pub ts: Timestamp,
    pub level: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_start: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vxid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sqlstate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_position: Option<u32>,
}

impl LogRecord {
    /// A record with only the always-present fields set.
    #[must_use]
    pub fn new(ts: Timestamp, level: Severity, message: impl Into<String>) -> Self {
        LogRecord {
            ts,
            level,
            message: message.into(),
            pid: None,
            session_id: None,
            session_line: None,
            session_start: None,
            user: None,
            database: None,
            application: None,
            host: None,
            port: None,
            backend_type: None,
            command_tag: None,
            vxid: None,
            txid: None,
            leader_pid: None,
            query_id: None,
            sqlstate: None,
            detail: None,
            hint: None,
            context: None,
            internal_query: None,
            statement: None,
            cursor_position: None,
        }
    }
}

/// An owned string, or `None` for a missing or empty value.
#[must_use]
pub fn text(value: Option<&str>) -> Option<String> {
    value.filter(|v| !v.is_empty()).map(str::to_owned)
}

/// Parses an optional number; an empty value is `None`, a bad one names `column`.
///
/// # Errors
///
/// Fails with a message naming `column` when the value is not a number.
pub fn parse_num<T: FromStr>(value: Option<&str>, column: &str) -> Result<Option<T>, String> {
    value
        .filter(|v| !v.is_empty())
        .map(|v| {
            v.parse::<T>()
                .map_err(|_err| format!("{column}: {v:?} is not a valid number"))
        })
        .transpose()
}

/// Splits csvlog's `connection_from` (`host:port`, `[local]` or empty).
///
/// # Errors
///
/// Fails when the part after the last `:` is not a port.
pub fn split_connection(value: Option<&str>) -> Result<(Option<String>, Option<u16>), String> {
    let Some(value) = value.filter(|v| !v.is_empty() && *v != "[local]") else {
        return Ok((None, None));
    };
    match value.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_err| format!("connection_from: {port:?} is not a port"))?;
            Ok((text(Some(host)), Some(port)))
        }
        None => Ok((Some(value.to_owned()), None)),
    }
}

/// SQLSTATE `00000` means "no error" and is dropped.
#[must_use]
pub fn sqlstate(value: Option<&str>) -> Option<String> {
    text(value).filter(|code| code != "00000")
}

/// Query id 0 means "not computed" and is dropped.
#[must_use]
pub fn query_id(value: Option<i64>) -> Option<i64> {
    value.filter(|id| *id != 0)
}

/// Hosts that carry no information (`[local]`, `[unknown]`, empty) are dropped.
#[must_use]
pub fn host(value: Option<&str>) -> Option<String> {
    text(value).filter(|h| h != "[local]" && h != "[unknown]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_labels_round_trip() {
        for (label, level) in [
            ("DEBUG", Severity::Debug),
            ("INFO", Severity::Info),
            ("NOTICE", Severity::Notice),
            ("WARNING", Severity::Warning),
            ("ERROR", Severity::Error),
            ("LOG", Severity::Log),
            ("FATAL", Severity::Fatal),
            ("PANIC", Severity::Panic),
        ] {
            assert_eq!(Severity::from_label(label), Some(level), "{label}");
        }
        assert_eq!(Severity::from_label("STATEMENT"), None);
        assert_eq!(Severity::from_label("log"), None);
        // Postgres prints DEBUG for every debug level; it never prints DEBUG1..5.
        assert_eq!(Severity::from_label("DEBUG1"), None);
    }

    #[test]
    fn serializes_without_empty_fields_and_with_an_upper_case_level() {
        let mut record = LogRecord::new(
            "2026-10-14T10:20:09.302Z".parse().unwrap(),
            Severity::Log,
            "hello",
        );
        record.pid = Some(42);
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"ts":"2026-10-14T10:20:09.302Z","level":"LOG","message":"hello","pid":42}"#
        );
    }

    #[test]
    fn text_drops_empty_values() {
        assert_eq!(text(Some("")), None);
        assert_eq!(text(None), None);
        assert_eq!(text(Some("x")), Some("x".to_owned()));
    }

    #[test]
    fn numbers_parse_or_name_the_column() {
        assert_eq!(parse_num::<u32>(Some("42"), "pid"), Ok(Some(42)));
        assert_eq!(parse_num::<u32>(Some(""), "pid"), Ok(None));
        assert_eq!(parse_num::<u32>(None, "pid"), Ok(None));
        let err = parse_num::<u32>(Some("x"), "pid").unwrap_err();
        assert!(err.contains("pid"), "{err}");
    }

    #[test]
    fn connection_from_splits_host_and_port() {
        assert_eq!(
            split_connection(Some("10.0.0.1:5432")),
            Ok((Some("10.0.0.1".to_owned()), Some(5432)))
        );
        assert_eq!(
            split_connection(Some("::1:5432")),
            Ok((Some("::1".to_owned()), Some(5432)))
        );
        assert_eq!(split_connection(Some("[local]")), Ok((None, None)));
        assert_eq!(split_connection(Some("")), Ok((None, None)));
        assert_eq!(split_connection(None), Ok((None, None)));
        assert_eq!(
            split_connection(Some("somehost")),
            Ok((Some("somehost".to_owned()), None))
        );
        split_connection(Some("h:port")).unwrap_err();
    }

    #[test]
    fn normalizers() {
        assert_eq!(sqlstate(Some("00000")), None);
        assert_eq!(sqlstate(Some("")), None);
        assert_eq!(sqlstate(Some("40P01")), Some("40P01".to_owned()));
        assert_eq!(query_id(Some(0)), None);
        assert_eq!(query_id(Some(-5)), Some(-5));
        assert_eq!(host(Some("[local]")), None);
        assert_eq!(host(Some("[unknown]")), None);
        assert_eq!(host(Some("10.0.0.1")), Some("10.0.0.1".to_owned()));
    }

    #[test]
    fn labels_and_error_levels() {
        assert_eq!(Severity::Log.label(), "LOG");
        assert_eq!(Severity::Debug.label(), "DEBUG");
        assert!(
            Severity::Error.is_error() && Severity::Fatal.is_error() && Severity::Panic.is_error()
        );
        assert!(!Severity::Warning.is_error() && !Severity::Log.is_error());
    }
}
