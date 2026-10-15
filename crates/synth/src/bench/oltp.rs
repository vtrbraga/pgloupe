//! In-process workers for the events pgbench does not produce: locks, deadlocks, rollbacks,
//! temp files, errors and connections.
//!
//! They count what they do, so the counts can be compared with the server's counters and with
//! pgloupe's report.

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use anyhow::{Context as _, bail};
use postgres::{Client, error::SqlState};
use rand::{RngExt as _, SeedableRng as _};
use rand_chacha::ChaCha8Rng;

use super::truth::WorkerStats;
use crate::{rawclient::Raw, session::Target};

/// Rows of the small table the `locks` sessions fight over.
pub const HOT_ROWS: i32 = 6;

/// Creates and fills the hot table (run in the `bench` database).
pub const SETUP_SQL: &str = "CREATE TABLE bench_hot (id int PRIMARY KEY, v int NOT NULL DEFAULT 0); \
                             INSERT INTO bench_hot (id) SELECT generate_series(0, 5)";

/// Concurrent sessions of the `locks` worker.
const LOCK_SESSIONS: u32 = 8;

/// How long a `locks` transaction holds its first row; above `deadlock_timeout` (200 ms) so
/// waiters log a lock wait.
const HOLD: Duration = Duration::from_millis(250);

/// One transaction in 100 ends with an explicit `ROLLBACK`.
const ROLLBACK_ONE_IN: u32 = 100;

/// A window function forces a sort of the whole table, which spills with `work_mem = 1MB`.
const ANALYTIC_SQL: &str = "SELECT sum(aid) FROM (SELECT aid, row_number() OVER (ORDER BY filler, aid) AS rn \
                            FROM pgbench_accounts) s WHERE rn % 1000 = 0";

/// Counters a worker keeps.
type Events = BTreeMap<String, u64>;

fn bump(events: &mut Events, name: &str) {
    *events.entry(name.to_owned()).or_default() += 1;
}

fn stats(name: &str, events: Events) -> WorkerStats {
    WorkerStats {
        name: name.to_owned(),
        rounds: 0,
        transactions: 0,
        events,
    }
}

/// Two different rows of the hot table, in random order (opposite orders make deadlocks).
#[must_use]
pub fn pick_pair(rng: &mut ChaCha8Rng) -> (i32, i32) {
    let a = rng.random_range(0..HOT_ROWS);
    let mut b = rng.random_range(0..HOT_ROWS - 1);
    if b >= a {
        b += 1;
    }
    (a, b)
}

/// A statement that must fail, and how.
#[derive(Debug)]
pub struct ErrorCase {
    /// SQL to run.
    pub sql: &'static str,
    /// Expected SQLSTATE.
    pub sqlstate: &'static str,
    /// Whether to run it under a 30 ms `statement_timeout`.
    pub timeout: bool,
}

/// The failures the `errors` worker cycles through.
pub const ERROR_CASES: &[ErrorCase] = &[
    ErrorCase {
        sql: "SELEC 1",
        sqlstate: "42601",
        timeout: false,
    },
    ErrorCase {
        sql: "SELECT * FROM bench_missing",
        sqlstate: "42P01",
        timeout: false,
    },
    ErrorCase {
        sql: "INSERT INTO bench_hot (id) VALUES (0)",
        sqlstate: "23505",
        timeout: false,
    },
    ErrorCase {
        sql: "SELECT 1/0",
        sqlstate: "22012",
        timeout: false,
    },
    ErrorCase {
        sql: "SELECT pg_sleep(1)",
        sqlstate: "57014",
        timeout: true,
    },
];

/// Database, user and whether to hang up without saying goodbye, for the `n`th connection.
#[must_use]
pub const fn connection_plan(n: u64) -> (&'static str, &'static str, bool) {
    const TARGETS: [(&str, &str); 4] = [
        ("bench", "postgres"),
        ("pagila", "app_ro"),
        ("pagila", "report"),
        ("pagila", "app_rw"),
    ];
    let (db, user) = TARGETS[(n % 4) as usize];
    (db, user, n % 5 == 4)
}

/// Runs `body` until `stop`; an error from it stops everyone and is returned.
fn looped(
    stop: &AtomicBool,
    mut body: impl FnMut(&mut Events) -> anyhow::Result<()>,
) -> anyhow::Result<Events> {
    let mut events = Events::new();
    while !stop.load(Ordering::Relaxed) {
        if let Err(e) = body(&mut events) {
            stop.store(true, Ordering::Relaxed);
            return Err(e);
        }
    }
    Ok(events)
}

/// Eight sessions updating two of six rows with a pause between, in random order: lock waits,
/// deadlocks, and a few explicit rollbacks.
///
/// # Errors
///
/// Fails if a session cannot connect or hits an error other than a deadlock.
pub fn locks(target: Target, seed: u64, stop: &AtomicBool) -> anyhow::Result<WorkerStats> {
    let results: Vec<anyhow::Result<Events>> = thread::scope(|scope| {
        let handles: Vec<_> = (0..LOCK_SESSIONS)
            .map(|i| scope.spawn(move || lock_session(target, seed + u64::from(i), stop)))
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("a lock session panicked")))
            })
            .collect()
    });
    let mut total = Events::new();
    for result in results {
        for (name, n) in result? {
            *total.entry(name).or_default() += n;
        }
    }
    Ok(stats("locks", total))
}

fn lock_session(target: Target, seed: u64, stop: &AtomicBool) -> anyhow::Result<Events> {
    let mut client = target.connect("bench", "postgres", "bench:locks")?;
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    looped(stop, |events| {
        let (a, b) = pick_pair(&mut rng);
        let explicit = rng.random_range(0..ROLLBACK_ONE_IN) == 0;
        let outcome = transfer(&mut client, a, b, explicit)?;
        bump(events, "transactions");
        match outcome {
            Outcome::Committed => {}
            Outcome::RolledBack => bump(events, "explicit_rollbacks"),
            Outcome::Deadlock => bump(events, "deadlocks"),
        }
        Ok(())
    })
}

enum Outcome {
    Committed,
    RolledBack,
    Deadlock,
}

fn transfer(client: &mut Client, a: i32, b: i32, explicit: bool) -> anyhow::Result<Outcome> {
    client.batch_execute("BEGIN")?;
    match update_both(client, a, b) {
        Ok(()) => {
            client.batch_execute(if explicit { "ROLLBACK" } else { "COMMIT" })?;
            Ok(if explicit {
                Outcome::RolledBack
            } else {
                Outcome::Committed
            })
        }
        Err(e) if e.code() == Some(&SqlState::T_R_DEADLOCK_DETECTED) => {
            client.batch_execute("ROLLBACK")?;
            Ok(Outcome::Deadlock)
        }
        Err(e) => Err(e).context("locks transaction"),
    }
}

fn update_both(client: &mut Client, a: i32, b: i32) -> Result<(), postgres::Error> {
    const SQL: &str = "UPDATE bench_hot SET v = v + 1 WHERE id = $1";
    client.execute(SQL, &[&a])?;
    thread::sleep(HOLD);
    client.execute(SQL, &[&b])?;
    Ok(())
}

/// One session sorting the whole accounts table with a 1 MB `work_mem`: temp files.
///
/// # Errors
///
/// Fails if the connection or a query fails.
pub fn analytic(target: Target, stop: &AtomicBool) -> anyhow::Result<WorkerStats> {
    let mut client = target.connect("bench", "postgres", "bench:analytic")?;
    client.batch_execute("SET work_mem = '1MB'")?;
    let events = looped(stop, |events| {
        client.query(ANALYTIC_SQL, &[]).context("analytic query")?;
        bump(events, "queries");
        thread::sleep(Duration::from_millis(500));
        Ok(())
    })?;
    Ok(stats("analytic", events))
}

/// One session running statements that fail, cycling through [`ERROR_CASES`].
///
/// # Errors
///
/// Fails if a statement succeeds or fails with another SQLSTATE than expected.
pub fn errors(target: Target, stop: &AtomicBool) -> anyhow::Result<WorkerStats> {
    let mut client = target.connect("bench", "postgres", "bench:errors")?;
    let mut n = 0_usize;
    let events = looped(stop, |events| {
        let case = &ERROR_CASES[n % ERROR_CASES.len()];
        n += 1;
        run_error_case(&mut client, case)?;
        bump(events, "errors");
        bump(events, &format!("error_{}", case.sqlstate));
        thread::sleep(Duration::from_millis(100));
        Ok(())
    })?;
    Ok(stats("errors", events))
}

fn run_error_case(client: &mut Client, case: &ErrorCase) -> anyhow::Result<()> {
    if case.timeout {
        client.batch_execute("SET statement_timeout = '30ms'")?;
    }
    let result = client.batch_execute(case.sql);
    if case.timeout {
        client.batch_execute("RESET statement_timeout")?;
    }
    match result {
        Ok(()) => bail!("{:?} was expected to fail", case.sql),
        Err(e) if e.code().map(SqlState::code) == Some(case.sqlstate) => Ok(()),
        Err(e) => Err(e).with_context(|| format!("{:?} failed with another code", case.sql)),
    }
}

/// Connects, asks one question and leaves, one connection after another; every fifth leaves
/// without a goodbye.
///
/// # Errors
///
/// Fails if a connection or its query fails.
pub fn churn(target: Target, stop: &AtomicBool) -> anyhow::Result<WorkerStats> {
    let mut n = 0_u64;
    let events = looped(stop, |events| {
        let (db, user, abrupt) = connection_plan(n);
        n += 1;
        if abrupt {
            let mut raw = Raw::connect(target.port, db, user, "bench:churn")?;
            // Leaving while idle is silent; leaving inside a transaction is what the server logs.
            raw.query("BEGIN")?;
            raw.hang_up();
            bump(events, "abrupt");
        } else {
            let mut client = target.connect(db, user, "bench:churn")?;
            client.simple_query("SELECT 1")?;
            bump(events, "connects");
        }
        thread::sleep(Duration::from_millis(50));
        Ok(())
    })?;
    Ok(stats("churn", events))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn pairs_are_two_different_hot_rows() {
        let mut rng = ChaCha8Rng::seed_from_u64(7);
        let mut seen = HashSet::new();
        for _ in 0..500 {
            let (a, b) = pick_pair(&mut rng);
            assert_ne!(a, b);
            assert!((0..HOT_ROWS).contains(&a) && (0..HOT_ROWS).contains(&b));
            seen.insert((a, b));
        }
        assert!(
            seen.contains(&(1, 2)) && seen.contains(&(2, 1)),
            "both orders occur"
        );
    }

    #[test]
    fn the_setup_creates_as_many_rows_as_the_pairs_use() {
        assert!(SETUP_SQL.contains("generate_series(0, 5)"));
        assert_eq!(HOT_ROWS, 6);
    }

    #[test]
    fn error_cases_have_distinct_sqlstates() {
        let codes: HashSet<&str> = ERROR_CASES.iter().map(|c| c.sqlstate).collect();
        assert_eq!(codes.len(), ERROR_CASES.len());
        assert_eq!(ERROR_CASES.iter().filter(|c| c.timeout).count(), 1);
    }

    #[test]
    fn every_fifth_connection_hangs_up_and_all_databases_are_used() {
        let plans: Vec<_> = (0..20).map(connection_plan).collect();
        assert_eq!(plans.iter().filter(|p| p.2).count(), 4);
        let users: HashSet<&str> = plans.iter().map(|p| p.1).collect();
        assert_eq!(users.len(), 4);
        assert!(plans.iter().all(|p| p.0 == "bench" || p.0 == "pagila"));
    }
}
