//! One prepared statement executed many times with bound parameters.

use rand::RngExt as _;

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::manifest::EventKind;

const EXECUTIONS: u32 = 50;

/// The extended-protocol scenario.
pub const SCENARIO: Scenario = Scenario {
    name: "extended",
    phase: Phase::Main,
    run,
    expect: &[
        ExpectSpec::exactly(EventKind::Parse, 1),
        ExpectSpec::exactly(EventKind::Bind, EXECUTIONS),
        ExpectSpec::exactly(EventKind::Execute, EXECUTIONS),
    ],
};

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut db = ctx.session("pagila")?;
    let statement = db.prepare("SELECT title FROM film WHERE film_id = $1")?;
    for _ in 0..EXECUTIONS {
        let film_id: i32 = ctx.rng.random_range(1..=1000);
        db.query(&statement, &[&film_id])?;
    }
    Ok(())
}
