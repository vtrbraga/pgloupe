//! Clients with the application names real tools use, for filtering by name pattern.

use super::{Ctx, Phase, Scenario};

/// Names with spaces, versions and counters: what `pgAdmin` and `DBeaver` send, and a fleet of
/// numbered workers.
pub const NAMES: &[&str] = &[
    "pgAdmin 4 - CONN:1234567",
    "pgAdmin 4 - CONN:7654321",
    "DBeaver 24.1.0 - Main <pagila>",
    "DBeaver 24.2.1 - Metadata",
    "app-web-1",
    "app-web-2",
];

/// One statement per application name.
pub const SCENARIO: Scenario = Scenario {
    name: "apps",
    phase: Phase::Main,
    run,
    expect: &[],
};

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    for name in NAMES {
        let mut db = ctx.target.connect("pagila", "postgres", name)?;
        db.batch_execute("SELECT count(*) FROM film")?;
    }
    Ok(())
}
