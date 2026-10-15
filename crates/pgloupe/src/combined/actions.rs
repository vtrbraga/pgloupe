//! What to look at first: the problems in the data, ranked, each with what to do about it.

use serde::Serialize;

use super::{Joined, JoinedRow};
use crate::analyze::report::Report;

/// Statements named by one action.
const LISTED: usize = 5;
/// A statement this share of all execution time is worth starting with.
const DOMINANT_SHARE_PCT: f64 = 50.0;
/// A statement is only called out when at least this many statements have time recorded...
const MIN_STATEMENTS_FOR_DOMINANT: usize = 3;
/// ...and it took at least this long (milliseconds).
const MIN_DOMINANT_MS: f64 = 100.0;
/// Lock waits adding up to this much make the action urgent.
const URGENT_LOCK_WAIT_MS: f64 = 5_000.0;

/// One problem and what to do about it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Action {
    /// Stable identifier, for tools and tests.
    pub code: &'static str,
    /// `high` or `medium`.
    pub priority: &'static str,
    /// The problem in one line, with its numbers.
    pub title: String,
    /// What to do about it.
    pub advice: String,
    /// A line that shows it, when there is one: from the log, or the statement itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// What `evidence` is: `log` or `statement`.
    pub evidence_kind: &'static str,
    /// The statements involved (fingerprints), worst first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub statements: Vec<String>,
    /// The dashboard view that has the details.
    pub section: &'static str,
}

fn ms(value: f64) -> String {
    if value >= 60_000.0 {
        format!("{:.1} min", value / 60_000.0)
    } else if value >= 1_000.0 {
        format!("{:.2} s", value / 1_000.0)
    } else {
        format!("{value:.0} ms")
    }
}

#[expect(clippy::cast_precision_loss, reason = "a size shown with one decimal")]
fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1_024.0 && unit < UNITS.len() - 1 {
        size /= 1_024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn cut(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= 240 {
        line
    } else {
        let mut short: String = line.chars().take(239).collect();
        short.push('…');
        short
    }
}

/// What a well-known SQLSTATE usually means for the person reading the log.
fn error_advice(sqlstate: Option<&str>) -> &'static str {
    match sqlstate {
        Some("42P01") => {
            "A statement names a table or view that does not exist. Check that migrations ran, the search_path, and that the application connects to the right database."
        }
        Some("42703") => {
            "A statement names a column that does not exist: usually a deploy that ran before its migration, or the other way round."
        }
        Some("42601") => {
            "A statement has a syntax error: look at how the application builds its SQL (string concatenation, ORM bug)."
        }
        Some("23505") => {
            "A unique constraint was violated: the application inserts a row that exists. Use INSERT … ON CONFLICT, or handle and retry."
        }
        Some("23503") => {
            "A foreign key was violated: a row refers to one that does not exist (or still exists, on delete). Check the order of writes."
        }
        Some("23502") => "A NOT NULL column received NULL: check what the application sends.",
        Some("22012") => "A statement divided by zero: guard the divisor (NULLIF) or fix the data.",
        Some("53300") => {
            "The server ran out of connections: use a pooler, lower the application pool sizes, or raise max_connections with care."
        }
        Some("53200") => {
            "The server ran out of memory: check work_mem times concurrent sessions, and the statements below."
        }
        Some("57P01") => {
            "Sessions were ended by an administrator command or a failover: expected during maintenance, a problem otherwise."
        }
        Some("28P01" | "28000") => {
            "Logins failed: wrong credentials, or a pg_hba.conf rule. Check which user and host fail (Breakdowns view)."
        }
        _ => "Look at the statements below and at the message and DETAIL in the Errors view.",
    }
}

/// A deadlock error, with or without a SQLSTATE in the log (stderr without `%e` has none).
fn is_deadlock(error: &crate::analyze::report::ErrorRow) -> bool {
    error.sqlstate.as_deref() == Some("40P01")
        || error.first_message.starts_with("deadlock detected")
}

fn fingerprints<'a>(rows: impl IntoIterator<Item = &'a JoinedRow>) -> Vec<String> {
    rows.into_iter()
        .take(LISTED)
        .map(|row| row.fingerprint.clone())
        .collect()
}

/// The ranked actions for what the logs and the statistics show.
#[must_use]
pub fn actions(logs: Option<&Report>, joined: &Joined) -> Vec<Action> {
    let mut found = Vec::new();
    if let Some(logs) = logs {
        deadlocks(logs, &mut found);
        cancellations(logs, &mut found);
    }
    dominant_statement(joined, &mut found);
    if let Some(logs) = logs {
        lock_waits(logs, joined, &mut found);
        blockers(logs, joined, &mut found);
        temp_files(logs, joined, &mut found);
        errors(logs, &mut found);
    }
    // High first; the rules above already run in order of urgency within a priority.
    found.sort_by_key(|action| action.priority != "high");
    found
}

fn deadlocks(logs: &Report, found: &mut Vec<Action>) {
    let n = logs.locks.deadlocks;
    if n == 0 {
        return;
    }
    let error = logs.errors.iter().find(|e| is_deadlock(e));
    found.push(Action {
        code: "deadlocks",
        priority: "high",
        title: format!("{}: Postgres cancelled one transaction each time", plural(n, "deadlock", "deadlocks")),
        advice: "Two transactions each held a lock the other needed. Make every code path take its locks in the same order (same tables, same row order), keep transactions short, and retry the cancelled one. The detail below names the processes and locks involved.".to_owned(),
        evidence: error.and_then(|e| e.detail.as_deref()).map(cut),
        evidence_kind: "log",
        statements: error
            .map(|e| e.statements.iter().take(LISTED).map(|s| s.fingerprint.clone()).collect())
            .unwrap_or_default(),
        section: "errors",
    });
}

fn cancellations(logs: &Report, found: &mut Vec<Action>) {
    for row in &logs.cancelled.by_reason {
        let (title, advice) = match row.reason.as_str() {
            "statement_timeout" => (
                format!(
                    "{} hit statement_timeout",
                    plural(row.count, "statement", "statements")
                ),
                "The statement ran longer than the limit and was cancelled. Run EXPLAIN (ANALYZE, BUFFERS) on it and add an index or rewrite it; raise statement_timeout only if the work is legitimate.",
            ),
            "lock_timeout" => (
                format!(
                    "{} hit lock_timeout",
                    plural(row.count, "statement", "statements")
                ),
                "Another transaction held a conflicting lock for longer than lock_timeout. Find the blocker (pg_stat_activity, pg_locks, or the log_lock_waits lines) and shorten or reorder it.",
            ),
            "user_request" => (
                format!(
                    "{} cancelled on request",
                    plural(row.count, "statement", "statements")
                ),
                "Someone or something cancelled these (Ctrl-C, pg_cancel_backend, a client timeout). If it is an application timeout, the statement is too slow for it: see the statements below.",
            ),
            "lock_not_available" => (
                format!(
                    "{} could not get a lock they refused to wait for",
                    plural(row.count, "statement", "statements")
                ),
                "These statements asked not to wait (NOWAIT, SKIP LOCKED or a zero lock_timeout) and the row or table was locked by someone else. If that is by design, retry in the application; if not, find who holds the locks (pg_locks, the log_lock_waits lines) and shorten that transaction.",
            ),
            _ => (
                format!(
                    "{} cancelled by the server",
                    plural(row.count, "statement", "statements")
                ),
                "The server cancelled these (recovery conflicts on a standby, autovacuum yielding). On a standby, see max_standby_streaming_delay.",
            ),
        };
        found.push(Action {
            code: "cancelled",
            priority: if row.reason == "statement_timeout" || row.reason == "lock_timeout" {
                "high"
            } else {
                "medium"
            },
            title,
            advice: advice.to_owned(),
            evidence: None,
            evidence_kind: "log",
            statements: row
                .statements
                .iter()
                .take(LISTED)
                .map(|s| s.fingerprint.clone())
                .collect(),
            section: "errors",
        });
    }
}

fn dominant_statement(joined: &Joined, found: &mut Vec<Action>) {
    let total = |row: &JoinedRow| {
        row.stats
            .as_ref()
            .map(|s| (s.total_exec_ms, s.share_pct))
            .or_else(|| row.log.as_ref().map(|l| (l.total_ms, l.share_pct)))
    };
    let top = joined
        .rows
        .iter()
        .filter_map(|row| total(row).map(|(time, share)| (row, time, share)))
        .filter(|(_, time, share)| *time > 0.0 && share.is_finite())
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let Some((row, time, share)) = top else {
        return;
    };
    // One statement among a handful, or a few milliseconds in all, says nothing.
    let timed_rows = joined
        .rows
        .iter()
        .filter(|r| total(r).is_some_and(|(t, _)| t > 0.0))
        .count();
    if share < DOMINANT_SHARE_PCT
        || timed_rows < MIN_STATEMENTS_FOR_DOMINANT
        || time < MIN_DOMINANT_MS
    {
        return;
    }
    found.push(Action {
        code: "dominant_statement",
        priority: "high",
        title: format!("One statement takes {share:.0}% of the execution time ({})", ms(time)),
        advice: "Start here. Run EXPLAIN (ANALYZE, BUFFERS) on it, check for a missing index, and look at its lock waits and errors in the detail.".to_owned(),
        evidence: Some(cut(&row.text)),
        evidence_kind: "statement",
        statements: vec![row.fingerprint.clone()],
        section: "statements",
    });
}

fn lock_waits(logs: &Report, joined: &Joined, found: &mut Vec<Action>) {
    let l = &logs.locks;
    if l.waits == 0 {
        return;
    }
    let mut waiting: Vec<&JoinedRow> = joined
        .rows
        .iter()
        .filter(|row| row.log.as_ref().is_some_and(|log| log.lock_waits > 0))
        .collect();
    waiting.sort_by(|a, b| {
        let wait = |row: &JoinedRow| row.log.as_ref().map_or(0.0, |log| log.lock_wait_ms);
        wait(b)
            .total_cmp(&wait(a))
            .then(a.fingerprint.cmp(&b.fingerprint))
    });
    found.push(Action {
        code: "lock_waits",
        priority: if l.total_wait_ms >= URGENT_LOCK_WAIT_MS { "high" } else { "medium" },
        title: format!(
            "{} waited for locks, {} in total (longest {})",
            plural(l.waits, "statement", "statements"),
            ms(l.total_wait_ms),
            ms(l.max_wait_ms)
        ),
        advice: "These statements queued behind transactions that held conflicting locks. Find what held them (the DETAIL of the log_lock_waits lines names the blocking process), then shorten or reorder those transactions; lock_timeout makes waiters fail fast instead of queueing.".to_owned(),
        evidence: None,
        evidence_kind: "log",
        statements: fingerprints(waiting),
        section: "locks",
    });
}

/// The statement that made others wait the most, when the log says who held the lock.
fn blockers(logs: &Report, joined: &Joined, found: &mut Vec<Action>) {
    // Total wait per holding statement; a holder the log shows nothing of is not actionable.
    let mut by_blocker: std::collections::BTreeMap<&str, (u64, f64, &str)> =
        std::collections::BTreeMap::new();
    for row in &logs.locks.blockers {
        if let Some(blocker) = row.blocker.as_deref() {
            let entry = by_blocker
                .entry(blocker)
                .or_insert((0, 0.0, &row.blocker_text));
            entry.0 += row.count;
            entry.1 += row.total_wait_ms;
        }
    }
    let Some((fingerprint, (count, wait_ms, text))) = by_blocker
        .iter()
        .max_by(|a, b| a.1.1.total_cmp(&b.1.1).then_with(|| b.0.cmp(a.0)))
        .map(|(fp, v)| (*fp, *v))
    else {
        return;
    };
    let shown = joined.rows.iter().any(|row| row.fingerprint == fingerprint);
    found.push(Action {
        code: "blocking_statement",
        priority: if wait_ms >= URGENT_LOCK_WAIT_MS { "high" } else { "medium" },
        title: format!(
            "One statement held locks that made others wait {} times ({} in all)",
            count,
            ms(wait_ms)
        ),
        advice: "The statement shown was running, or was the last one its transaction ran before sitting idle, when others queued behind it. Shorten that transaction: commit sooner, take locks in the same order everywhere, and keep slow work out of it.".to_owned(),
        evidence: Some(cut(text)),
        evidence_kind: "statement",
        statements: if shown { vec![fingerprint.to_owned()] } else { Vec::new() },
        section: "locks",
    });
}

fn temp_files(logs: &Report, joined: &Joined, found: &mut Vec<Action>) {
    let t = &logs.temp_files;
    if t.files == 0 {
        return;
    }
    let mut spilling: Vec<&JoinedRow> = joined
        .rows
        .iter()
        .filter(|row| row.log.as_ref().is_some_and(|log| log.temp_files > 0))
        .collect();
    spilling.sort_by(|a, b| {
        let size = |row: &JoinedRow| row.log.as_ref().map_or(0, |log| log.temp_bytes);
        size(b)
            .cmp(&size(a))
            .then(a.fingerprint.cmp(&b.fingerprint))
    });
    found.push(Action {
        code: "temp_files",
        priority: "medium",
        title: format!(
            "{} written to disk ({}, largest {})",
            plural(t.files, "temp file", "temp files"),
            bytes(t.bytes),
            bytes(t.max_bytes)
        ),
        advice: format!(
            "Sorts, hashes or aggregates did not fit in work_mem and spilled. For the statements below: add an index that gives the order, reduce the rows they handle, or raise work_mem for that role or session (SET LOCAL). A work_mem of at least {} is needed to keep the largest operation in memory, and usually more: in-memory sorts and hashes take more room than they do on disk.",
            bytes(t.max_bytes)
        ),
        evidence: None,
        evidence_kind: "log",
        statements: fingerprints(spilling),
        section: "locks",
    });
}

fn errors(logs: &Report, found: &mut Vec<Action>) {
    let own = |e: &&crate::analyze::report::ErrorRow| {
        !is_deadlock(e)
            && !matches!(e.sqlstate.as_deref(), Some("57014" | "55P03"))
            && !e.first_message.starts_with("canceling statement")
    };
    let rest: Vec<_> = logs.errors.iter().filter(own).collect();
    let Some(top) = rest.first() else {
        return;
    };
    let total: u64 = rest.iter().map(|e| e.count).sum();
    found.push(Action {
        code: "errors",
        priority: "medium",
        title: format!(
            "{}{}: {} {}",
            plural(total, "error", "errors"),
            if rest.len() > 1 {
                format!(" of {} kinds, most often", rest.len())
            } else {
                String::new()
            },
            top.sqlstate.as_deref().unwrap_or("no SQLSTATE"),
            cut(top.first_message.lines().next().unwrap_or(""))
        ),
        advice: error_advice(top.sqlstate.as_deref()).to_owned(),
        evidence: top.detail.as_deref().map(cut),
        evidence_kind: "log",
        statements: top
            .statements
            .iter()
            .take(LISTED)
            .map(|s| s.fingerprint.clone())
            .collect(),
        section: "errors",
    });
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::{
        analyze::{
            acc::Accumulators,
            pipeline::Output,
            report::{ReportOptions, SortBy, all_statements},
        },
        combined::join,
        log::{LogRecord, Severity},
    };

    fn at(second: i64) -> Timestamp {
        Timestamp::from_second(1_760_000_000 + second).unwrap()
    }

    fn statement(second: i64, ms: f64, sql: &str) -> LogRecord {
        LogRecord::new(
            at(second),
            Severity::Log,
            format!("duration: {ms} ms  statement: {sql}"),
        )
    }

    fn failure(second: i64, sqlstate: &str, message: &str, sql: &str) -> LogRecord {
        LogRecord {
            sqlstate: Some(sqlstate.to_owned()),
            statement: Some(sql.to_owned()),
            ..LogRecord::new(at(second), Severity::Error, message)
        }
    }

    fn run(records: &[LogRecord]) -> Vec<Action> {
        let mut acc = Accumulators::new(100, 60);
        for record in records {
            acc.add_record(record);
        }
        let rows = all_statements(&acc);
        let report = Report::build(
            &Output {
                acc,
                files: Vec::new(),
            },
            &ReportOptions {
                by: SortBy::Total,
                top: 20,
            },
        );
        actions(Some(&report), &join(rows, None))
    }

    fn codes(found: &[Action]) -> Vec<&'static str> {
        found.iter().map(|a| a.code).collect()
    }

    #[test]
    fn a_quiet_log_has_nothing_to_fix() {
        let found = run(&[
            statement(1, 1.0, "SELECT 1 FROM a"),
            statement(2, 2.0, "SELECT 1 FROM b"),
            statement(3, 2.0, "SELECT 1 FROM c"),
        ]);
        assert_eq!(codes(&found), Vec::<&str>::new());
    }

    #[test]
    fn a_deadlock_comes_first_with_its_detail_and_statements() {
        let mut error = failure(5, "40P01", "deadlock detected", "UPDATE film SET x = 1");
        error.detail = Some(
            "Process 1 waits for ShareLock on transaction 9; blocked by process 2.".to_owned(),
        );
        let found = run(&[
            statement(1, 5.0, "SELECT 1 FROM a"),
            statement(2, 900.0, "SELECT 1 FROM big"),
            LogRecord {
                detail: None,
                ..failure(
                    6,
                    "42P01",
                    "relation \"x\" does not exist",
                    "SELECT * FROM x",
                )
            },
            error,
        ]);
        assert_eq!(found[0].code, "deadlocks");
        assert_eq!(found[0].priority, "high");
        assert!(found[0].evidence.as_deref().unwrap().contains("ShareLock"));
        assert_eq!(found[0].statements.len(), 1);
        assert!(found[0].advice.contains("same order"));
    }

    #[test]
    fn timeouts_name_the_statement_and_say_what_to_try() {
        let found = run(&[
            statement(1, 1.0, "SELECT 1 FROM a"),
            failure(
                2,
                "57014",
                "canceling statement due to statement timeout",
                "SELECT pg_sleep(60)",
            ),
        ]);
        let cancelled = found.iter().find(|a| a.code == "cancelled").unwrap();
        assert_eq!(cancelled.priority, "high");
        assert!(
            cancelled.title.contains("statement_timeout"),
            "{}",
            cancelled.title
        );
        assert!(cancelled.advice.contains("EXPLAIN"));
        assert_eq!(cancelled.statements.len(), 1);
    }

    #[test]
    fn a_statement_with_most_of_the_time_is_called_out() {
        let found = run(&[
            statement(1, 1.0, "SELECT 1 FROM a"),
            statement(1, 1.0, "SELECT 1 FROM b"),
            statement(2, 990.0, "SELECT * FROM huge"),
        ]);
        let dominant = found
            .iter()
            .find(|a| a.code == "dominant_statement")
            .unwrap();
        assert!(dominant.title.contains("100%"), "{}", dominant.title);
        assert!(dominant.evidence.as_deref().unwrap().contains("huge"));
        assert_eq!(dominant.evidence_kind, "statement");
    }

    #[test]
    fn a_tiny_log_does_not_call_anything_dominant() {
        let few = run(&[
            statement(1, 990.0, "SELECT * FROM huge"),
            statement(2, 1.0, "SELECT 1 FROM a"),
        ]);
        assert!(!codes(&few).contains(&"dominant_statement"), "{few:?}");
        let fast = run(&[
            statement(1, 40.0, "SELECT 1 FROM a"),
            statement(2, 1.0, "SELECT 1 FROM b"),
            statement(3, 1.0, "SELECT 1 FROM c"),
        ]);
        assert!(!codes(&fast).contains(&"dominant_statement"), "{fast:?}");
    }

    #[test]
    fn each_cancellation_reason_lists_its_own_statements() {
        let found = run(&[
            statement(1, 1.0, "SELECT 1 FROM a"),
            failure(
                2,
                "57014",
                "canceling statement due to statement timeout",
                "SELECT pg_sleep(60)",
            ),
            failure(
                3,
                "57014",
                "canceling statement due to user request",
                "SELECT 42",
            ),
        ]);
        let cancelled: Vec<&Action> = found.iter().filter(|a| a.code == "cancelled").collect();
        assert_eq!(cancelled.len(), 2);
        assert!(
            cancelled.iter().all(|a| a.statements.len() == 1),
            "{cancelled:?}"
        );
        assert_ne!(cancelled[0].statements, cancelled[1].statements);
    }

    #[test]
    fn nowait_failures_get_their_own_advice() {
        let found = run(&[
            statement(1, 1.0, "SELECT 1 FROM a"),
            failure(
                2,
                "55P03",
                "could not obtain lock on row in relation \"t\"",
                "SELECT * FROM t FOR UPDATE NOWAIT",
            ),
        ]);
        let action = found.iter().find(|a| a.code == "cancelled").unwrap();
        assert!(action.advice.contains("NOWAIT"), "{}", action.advice);
        assert!(!action.advice.contains("standby"));
    }

    #[test]
    fn a_deadlock_without_a_sqlstate_is_still_a_deadlock() {
        let mut error = LogRecord::new(at(2), Severity::Error, "deadlock detected");
        error.statement = Some("UPDATE t SET a = 1".to_owned());
        error.detail = Some("Process 1 waits for ShareLock".to_owned());
        let found = run(&[statement(1, 1.0, "SELECT 1 FROM a"), error]);
        assert_eq!(found[0].code, "deadlocks");
        assert!(found[0].evidence.is_some());
        assert!(!codes(&found).contains(&"errors"), "{found:?}");
    }

    #[test]
    fn plain_errors_get_advice_for_their_sqlstate() {
        let found = run(&[
            statement(1, 1.0, "SELECT 1 FROM a"),
            statement(1, 1.0, "SELECT 1 FROM b"),
            statement(1, 1.0, "SELECT 1 FROM c"),
            failure(
                3,
                "23505",
                "duplicate key value violates unique constraint",
                "INSERT INTO t VALUES (1)",
            ),
        ]);
        let errors = found.iter().find(|a| a.code == "errors").unwrap();
        assert_eq!(errors.priority, "medium");
        assert!(errors.advice.contains("ON CONFLICT"), "{}", errors.advice);
        assert!(errors.title.contains("23505"));
    }

    #[test]
    fn deadlocks_and_cancellations_are_not_also_counted_as_plain_errors() {
        let found = run(&[
            statement(1, 1.0, "SELECT 1 FROM a"),
            statement(1, 1.0, "SELECT 1 FROM b"),
            statement(1, 1.0, "SELECT 1 FROM c"),
            failure(2, "40P01", "deadlock detected", "UPDATE t SET a = 1"),
            failure(
                3,
                "57014",
                "canceling statement due to user request",
                "SELECT 1",
            ),
        ]);
        assert!(!codes(&found).contains(&"errors"), "{found:?}");
    }

    #[test]
    fn high_priority_actions_precede_medium_ones() {
        let found = run(&[
            statement(1, 1.0, "SELECT 1 FROM a"),
            failure(2, "23505", "duplicate key", "INSERT INTO t VALUES (1)"),
            failure(
                3,
                "57014",
                "canceling statement due to statement timeout",
                "SELECT pg_sleep(9)",
            ),
        ]);
        let priorities: Vec<_> = found.iter().map(|a| a.priority).collect();
        let first_medium = priorities.iter().position(|p| *p == "medium").unwrap();
        assert!(priorities[..first_medium].iter().all(|p| *p == "high"));
    }
}
