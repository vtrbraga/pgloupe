//! Links durations to the statement they belong to, per backend process.
//!
//! The log names a cursor fetch, a SQL-level `EXECUTE` or a bare `duration:` line without saying
//! which query it is about. This stage runs once per file, in file order, before records are
//! split between workers, so the result does not depend on thread count.

use std::collections::HashMap;

use crate::{
    analyze::event::{Event, StatementKind, classify},
    log::{Correlation, LogRecord, Phase},
};

/// State is dropped wholesale past this many entries; a server with that many live cursors or
/// prepared statements is already past what a log can explain.
const MAX_ENTRIES: usize = 100_000;

/// Remembers cursors, prepared statements and the last untimed statement of each backend.
#[derive(Debug, Default)]
pub struct Correlator {
    cursors: HashMap<(u32, String), String>,
    prepared: HashMap<(u32, String), String>,
    untimed: HashMap<u32, String>,
}

impl Correlator {
    /// A correlator with nothing remembered.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `record.correlation` when its duration belongs to another statement.
    pub fn apply(&mut self, record: &mut LogRecord) {
        let Some(pid) = record.pid else {
            return;
        };
        if self.cursors.len() + self.prepared.len() + self.untimed.len() > MAX_ENTRIES {
            *self = Self::default();
        }
        match classify(record) {
            Event::Statement {
                kind: kind @ (StatementKind::Statement | StatementKind::Execute),
                duration_ms,
                text,
                ..
            } => {
                let correlation = if kind == StatementKind::Statement {
                    self.simple(pid, &text)
                } else {
                    None
                };
                let effective = correlation.as_ref().map_or(text, |c| c.sql.clone());
                if duration_ms.is_none() {
                    self.untimed.insert(pid, effective);
                } else {
                    self.untimed.remove(&pid);
                }
                record.correlation = correlation;
            }
            Event::Duration { .. } => {
                record.correlation = self.untimed.remove(&pid).map(|sql| Correlation {
                    sql,
                    phase: Phase::Execute,
                    timing_only: true,
                });
            }
            Event::Disconnection { .. } => {
                self.cursors.retain(|(p, _), _| *p != pid);
                self.prepared.retain(|(p, _), _| *p != pid);
                self.untimed.remove(&pid);
            }
            _ => {}
        }
    }

    /// Statements that change or use correlation state, in the simple protocol.
    fn simple(&mut self, pid: u32, text: &str) -> Option<Correlation> {
        let trimmed = text.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        let word = lower.split(|c: char| !c.is_ascii_alphabetic()).next()?;
        match word {
            "declare" => {
                let (name, inner) = declared(trimmed)?;
                self.cursors.insert((pid, name), inner.clone());
                Some(execute(inner))
            }
            "fetch" | "move" => {
                let name = cursor_of(trimmed);
                let sql = self.cursors.get(&(pid, name))?.clone();
                Some(Correlation {
                    sql,
                    phase: Phase::Fetch,
                    timing_only: false,
                })
            }
            "close" => {
                let name = cursor_of(trimmed);
                if name.eq_ignore_ascii_case("all") {
                    self.cursors.retain(|(p, _), _| *p != pid);
                } else {
                    self.cursors.remove(&(pid, name));
                }
                None
            }
            "commit" | "rollback" | "end" | "abort" => {
                self.cursors.retain(|(p, _), _| *p != pid);
                None
            }
            "prepare" => {
                if let Some((name, body)) = prepared(trimmed) {
                    self.prepared.insert((pid, name), body);
                }
                None
            }
            "execute" => {
                let name = statement_name(trimmed.get(7..)?);
                let sql = self.prepared.get(&(pid, name))?.clone();
                Some(execute(sql))
            }
            "deallocate" => {
                let name = statement_name(
                    trimmed
                        .get(10..)?
                        .trim_start_matches(|c: char| c.is_whitespace()),
                );
                let name = name.strip_prefix("prepare ").unwrap_or(&name).to_owned();
                if name.eq_ignore_ascii_case("all") {
                    self.prepared.retain(|(p, _), _| *p != pid);
                } else {
                    self.prepared.remove(&(pid, name));
                }
                None
            }
            _ => None,
        }
    }
}

fn execute(sql: String) -> Correlation {
    Correlation {
        sql,
        phase: Phase::Execute,
        timing_only: false,
    }
}

/// The identifier at the start of `text` (after optional spaces), without quotes or a parameter
/// list.
fn statement_name(text: &str) -> String {
    let text = text.trim_start();
    let end = text
        .find(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .unwrap_or(text.len());
    text[..end].trim_matches('"').to_ascii_lowercase()
}

/// `DECLARE name [options] CURSOR [WITH|WITHOUT HOLD] FOR query` -> `(name, query)`.
fn declared(text: &str) -> Option<(String, String)> {
    let rest = text.get(7..)?.trim_start();
    let name = statement_name(rest);
    let lower = rest.to_ascii_lowercase();
    let cursor = lower.find("cursor")?;
    let after = &lower[cursor..];
    let for_at = cursor + after.find(" for ")? + 5;
    let query = rest[for_at..].trim().trim_end_matches(';').trim();
    (!name.is_empty() && !query.is_empty()).then(|| (name, query.to_owned()))
}

/// The cursor a `FETCH`, `MOVE` or `CLOSE` names: its last word.
fn cursor_of(text: &str) -> String {
    let last = text
        .trim_end()
        .trim_end_matches(';')
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or_default();
    last.trim_matches('"').to_ascii_lowercase()
}

/// `PREPARE name [(types)] AS query` -> `(name, query)`.
fn prepared(text: &str) -> Option<(String, String)> {
    let rest = text.get(7..)?.trim_start();
    let name = statement_name(rest);
    let lower = rest.to_ascii_lowercase();
    let as_at = lower.find(" as ")? + 4;
    let query = rest[as_at..].trim().trim_end_matches(';').trim();
    (!name.is_empty() && !query.is_empty()).then(|| (name, query.to_owned()))
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::log::Severity;

    fn record(pid: u32, message: &str) -> LogRecord {
        let mut r = LogRecord::new(Timestamp::UNIX_EPOCH, Severity::Log, message);
        r.pid = Some(pid);
        r
    }

    fn run(correlator: &mut Correlator, pid: u32, message: &str) -> Option<Correlation> {
        let mut r = record(pid, message);
        correlator.apply(&mut r);
        r.correlation
    }

    #[test]
    fn a_fetch_belongs_to_the_query_its_cursor_was_declared_for() {
        let mut c = Correlator::new();
        let declared = run(
            &mut c,
            7,
            "duration: 5 ms  statement: DECLARE cur CURSOR WITH HOLD FOR SELECT * FROM big;",
        )
        .unwrap();
        assert_eq!(declared.sql, "SELECT * FROM big");
        assert_eq!(declared.phase, Phase::Execute);
        for fetch in [
            "FETCH 100 FROM cur",
            "FETCH ALL IN \"cur\"",
            "move forward 5 in cur",
        ] {
            let got = run(&mut c, 7, &format!("duration: 300 ms  statement: {fetch}")).unwrap();
            assert_eq!(got.sql, "SELECT * FROM big", "{fetch}");
            assert_eq!(got.phase, Phase::Fetch);
        }
        // Another backend's cursor of the same name is a different cursor.
        assert!(run(&mut c, 8, "duration: 1 ms  statement: FETCH 1 FROM cur").is_none());
        run(&mut c, 7, "duration: 0.1 ms  statement: CLOSE cur");
        assert!(run(&mut c, 7, "duration: 1 ms  statement: FETCH 1 FROM cur").is_none());
    }

    #[test]
    fn commit_and_disconnection_forget_cursors() {
        let mut c = Correlator::new();
        run(&mut c, 7, "statement: DECLARE a CURSOR FOR SELECT 1");
        run(&mut c, 7, "duration: 1 ms  statement: COMMIT");
        assert!(run(&mut c, 7, "duration: 1 ms  statement: FETCH 1 FROM a").is_none());
        run(&mut c, 7, "statement: DECLARE b CURSOR FOR SELECT 2");
        run(
            &mut c,
            7,
            "disconnection: session time: 0:00:01.000 user=u database=d host=h port=1",
        );
        assert!(run(&mut c, 7, "duration: 1 ms  statement: FETCH 1 FROM b").is_none());
    }

    #[test]
    fn a_sql_level_execute_belongs_to_the_prepared_statement() {
        let mut c = Correlator::new();
        run(
            &mut c,
            3,
            "duration: 0.2 ms  statement: PREPARE q (int) AS SELECT * FROM film WHERE film_id = $1",
        );
        let got = run(&mut c, 3, "duration: 4 ms  statement: EXECUTE q(5)").unwrap();
        assert_eq!(got.sql, "SELECT * FROM film WHERE film_id = $1");
        assert_eq!(got.phase, Phase::Execute);
        run(&mut c, 3, "statement: DEALLOCATE q");
        assert!(run(&mut c, 3, "duration: 4 ms  statement: EXECUTE q(5)").is_none());
    }

    #[test]
    fn a_bare_duration_belongs_to_the_last_untimed_statement_of_its_backend() {
        let mut c = Correlator::new();
        assert!(run(&mut c, 1, "statement: SELECT 1").is_none());
        assert!(run(&mut c, 2, "statement: SELECT 2").is_none());
        let got = run(&mut c, 1, "duration: 9.000 ms").unwrap();
        assert_eq!((got.sql.as_str(), got.timing_only), ("SELECT 1", true));
        // Used once.
        assert!(run(&mut c, 1, "duration: 9.000 ms").is_none());
        let got = run(&mut c, 2, "duration: 1.000 ms").unwrap();
        assert_eq!(got.sql, "SELECT 2");
    }

    #[test]
    fn records_without_a_pid_are_left_alone() {
        let mut c = Correlator::new();
        let mut r = LogRecord::new(Timestamp::UNIX_EPOCH, Severity::Log, "duration: 1 ms");
        c.apply(&mut r);
        assert!(r.correlation.is_none());
    }
}
