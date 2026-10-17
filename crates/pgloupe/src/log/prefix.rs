//! `log_line_prefix` compilation and per-line matching, written by hand (no regex).

use jiff::tz::Offset;
use thiserror::Error;

use super::{
    record::{LogRecord, Severity, host},
    time::{parse_epoch, parse_log_time},
};

/// Escapes that produce a value. `%q` and `%%` are handled separately.
const ESCAPES: &str = "abcdehilmnpPQrstuvx";

/// The second `log_line_prefix` RDS for PostgreSQL allows.
///
/// It adds millisecond timestamps, the session line, SQLSTATE, session start, virtual
/// transaction, session id and application.
pub const RDS_FULL_PREFIX: &str = "%m:%r:%u@%d:[%p]:%l:%e:%s:%v:%x:%c:%q%a:";

/// A `log_line_prefix` that cannot be used.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PrefixError {
    #[error("unsupported escape `%{escape}` at position {position} of the prefix")]
    UnknownEscape { escape: char, position: usize },
    #[error("the prefix ends with a lone `%`")]
    TrailingPercent,
    #[error(
        "the prefix has no timestamp escape (%t, %m or %n) before any %q, so events cannot be placed in time"
    )]
    NoTimestamp,
}

/// What comes after the prefix on a line: `LABEL:  text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// Starts a new entry.
    Level(Severity),
    /// Attaches to the open entry.
    Detail,
    Hint,
    Query,
    Context,
    Statement,
    Location,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Literal(String),
    Escape(char),
    Quiet,
}

/// A compiled `log_line_prefix`.
#[derive(Debug, Clone)]
pub struct Prefix {
    tokens: Vec<Token>,
}

/// Splits `LABEL:  text` (two spaces after the colon) into its label and text.
#[must_use]
pub fn split_label(rest: &str) -> Option<(Label, &str)> {
    let colon = rest.find(':')?;
    let text = rest[colon..].strip_prefix(":  ")?;
    let label = match &rest[..colon] {
        "DETAIL" => Label::Detail,
        "HINT" => Label::Hint,
        "QUERY" => Label::Query,
        "CONTEXT" => Label::Context,
        "STATEMENT" => Label::Statement,
        "LOCATION" | "BACKTRACE" => Label::Location,
        // The English severities are by far the most common label: no table lookup for them.
        name @ ("DEBUG" | "INFO" | "NOTICE" | "WARNING" | "ERROR" | "LOG" | "FATAL" | "PANIC") => {
            Label::Level(Severity::from_label(name)?)
        }
        name => match super::locale::localized_secondary(name) {
            Some("DETAIL") => Label::Detail,
            Some("HINT") => Label::Hint,
            Some("QUERY") => Label::Query,
            Some("CONTEXT") => Label::Context,
            Some("STATEMENT") => Label::Statement,
            Some(_) => Label::Location,
            None => Label::Level(Severity::from_label(name)?),
        },
    };
    Some((label, text))
}

impl Prefix {
    /// Compiles the server's `log_line_prefix` setting.
    ///
    /// # Errors
    ///
    /// Fails on an unsupported escape, a trailing `%`, or a prefix that has no timestamp before
    /// any `%q`.
    pub fn compile(format: &str) -> Result<Prefix, PrefixError> {
        let mut tokens: Vec<Token> = Vec::new();
        let mut chars = format.char_indices();
        while let Some((position, c)) = chars.next() {
            if c != '%' {
                push_literal(&mut tokens, c);
                continue;
            }
            let Some((_, escape)) = chars.next() else {
                return Err(PrefixError::TrailingPercent);
            };
            match escape {
                '%' => push_literal(&mut tokens, '%'),
                'q' => tokens.push(Token::Quiet),
                e if ESCAPES.contains(e) => tokens.push(Token::Escape(e)),
                other => {
                    return Err(PrefixError::UnknownEscape {
                        escape: other,
                        position,
                    });
                }
            }
        }
        let has_timestamp = tokens
            .iter()
            .take_while(|token| **token != Token::Quiet)
            .any(|token| matches!(token, Token::Escape('t' | 'm' | 'n')));
        if !has_timestamp {
            return Err(PrefixError::NoTimestamp);
        }
        Ok(Prefix { tokens })
    }

    /// Matches `line` against the prefix, filling `record` from the escapes, and returns the label
    /// and the text after `LABEL:  `.
    ///
    /// # Errors
    ///
    /// Fails with a reason when the line does not match the prefix, has no severity label after
    /// it, or carries a value that does not parse (including an unknown timezone abbreviation).
    pub fn parse_line<'a>(
        &self,
        line: &'a str,
        record: &mut LogRecord,
        fallback: Option<Offset>,
    ) -> Result<(Label, &'a str), String> {
        let quiet = self.tokens.iter().position(|token| *token == Token::Quiet);
        let (head, tail) = match quiet {
            Some(index) => (&self.tokens[..index], Some(&self.tokens[index + 1..])),
            None => (self.tokens.as_slice(), None),
        };
        let mut budget = LINE_BUDGET;
        let mut pos = match_tokens(head, line, 0, record, fallback, tail.is_none(), &mut budget)?;
        // After `%q`, a process without session info prints nothing more of the prefix: the
        // label follows directly. If it does not, the rest of the prefix must match.
        let tail_end = tail
            .filter(|_| split_label(&line[pos..]).is_none())
            .and_then(|tail| {
                match_tokens(tail, line, pos, record, fallback, true, &mut budget).ok()
            });
        if let Some(end) = tail_end {
            pos = end;
        }
        split_label(&line[pos..]).ok_or_else(|| {
            format!(
                "no severity label after the log_line_prefix (column {})",
                pos + 1
            )
        })
    }
}

/// Matches `tokens` (which contain no `%q`) from byte `start`; returns the end position.
fn match_tokens(
    tokens: &[Token],
    line: &str,
    start: usize,
    record: &mut LogRecord,
    fallback: Option<Offset>,
    label_follows: bool,
    budget: &mut usize,
) -> Result<usize, String> {
    let mut pos = start;
    for (index, token) in tokens.iter().enumerate() {
        let rest = &line[pos..];
        match token {
            Token::Literal(text) => {
                if !rest.starts_with(text.as_str()) {
                    return Err(format!(
                        "line does not match the log_line_prefix (expected {text:?} at column {})",
                        pos + 1
                    ));
                }
                pos += text.len();
            }
            Token::Quiet => {}
            Token::Escape(escape) => {
                if *escape == 'r'
                    && let Some(len) = ipv6_remote_len(rest)
                {
                    apply('r', &rest[..len], record, fallback)?;
                    pos += len;
                    continue;
                }
                let next_literal = match tokens.get(index + 1) {
                    Some(Token::Literal(text)) => Some(text.as_str()),
                    _ => None,
                };
                if let Some(literal) = next_literal
                    && is_free_text(*escape)
                {
                    return match_free_text(
                        (*escape, literal),
                        &tokens[index + 1..],
                        line,
                        pos,
                        record,
                        (fallback, label_follows),
                        budget,
                    );
                }
                let len = escape_len(*escape, rest, next_literal).ok_or_else(|| {
                    format!(
                        "line does not match the log_line_prefix (%{escape} did not match at column {})",
                        pos + 1
                    )
                })?;
                apply(*escape, &rest[..len], record, fallback)?;
                pos += len;
            }
        }
    }
    Ok(pos)
}

/// Escapes whose value is free text, so it can hold the character that ends it: `application_name`
/// is `pgAdmin 4 - CONN:123` when the prefix has a space after `%a`.
fn is_free_text(escape: char) -> bool {
    !matches!(escape, 't' | 's' | 'm' | 'n' | 'p' | 'l' | 'x' | 'P' | 'Q')
}

/// How many places one free-text value may end before the line is declared not to match.
const FREE_TEXT_ATTEMPTS: usize = 8;

/// Attempts allowed for all the free-text escapes of one line together.
const LINE_BUDGET: usize = 24;

/// Matches a free-text escape and what follows it. The value ends at the first `literal`; when the
/// rest of the prefix does not match there, it is retried at the next one.
fn match_free_text(
    (escape, literal): (char, &str),
    rest_tokens: &[Token],
    line: &str,
    pos: usize,
    record: &mut LogRecord,
    (fallback, label_follows): (Option<Offset>, bool),
    budget: &mut usize,
) -> Result<usize, String> {
    let rest = &line[pos..];
    let mut from = 0;
    let mut failure = None;
    for _ in 0..FREE_TEXT_ATTEMPTS {
        // One budget for the whole line: nested free-text escapes cannot multiply the attempts.
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Some(found) = rest[from..].find(literal) else {
            break;
        };
        let len = from + found;
        apply(escape, &rest[..len], record, fallback)?;
        match match_tokens(
            rest_tokens,
            line,
            pos + len,
            record,
            fallback,
            label_follows,
            budget,
        ) {
            // When the prefix ends here a severity label must follow: `4` in `pgAdmin 4 - …` is
            // also a number, so only the label tells which space ended the name.
            Ok(end) if !label_follows || split_label(&line[end..]).is_some() => return Ok(end),
            Ok(_) => failure.get_or_insert_with(|| {
                format!(
                    "no severity label after the log_line_prefix (column {})",
                    pos + len + 1
                )
            }),
            Err(error) => failure.get_or_insert(error),
        };
        from = len + rest[len..].chars().next().map_or(1, char::len_utf8);
    }
    Err(failure.unwrap_or_else(|| {
        format!(
            "line does not match the log_line_prefix (%{escape} did not match at column {})",
            pos + 1
        )
    }))
}

fn push_literal(tokens: &mut Vec<Token>, c: char) {
    if let Some(Token::Literal(text)) = tokens.last_mut() {
        text.push(c);
    } else {
        tokens.push(Token::Literal(c.to_string()));
    }
}

fn digits(s: &str) -> usize {
    s.bytes().take_while(u8::is_ascii_digit).count()
}

/// Byte length of the value an escape consumes at the start of `rest`.
fn escape_len(escape: char, rest: &str, next_literal: Option<&str>) -> Option<usize> {
    match escape {
        't' | 's' => timestamp_len(rest, false),
        'm' => timestamp_len(rest, true),
        'n' => epoch_len(rest),
        'p' | 'l' | 'x' => Some(digits(rest)).filter(|n| *n > 0),
        // Empty when the process is not a parallel worker.
        'P' => Some(digits(rest)),
        'Q' => {
            let sign = usize::from(rest.starts_with('-'));
            let n = digits(&rest[sign..]);
            (n > 0).then_some(sign + n)
        }
        _ => match next_literal {
            Some(literal) => rest.find(literal),
            None => Some(rest.find(' ').unwrap_or(rest.len())),
        },
    }
}

/// `YYYY-MM-DD HH:MM:SS`, optional `.fff` (when `with_fraction`), a space, and a zone token.
fn timestamp_len(s: &str, with_fraction: bool) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let shape_ok = b[..19].iter().enumerate().all(|(i, c)| match i {
        4 | 7 => *c == b'-',
        10 => *c == b' ',
        13 | 16 => *c == b':',
        _ => c.is_ascii_digit(),
    });
    if !shape_ok {
        return None;
    }
    let mut end = 19;
    if with_fraction && b.get(end) == Some(&b'.') {
        let n = digits(&s[end + 1..]);
        if n == 0 {
            return None;
        }
        end += 1 + n;
    }
    if b.get(end) != Some(&b' ') {
        return None;
    }
    end += 1;
    Some(end + zone_len(&s[end..])?)
}

/// An alphabetic abbreviation, or a sign and two or four digits.
fn zone_len(s: &str) -> Option<usize> {
    match s.as_bytes().first()? {
        b'+' | b'-' => {
            let n = digits(&s[1..]);
            matches!(n, 2 | 4).then_some(1 + n)
        }
        c if c.is_ascii_alphabetic() => Some(s.bytes().take_while(u8::is_ascii_alphabetic).count()),
        _ => None,
    }
}

fn epoch_len(s: &str) -> Option<usize> {
    let whole = digits(s);
    if whole == 0 {
        return None;
    }
    if s[whole..].starts_with('.') {
        let fraction = digits(&s[whole + 1..]);
        if fraction > 0 {
            return Some(whole + 1 + fraction);
        }
    }
    Some(whole)
}

fn number<T: std::str::FromStr>(text: &str, escape: char) -> Result<T, String> {
    text.parse()
        .map_err(|_err| format!("%{escape}: {text:?} is not a number"))
}

fn apply(
    escape: char,
    text: &str,
    record: &mut LogRecord,
    fallback: Option<Offset>,
) -> Result<(), String> {
    let name = |value: &str| (!value.is_empty() && value != "[unknown]").then(|| value.to_owned());
    match escape {
        't' | 'm' => record.ts = parse_log_time(text, fallback)?,
        'n' => record.ts = parse_epoch(text)?,
        's' => record.session_start = Some(parse_log_time(text, fallback)?),
        'p' => record.pid = Some(number(text, 'p')?),
        'P' => {
            record.leader_pid = (!text.is_empty()).then(|| number(text, 'P')).transpose()?;
        }
        'l' => record.session_line = Some(number(text, 'l')?),
        'x' => record.txid = Some(number(text, 'x')?),
        'Q' => {
            let id: i64 = number(text, 'Q')?;
            record.query_id = (id != 0).then_some(id);
        }
        'u' => record.user = name(text),
        'd' => record.database = name(text),
        'a' => record.application = name(text),
        'h' => record.host = host(Some(text)),
        'r' => (record.host, record.port) = remote(text)?,
        'c' => record.session_id = name(text),
        'i' => record.command_tag = name(text),
        'e' => record.sqlstate = name(text).filter(|code| code != "00000"),
        'v' => record.vxid = name(text),
        'b' => record.backend_type = name(text),
        _ => {}
    }
    Ok(())
}

/// Length of an IPv6 `%r` value (`2600:1f18::1(5432)`) at the start of `rest`, whose colons would
/// otherwise end the free-text match early. `None` for any other shape.
fn ipv6_remote_len(rest: &str) -> Option<usize> {
    let open = rest.find('(')?;
    let host = &rest[..open];
    let is_address = host.contains(':')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '%' | '-' | '_'));
    let port = digits(&rest[open + 1..]);
    (is_address && port > 0 && rest[open + 1 + port..].starts_with(')')).then_some(open + port + 2)
}

/// `%r` prints `host(port)`, `[local]`, or nothing.
fn remote(text: &str) -> Result<(Option<String>, Option<u16>), String> {
    match text.split_once('(') {
        Some((host_part, port_part)) => {
            let port = port_part
                .trim_end_matches(')')
                .parse::<u16>()
                .map_err(|_err| format!("%r: {port_part:?} is not a port"))?;
            Ok((host(Some(host_part)), Some(port)))
        }
        None => Ok((host(Some(text)), None)),
    }
}

/// The timestamp escape a line starts with: `m` (with milliseconds), `t` (seconds) or `n` (epoch).
#[must_use]
pub fn leading_timestamp_escape(line: &str) -> Option<char> {
    if timestamp_len(line, true).is_some() {
        return Some(if line.as_bytes().get(19) == Some(&b'.') {
            'm'
        } else {
            't'
        });
    }
    epoch_len(line)
        .filter(|len| line[..*len].contains('.'))
        .map(|_| 'n')
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;

    const SYNTH: &str = "%m [%p] %q%u@%d/%a %Q ";
    const RDS: &str = "%t:%r:%u@%d:[%p]:";

    fn blank() -> LogRecord {
        LogRecord::new(Timestamp::UNIX_EPOCH, Severity::Log, "")
    }

    fn parse<'a>(prefix: &Prefix, line: &'a str) -> Result<(LogRecord, Label, &'a str), String> {
        let mut record = blank();
        let (label, text) = prefix.parse_line(line, &mut record, None)?;
        Ok((record, label, text))
    }

    #[test]
    fn compile_rejects_unknown_escapes_and_names_them() {
        assert_eq!(
            Prefix::compile("%m %z ").unwrap_err(),
            PrefixError::UnknownEscape {
                escape: 'z',
                position: 3
            }
        );
        assert_eq!(
            Prefix::compile("%m %").unwrap_err(),
            PrefixError::TrailingPercent
        );
    }

    #[test]
    fn compile_requires_a_timestamp_before_any_percent_q() {
        assert_eq!(
            Prefix::compile("%p ").unwrap_err(),
            PrefixError::NoTimestamp
        );
        assert_eq!(
            Prefix::compile("%q%t ").unwrap_err(),
            PrefixError::NoTimestamp
        );
        Prefix::compile("%n ").unwrap();
        Prefix::compile("%m %q%u").unwrap();
    }

    #[test]
    fn double_percent_is_a_literal() {
        let prefix = Prefix::compile("%m 100%% [%p] ").unwrap();
        let (record, label, text) =
            parse(&prefix, "2026-10-14 10:20:09.302 UTC 100% [42] LOG:  hi").unwrap();
        assert_eq!(record.pid, Some(42));
        assert_eq!(label, Label::Level(Severity::Log));
        assert_eq!(text, "hi");
    }

    #[test]
    fn synth_prefix_on_a_session_line() {
        let prefix = Prefix::compile(SYNTH).unwrap();
        let line = "2026-10-14 10:20:10.100 UTC [96] postgres@pagila/synth:demo 7567370182540008004 LOG:  duration: 12.345 ms  statement: SELECT 1";
        let (record, label, text) = parse(&prefix, line).unwrap();
        assert_eq!(
            record.ts,
            "2026-10-14T10:20:10.100Z".parse::<Timestamp>().unwrap()
        );
        assert_eq!(record.pid, Some(96));
        assert_eq!(record.user.as_deref(), Some("postgres"));
        assert_eq!(record.database.as_deref(), Some("pagila"));
        assert_eq!(record.application.as_deref(), Some("synth:demo"));
        assert_eq!(record.query_id, Some(7_567_370_182_540_008_004));
        assert_eq!(label, Label::Level(Severity::Log));
        assert_eq!(text, "duration: 12.345 ms  statement: SELECT 1");
    }

    #[test]
    fn percent_q_stops_the_prefix_for_non_session_processes() {
        let prefix = Prefix::compile(SYNTH).unwrap();
        let line = "2026-10-14 10:20:09.302 UTC [42] LOG:  database system is ready";
        let (record, label, text) = parse(&prefix, line).unwrap();
        assert_eq!(record.pid, Some(42));
        assert_eq!(record.user, None);
        assert_eq!(label, Label::Level(Severity::Log));
        assert_eq!(text, "database system is ready");
    }

    #[test]
    fn an_application_name_with_spaces_is_read_whole() {
        let prefix = Prefix::compile("%m [%p] %q%u@%d/%a %Q ").unwrap();
        for app in [
            "pgAdmin 4 - CONN:1234567",
            "DBeaver 24.1.0 - Main <pagila>",
            "psql",
        ] {
            let line = format!(
                "2026-10-14 10:00:00.000 UTC [7] bob@shop/{app} 0 LOG:  statement: SELECT 1"
            );
            let (record, _, text) = parse(&prefix, &line).unwrap();
            assert_eq!(record.application.as_deref(), Some(app), "{line}");
            assert_eq!(record.database.as_deref(), Some("shop"));
            assert_eq!(text, "statement: SELECT 1");
        }
    }

    #[test]
    fn unknown_user_database_and_application_are_none() {
        let prefix = Prefix::compile(SYNTH).unwrap();
        let line = "2026-10-14 10:20:12.432 UTC [103] [unknown]@[unknown]/[unknown] 0 LOG:  connection received: host=172.17.0.1 port=38590";
        let (record, _, _) = parse(&prefix, line).unwrap();
        assert_eq!(record.user, None);
        assert_eq!(record.database, None);
        assert_eq!(record.application, None);
        assert_eq!(record.query_id, None, "0 means not computed");
    }

    #[test]
    fn rds_default_prefix() {
        let prefix = Prefix::compile(RDS).unwrap();
        let line =
            "2026-10-14 10:20:09 UTC:10.0.0.1(5432):alice@app:[123]:LOG:  connection authorized";
        let (record, label, text) = parse(&prefix, line).unwrap();
        assert_eq!(record.host.as_deref(), Some("10.0.0.1"));
        assert_eq!(record.port, Some(5432));
        assert_eq!(record.user.as_deref(), Some("alice"));
        assert_eq!(record.database.as_deref(), Some("app"));
        assert_eq!(record.pid, Some(123));
        assert_eq!(label, Label::Level(Severity::Log));
        assert_eq!(text, "connection authorized");
    }

    #[test]
    fn rds_prefix_on_a_background_process_with_empty_fields() {
        let prefix = Prefix::compile(RDS).unwrap();
        let line = "2026-10-14 10:20:09 UTC::@:[44]:LOG:  checkpoint starting: time";
        let (record, _, text) = parse(&prefix, line).unwrap();
        assert_eq!(record.host, None);
        assert_eq!(record.user, None);
        assert_eq!(record.pid, Some(44));
        assert_eq!(text, "checkpoint starting: time");
    }

    #[test]
    fn rds_full_prefix_on_a_session_line() {
        let prefix = Prefix::compile(RDS_FULL_PREFIX).unwrap();
        let line = "2026-10-14 10:20:09.302 UTC:10.0.0.1(5432):alice@app:[123]:7:42P01:2026-10-14 10:00:00 UTC:3/1234:0:6acf5759.7b:psql:ERROR:  relation \"x\" does not exist";
        let (record, label, text) = parse(&prefix, line).unwrap();
        assert_eq!(record.host.as_deref(), Some("10.0.0.1"));
        assert_eq!(record.port, Some(5432));
        assert_eq!(record.user.as_deref(), Some("alice"));
        assert_eq!(record.database.as_deref(), Some("app"));
        assert_eq!(record.pid, Some(123));
        assert_eq!(record.session_line, Some(7));
        assert_eq!(record.sqlstate.as_deref(), Some("42P01"));
        assert!(record.session_start.is_some());
        assert_eq!(record.vxid.as_deref(), Some("3/1234"));
        assert_eq!(record.txid, Some(0));
        assert_eq!(record.session_id.as_deref(), Some("6acf5759.7b"));
        assert_eq!(record.application.as_deref(), Some("psql"));
        assert_eq!(label, Label::Level(Severity::Error));
        assert_eq!(text, "relation \"x\" does not exist");
    }

    #[test]
    fn rds_full_prefix_on_a_background_line() {
        let prefix = Prefix::compile(RDS_FULL_PREFIX).unwrap();
        let line = "2026-10-14 10:20:09.302 UTC::@:[44]:1:00000:2026-10-14 10:00:00 UTC::0:6acf5759.2c:LOG:  checkpoint starting: time";
        let (record, label, text) = parse(&prefix, line).unwrap();
        assert_eq!(record.pid, Some(44));
        assert_eq!(
            (record.host, record.user, record.database),
            (None, None, None)
        );
        assert_eq!(record.vxid, None, "an empty %v");
        assert_eq!(record.txid, Some(0));
        assert_eq!(record.session_id.as_deref(), Some("6acf5759.2c"));
        assert_eq!(record.sqlstate, None, "00000 is not an error");
        assert_eq!(record.application, None);
        assert_eq!(label, Label::Level(Severity::Log));
        assert_eq!(text, "checkpoint starting: time");
    }

    #[test]
    fn rds_full_prefix_before_authentication() {
        let prefix = Prefix::compile(RDS_FULL_PREFIX).unwrap();
        let line = "2026-10-14 10:20:09.302 UTC:10.0.0.5(40000):[unknown]@[unknown]:[2001]:1:00000:2026-10-14 10:20:09 UTC::0:6a000001.7d1:[unknown]:LOG:  connection received: host=10.0.0.5 port=40000";
        let (record, _, text) = parse(&prefix, line).unwrap();
        assert_eq!(record.host.as_deref(), Some("10.0.0.5"));
        assert_eq!((record.user, record.database), (None, None));
        assert_eq!(record.application, None);
        assert_eq!(text, "connection received: host=10.0.0.5 port=40000");
    }

    #[test]
    fn an_application_with_colons_under_the_rds_full_prefix() {
        let prefix = Prefix::compile(RDS_FULL_PREFIX).unwrap();
        let line = "2026-10-14 10:20:09.302 UTC:10.0.0.1(5432):alice@app:[123]:7:00000:2026-10-14 10:00:00 UTC:3/1234:0:6acf5759.7b:synth:demo:LOG:  hi";
        let (record, _, text) = parse(&prefix, line).unwrap();
        assert_eq!(record.application.as_deref(), Some("synth:demo"));
        assert_eq!(text, "hi");
    }

    #[test]
    fn ipv6_clients_keep_their_address_under_both_rds_prefixes() {
        let short = Prefix::compile(RDS).unwrap();
        let full = Prefix::compile(RDS_FULL_PREFIX).unwrap();
        for address in [
            "2600:1f18:abcd:1234:5678:9abc:def0:1234",
            "::1",
            "fe80::1%eth0",
            "fe80::1%en-0",
            "::ffff:10.0.0.1",
        ] {
            let line = format!(
                "2026-10-14 10:20:09 UTC:{address}(5432):alice@app:[123]:LOG:  connection authorized"
            );
            let (record, label, text) = parse(&short, &line).unwrap();
            assert_eq!(record.host.as_deref(), Some(address), "{line}");
            assert_eq!(record.port, Some(5432), "{line}");
            assert_eq!(record.user.as_deref(), Some("alice"), "{line}");
            assert_eq!(record.database.as_deref(), Some("app"), "{line}");
            assert_eq!(record.pid, Some(123), "{line}");
            assert_eq!(label, Label::Level(Severity::Log));
            assert_eq!(text, "connection authorized");

            let line = format!(
                "2026-10-14 10:20:09.302 UTC:{address}(5432):alice@app:[123]:7:00000:2026-10-14 10:00:00 UTC:3/1234:0:6acf5759.7b:psql:LOG:  hi"
            );
            let (record, _, text) = parse(&full, &line).unwrap();
            assert_eq!(record.host.as_deref(), Some(address), "{line}");
            assert_eq!(record.port, Some(5432), "{line}");
            assert_eq!(record.user.as_deref(), Some("alice"), "{line}");
            assert_eq!(record.application.as_deref(), Some("psql"), "{line}");
            assert_eq!(text, "hi");
        }
    }

    #[test]
    fn local_connections_have_no_host() {
        let prefix = Prefix::compile(RDS).unwrap();
        let line = "2026-10-14 10:20:09 UTC:[local]:alice@app:[123]:LOG:  hi";
        let (record, _, _) = parse(&prefix, line).unwrap();
        assert_eq!(record.host, None);
        assert_eq!(record.port, None);
    }

    #[test]
    fn epoch_prefix() {
        let prefix = Prefix::compile("%n [%p] ").unwrap();
        let (record, _, _) = parse(&prefix, "1760437209.289 [42] LOG:  hi").unwrap();
        assert_eq!(
            record.ts,
            "2025-10-14T10:20:09.289Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn numeric_zone_tokens_in_a_prefix() {
        let prefix = Prefix::compile("%m [%p] ").unwrap();
        let (record, _, _) = parse(&prefix, "2026-10-14 07:20:09.289 -03 [42] LOG:  hi").unwrap();
        assert_eq!(
            record.ts,
            "2026-10-14T10:20:09.289Z".parse::<Timestamp>().unwrap()
        );
        let (record, _, _) = parse(&prefix, "2026-10-14 15:50:09.289 +0530 [42] LOG:  hi").unwrap();
        assert_eq!(
            record.ts,
            "2026-10-14T10:20:09.289Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn unknown_abbreviation_reports_the_hint_unless_fallback_given() {
        let prefix = Prefix::compile("%m [%p] ").unwrap();
        let line = "2026-10-14 07:20:09.289 BRT [42] LOG:  hi";
        let err = parse(&prefix, line).unwrap_err();
        assert!(err.contains("unknown timezone abbreviation 'BRT'"), "{err}");
        let mut record = blank();
        let minus_three = Offset::from_seconds(-3 * 3600).unwrap();
        prefix
            .parse_line(line, &mut record, Some(minus_three))
            .unwrap();
        assert_eq!(
            record.ts,
            "2026-10-14T10:20:09.289Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn secondary_labels_are_recognized() {
        let prefix = Prefix::compile(SYNTH).unwrap();
        for (name, label) in [
            ("DETAIL", Label::Detail),
            ("HINT", Label::Hint),
            ("QUERY", Label::Query),
            ("CONTEXT", Label::Context),
            ("STATEMENT", Label::Statement),
            ("LOCATION", Label::Location),
            ("ERROR", Label::Level(Severity::Error)),
        ] {
            let line = format!(
                "2026-10-14 10:20:10.300 UTC [96] postgres@pagila/app 0 {name}:  some text"
            );
            let (_, got, text) = parse(&prefix, &line).unwrap();
            assert_eq!(got, label, "{name}");
            assert_eq!(text, "some text");
        }
    }

    #[test]
    fn mismatches_say_what_was_expected_and_where() {
        let prefix = Prefix::compile(SYNTH).unwrap();
        let err = parse(&prefix, "hello world").unwrap_err();
        assert!(err.contains("log_line_prefix"), "{err}");
        let err = parse(&prefix, "2026-10-14 10:20:09.302 UTC [42] no label here").unwrap_err();
        assert!(err.contains("severity label"), "{err}");
        let err = parse(&prefix, "2026-10-14 10:20:09.302 UTC [x] LOG:  hi").unwrap_err();
        assert!(err.contains("%p"), "{err}");
    }

    #[test]
    fn split_label_needs_two_spaces_and_a_known_name() {
        assert_eq!(
            split_label("LOG:  hi"),
            Some((Label::Level(Severity::Log), "hi"))
        );
        assert_eq!(split_label("LOG: hi"), None);
        assert_eq!(split_label("NOPE:  hi"), None);
        assert_eq!(
            split_label("LOG:  "),
            Some((Label::Level(Severity::Log), ""))
        );
        assert_eq!(split_label(""), None);
    }

    #[test]
    fn backtrace_lines_are_secondary_and_ignored() {
        assert_eq!(
            split_label("BACKTRACE:  postgres: frame"),
            Some((Label::Location, "postgres: frame"))
        );
    }
}
