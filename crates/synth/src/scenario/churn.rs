//! Resets `pg_stat_statements`, then floods it past `max` so both a reset and evictions show up.

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::manifest::EventKind;

/// Distinct query shapes; above `pg_stat_statements.max` (500).
const SHAPES: u32 = 600;

/// The `pgss_churn` scenario.
pub const SCENARIO: Scenario = Scenario {
    name: "pgss_churn",
    phase: Phase::Churn,
    run,
    expect: &[ExpectSpec::exactly(EventKind::Statement, SHAPES + 1)],
};

/// Query whose normalized shape is unique for each `n`: `SELECT 1, 2, …, n`.
#[must_use]
pub fn shape(n: u32) -> String {
    let columns: Vec<String> = (1..=n).map(|i| i.to_string()).collect();
    format!("SELECT {}", columns.join(", "))
}

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    // pg_stat_statements_reset() lives in the database where the extension was created.
    let mut db = ctx.session("postgres")?;
    // The reset also zeroes `dealloc`, so evictions must come after it to be visible.
    db.batch_execute("SELECT pg_stat_statements_reset()")?;
    for n in 1..=SHAPES {
        db.batch_execute(&shape(n))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_differ_by_column_count() {
        assert_eq!(shape(1), "SELECT 1");
        assert_eq!(shape(3), "SELECT 1, 2, 3");
    }
}
