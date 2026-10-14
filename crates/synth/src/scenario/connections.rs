//! Short sessions across several login roles.

use rand::RngExt as _;

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::{bootstrap::ROLES, manifest::EventKind};

const SESSIONS_PER_ROLE: u32 = 20;
const SESSIONS: u32 = 60;

/// The `connections` scenario.
pub const SCENARIO: Scenario = Scenario {
    name: "connections",
    phase: Phase::Main,
    run,
    expect: &[
        ExpectSpec::exactly(EventKind::ConnectionAuthorized, SESSIONS),
        ExpectSpec::exactly(EventKind::Disconnection, SESSIONS),
        ExpectSpec::exactly(EventKind::Statement, SESSIONS),
    ],
};

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    for role in ROLES {
        for _ in 0..SESSIONS_PER_ROLE {
            let film_id: u32 = ctx.rng.random_range(1..=1000);
            let mut session = ctx.target.connect("pagila", role, &ctx.app)?;
            session.batch_execute(&format!("SELECT title FROM film WHERE film_id = {film_id}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_matches_roles_times_per_role() {
        assert_eq!(SESSIONS as usize, ROLES.len() * SESSIONS_PER_ROLE as usize);
    }
}
