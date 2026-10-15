//! Sessions that log in other languages: the server translates its log messages and, in csvlog
//! and jsonlog, the severity words too.

use std::{panic, thread, time::Duration};

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::{
    container::LOCALES,
    manifest::EventKind,
    session::{backend_pid, wait_for_lock_wait},
};

const POLL_TIMEOUT: Duration = Duration::from_secs(10);
const HOLD: Duration = Duration::from_millis(400);

/// Per language: three timed statements, a temp file, a cancelled statement and a lock wait.
pub const SCENARIO: Scenario = Scenario {
    name: "locale",
    phase: Phase::Main,
    run,
    expect: &[
        // SET, count, sort, the held UPDATE and the waiting one: the statements with a duration.
        ExpectSpec::at_least(EventKind::Statement, 4 * 4),
        // One or two spill files per sort, depending on the version.
        ExpectSpec::between(EventKind::TempFile, 4, 12),
        ExpectSpec::exactly(EventKind::LockWait, 4),
        ExpectSpec::exactly(EventKind::LockAcquired, 4),
        ExpectSpec::error("57014", 4),
        ExpectSpec::error("42P01", 4),
    ],
};

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut control = ctx.control("pagila")?;
    control
        .batch_execute("CREATE TABLE IF NOT EXISTS synth_locale (id int PRIMARY KEY, v text)")?;
    control.batch_execute("INSERT INTO synth_locale VALUES (1, 'x') ON CONFLICT DO NOTHING")?;
    for locale in LOCALES {
        let setting = format!("{locale}.UTF-8");
        let mut db = ctx.session("pagila")?;
        db.batch_execute(&format!("SET lc_messages = '{setting}'"))?;
        db.batch_execute("SELECT count(*) FROM film WHERE film_id > 10")?;
        db.batch_execute("SET work_mem = '64kB'")?;
        db.batch_execute(
            "SELECT count(*) FROM (SELECT g FROM generate_series(1, 200000) g ORDER BY g DESC) s",
        )?;
        db.batch_execute("RESET work_mem")?;
        must_fail(&mut db, "SELECT * FROM synth_locale_missing")?;
        db.batch_execute("SET statement_timeout = '100ms'")?;
        must_fail(&mut db, "SELECT pg_sleep(2)")?;
        db.batch_execute("RESET statement_timeout")?;
        drop(db);
        lock_wait_in(ctx, &setting, &mut control)?;
    }
    Ok(())
}

/// Runs `sql`, which has to fail: the error is what the log should show.
fn must_fail(db: &mut postgres::Client, sql: &str) -> anyhow::Result<()> {
    if db.batch_execute(sql).is_ok() {
        anyhow::bail!("{sql} should have failed");
    }
    Ok(())
}

/// A held row lock and a waiter that logs in `setting`.
fn lock_wait_in(ctx: &Ctx, setting: &str, control: &mut postgres::Client) -> anyhow::Result<()> {
    let mut holder = ctx.session("pagila")?;
    let mut waiter = ctx.session("pagila")?;
    waiter.batch_execute(&format!("SET lc_messages = '{setting}'"))?;
    let waiter_pid = backend_pid(&mut waiter)?;
    holder.batch_execute("BEGIN")?;
    holder.batch_execute("UPDATE synth_locale SET v = 'h' WHERE id = 1")?;
    thread::scope(|s| -> anyhow::Result<()> {
        let blocked =
            s.spawn(|| waiter.batch_execute("UPDATE synth_locale SET v = 'w' WHERE id = 1"));
        let observed = wait_for_lock_wait(control, waiter_pid, POLL_TIMEOUT);
        if observed.is_ok() {
            thread::sleep(HOLD);
        }
        let committed = holder.batch_execute("COMMIT");
        let joined = blocked.join().unwrap_or_else(|p| panic::resume_unwind(p));
        observed?;
        committed?;
        joined?;
        Ok(())
    })
}
