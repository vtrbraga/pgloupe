//! Row-lock contention: plain waits and deadlocks, ordered by observed lock state.

use std::{panic, thread, time::Duration};

use anyhow::bail;
use postgres::error::SqlState;

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::{
    manifest::EventKind,
    session::{backend_pid, wait_for_lock_wait},
};

const LOCK_WAIT_ROUNDS: i32 = 3;
const DEADLOCK_ROUNDS: i32 = 2;
/// Longer than `deadlock_timeout` (200 ms) so the wait is logged.
const HOLD_AFTER_WAIT: Duration = Duration::from_millis(400);
const POLL_TIMEOUT: Duration = Duration::from_secs(10);

/// Three logged lock waits that each end in acquisition.
pub const LOCK_WAIT: Scenario = Scenario {
    name: "lock_wait",
    phase: Phase::Main,
    run: run_lock_wait,
    expect: &[
        ExpectSpec::exactly(EventKind::LockWait, 3),
        ExpectSpec::exactly(EventKind::LockAcquired, 3),
    ],
};

/// Two deadlocks; which session is the victim depends on timing.
pub const DEADLOCK: Scenario = Scenario {
    name: "deadlock",
    phase: Phase::Main,
    run: run_deadlock,
    expect: &[
        ExpectSpec::exactly(EventKind::Deadlock, 2),
        ExpectSpec::between(EventKind::LockWait, 0, 2),
    ],
};

fn touch(film_id: i32) -> String {
    format!("UPDATE film SET rental_rate = rental_rate WHERE film_id = {film_id}")
}

fn run_lock_wait(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut control = ctx.control("pagila")?;
    for film_id in 10..10 + LOCK_WAIT_ROUNDS {
        let mut holder = ctx.session("pagila")?;
        let mut waiter = ctx.session("pagila")?;
        let waiter_pid = backend_pid(&mut waiter)?;
        let update = touch(film_id);
        holder.batch_execute("BEGIN")?;
        holder.batch_execute(&update)?;
        thread::scope(|s| -> anyhow::Result<()> {
            let blocked = s.spawn(|| waiter.batch_execute(&update));
            let observed = wait_for_lock_wait(&mut control, waiter_pid, POLL_TIMEOUT);
            if observed.is_ok() {
                thread::sleep(HOLD_AFTER_WAIT);
            }
            // Always release, even on failure, or joining the waiter would hang.
            let committed = holder.batch_execute("COMMIT");
            let joined = blocked.join().unwrap_or_else(|p| panic::resume_unwind(p));
            observed?;
            committed?;
            joined?;
            Ok(())
        })?;
    }
    Ok(())
}

fn is_deadlock(result: &Result<(), postgres::Error>) -> bool {
    result.as_ref().err().and_then(postgres::Error::code) == Some(&SqlState::T_R_DEADLOCK_DETECTED)
}

fn run_deadlock(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut control = ctx.control("pagila")?;
    for round in 0..DEADLOCK_ROUNDS {
        let (first, second) = (20 + 2 * round, 21 + 2 * round);
        let mut a = ctx.session("pagila")?;
        let mut b = ctx.session("pagila")?;
        let a_pid = backend_pid(&mut a)?;
        a.batch_execute("BEGIN")?;
        a.batch_execute(&touch(first))?;
        b.batch_execute("BEGIN")?;
        b.batch_execute(&touch(second))?;
        let (a_result, b_result) = thread::scope(|s| -> anyhow::Result<_> {
            let a_blocked = s.spawn(|| a.batch_execute(&touch(second)));
            let observed = wait_for_lock_wait(&mut control, a_pid, POLL_TIMEOUT);
            let b_result = observed.is_ok().then(|| b.batch_execute(&touch(first)));
            // B must end its transaction so A can finish whether or not B was the victim.
            let b_rollback = b.batch_execute("ROLLBACK");
            let a_result = a_blocked.join().unwrap_or_else(|p| panic::resume_unwind(p));
            observed?;
            b_rollback?;
            Ok((a_result, b_result.unwrap_or(Ok(()))))
        })?;
        a.batch_execute("ROLLBACK")?;
        let victims = [&a_result, &b_result]
            .into_iter()
            .filter(|r| is_deadlock(r))
            .count();
        let survivors = [&a_result, &b_result]
            .into_iter()
            .filter(|r| r.is_ok())
            .count();
        if victims != 1 || survivors != 1 {
            bail!(
                "round {round}: expected one victim and one survivor, got a={a_result:?} b={b_result:?}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::SETTINGS;

    #[test]
    fn hold_covers_at_least_two_deadlock_timeouts() {
        let (_, value) = SETTINGS
            .iter()
            .find(|(name, _)| *name == "deadlock_timeout")
            .unwrap();
        let timeout = Duration::from_millis(value.trim_end_matches("ms").parse().unwrap());
        assert!(
            HOLD_AFTER_WAIT >= timeout * 2,
            "{HOLD_AFTER_WAIT:?} vs {timeout:?}"
        );
    }
}
