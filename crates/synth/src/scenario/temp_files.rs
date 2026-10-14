//! Sorts and hashes that spill to disk under a tiny `work_mem`.

use anyhow::Context as _;

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::manifest::EventKind;

/// Queries run under a tiny `work_mem`; each must spill to disk.
pub const QUERIES: &[&str] = &[
    "SELECT count(*) FROM (SELECT g FROM generate_series(1, 300000) g ORDER BY g DESC OFFSET 0) s",
    "SELECT count(DISTINCT g) FROM generate_series(1, 300000) g",
    "SELECT count(*) FROM (SELECT r.customer_id, count(*) FROM rental r \
     JOIN payment p USING (customer_id) GROUP BY r.customer_id ORDER BY 2 DESC) s",
];

/// The `temp_files` scenario.
pub const SCENARIO: Scenario = Scenario {
    name: "temp_files",
    phase: Phase::Main,
    run,
    expect: &[ExpectSpec::at_least(EventKind::TempFile, 3)],
};

fn run(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut db = ctx.session("pagila")?;
    db.batch_execute("SET work_mem = '64kB'")?;
    for sql in QUERIES {
        db.batch_execute(sql)
            .with_context(|| format!("running {sql}"))?;
    }
    Ok(())
}
