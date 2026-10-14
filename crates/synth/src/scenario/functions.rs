//! DML inside PL/pgSQL: visible to `pg_stat_statements` (`toplevel = false`), not to the log.

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::manifest::EventKind;

/// Function calls per run.
pub const CALLS: i32 = 20;
/// Loop iterations per call (one INSERT + one UPDATE each).
pub const ROWS_PER_CALL: i32 = 5;

const CREATE: &str =
    "CREATE FUNCTION synth_rent(call int, n int) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    FOR i IN 1..n LOOP
        INSERT INTO rental (rental_date, inventory_id, customer_id, staff_id)
        VALUES ('2030-01-01'::timestamptz + make_interval(secs => call * n + i), i, 1, 1);
        UPDATE customer SET active = active WHERE customer_id = 1;
    END LOOP;
END
$$";

/// The `functions` scenario.
pub const SCENARIO: Scenario = Scenario {
    name: "functions",
    phase: Phase::Main,
    run,
    // CREATE FUNCTION plus one top-level SELECT per call.
    expect: &[ExpectSpec::exactly(EventKind::Statement, 21)],
};

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut db = ctx.session("pagila")?;
    db.batch_execute(CREATE)?;
    for call in 0..CALLS {
        db.batch_execute(&format!("SELECT synth_rent({call}, {ROWS_PER_CALL})"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_expectation_is_create_plus_calls() {
        assert_eq!(i64::from(SCENARIO.expect[0].min), i64::from(CALLS) + 1);
    }
}
