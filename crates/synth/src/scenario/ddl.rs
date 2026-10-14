//! Schema changes, a concurrent index build and an explicit checkpoint.

use anyhow::Context as _;

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::manifest::EventKind;

const STATEMENTS: &[&str] = &[
    "CREATE TABLE synth_ddl (id int PRIMARY KEY, note text)",
    "ALTER TABLE synth_ddl ADD COLUMN created_at timestamptz DEFAULT now()",
    "CREATE INDEX synth_ddl_note ON synth_ddl (note)",
    "CREATE INDEX CONCURRENTLY synth_ddl_created ON synth_ddl (created_at)",
    "DROP INDEX synth_ddl_note",
    "DROP TABLE synth_ddl",
    "CHECKPOINT",
];

/// The ddl scenario.
pub const SCENARIO: Scenario = Scenario {
    name: "ddl",
    phase: Phase::Main,
    run,
    expect: &[
        ExpectSpec::exactly(EventKind::Statement, 7),
        ExpectSpec::exactly(EventKind::Checkpoint, 1),
    ],
};

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut db = ctx.session("pagila")?;
    for sql in STATEMENTS {
        db.batch_execute(sql)
            .with_context(|| format!("running {sql}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_expectation_matches_list() {
        assert_eq!(SCENARIO.expect[0].min as usize, STATEMENTS.len());
    }
}
