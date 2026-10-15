//! Parse and bind that block, cursor fetches, and SQL-level prepared statements.

use std::{panic, thread, time::Duration};

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::{
    manifest::EventKind,
    session::{backend_pid, wait_for_lock_wait},
};

/// Long enough to stand out from the milliseconds a normal parse or bind takes.
const HOLD: Duration = Duration::from_millis(400);
const POLL_TIMEOUT: Duration = Duration::from_secs(10);
const FETCHES: u32 = 3;
const PREPARED_EXECUTIONS: u32 = 4;

/// A parse and a bind that wait for an `ACCESS EXCLUSIVE` lock held by another session.
pub const PHASES: Scenario = Scenario {
    name: "phases",
    phase: Phase::Main,
    run: run_phases,
    expect: &[
        ExpectSpec::at_least(EventKind::Parse, 2),
        ExpectSpec::at_least(EventKind::Bind, 2),
        ExpectSpec::at_least(EventKind::Execute, 2),
        ExpectSpec::exactly(EventKind::LockWait, 2),
        ExpectSpec::exactly(EventKind::LockAcquired, 2),
    ],
};

/// One cursor, fetched in batches that each take a while.
pub const CURSORS: Scenario = Scenario {
    name: "cursors",
    phase: Phase::Main,
    run: run_cursors,
    expect: &[ExpectSpec::at_least(EventKind::Statement, 3 + FETCHES)],
};

/// `PREPARE`, repeated `EXECUTE`, `DEALLOCATE` in the simple protocol.
pub const PREPARED_SQL: Scenario = Scenario {
    name: "prepared_sql",
    phase: Phase::Main,
    run: run_prepared_sql,
    expect: &[ExpectSpec::at_least(
        EventKind::Statement,
        2 + PREPARED_EXECUTIONS,
    )],
};

/// Holds `ACCESS EXCLUSIVE` on `synth_phase` while `blocked` runs on `session_pid`'s session, for
/// as long as it takes the lock wait to show, plus [`HOLD`].
fn hold_while<T: Send>(
    ctx: &Ctx,
    session_pid: i32,
    blocked: impl FnOnce() -> T + Send,
) -> anyhow::Result<T> {
    let mut control = ctx.control("pagila")?;
    let mut holder = ctx.session("pagila")?;
    holder.batch_execute("BEGIN")?;
    holder.batch_execute("LOCK TABLE synth_phase IN ACCESS EXCLUSIVE MODE")?;
    thread::scope(|s| {
        let running = s.spawn(blocked);
        let observed = wait_for_lock_wait(&mut control, session_pid, POLL_TIMEOUT);
        if observed.is_ok() {
            thread::sleep(HOLD);
        }
        // Always release, or joining the blocked session would hang.
        let committed = holder.batch_execute("COMMIT");
        let result = running.join().unwrap_or_else(|p| panic::resume_unwind(p));
        observed?;
        committed?;
        Ok(result)
    })
}

fn run_phases(ctx: &mut Ctx) -> anyhow::Result<()> {
    ctx.control("pagila")?
        .batch_execute("CREATE TABLE IF NOT EXISTS synth_phase (id int PRIMARY KEY, v text)")?;

    // Parse waits: the statement is not known to the server yet, so analysing it needs the lock.
    let mut parser = ctx.session("pagila")?;
    let parser_pid = backend_pid(&mut parser)?;
    let prepared = hold_while(ctx, parser_pid, || {
        parser.prepare("SELECT count(*) FROM synth_phase WHERE id > $1")
    })??;
    parser.query(&prepared, &[&0_i32])?;

    // Bind waits: parsed and described beforehand, but planning at bind needs the lock.
    let mut binder = ctx.session("pagila")?;
    let statement = binder.prepare("SELECT v FROM synth_phase WHERE id = $1")?;
    let binder_pid = backend_pid(&mut binder)?;
    hold_while(ctx, binder_pid, || binder.query(&statement, &[&1_i32]))??;
    Ok(())
}

fn run_cursors(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut db = ctx.session("pagila")?;
    db.batch_execute("BEGIN")?;
    db.batch_execute(
        "DECLARE synth_cur CURSOR FOR SELECT pg_sleep(0.002), film_id, title FROM film",
    )?;
    for _ in 0..FETCHES {
        db.batch_execute("FETCH 100 FROM synth_cur")?;
    }
    db.batch_execute("CLOSE synth_cur")?;
    db.batch_execute("COMMIT")?;
    Ok(())
}

fn run_prepared_sql(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut db = ctx.session("pagila")?;
    db.batch_execute("PREPARE synth_q (int) AS SELECT title FROM film WHERE film_id = $1")?;
    for film_id in 1..=PREPARED_EXECUTIONS {
        db.batch_execute(&format!("EXECUTE synth_q({film_id})"))?;
    }
    db.batch_execute("DEALLOCATE synth_q")?;
    Ok(())
}
