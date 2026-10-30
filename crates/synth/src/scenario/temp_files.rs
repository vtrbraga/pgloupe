//! Sorts and hashes that spill to disk under a tiny `work_mem`, from several roles and
//! applications and through cursors, so the report can say who spilled what.

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

/// A sort that spills when a cursor over it is first fetched.
const CURSOR_QUERY: &str = "SELECT g FROM generate_series(1, 300000) g ORDER BY g DESC";

/// A spilling query that takes a bind parameter, so it runs over the extended protocol.
const EXTENDED_QUERY: &str =
    "SELECT count(*) FROM (SELECT g FROM generate_series(1, 300000) g ORDER BY g DESC OFFSET $1) s";

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
    // Two service roles whose `work_mem` is too small for their queries: a reporting role that
    // runs the big rollup twice, and a web role that spills once.
    for (role, suffix, queries) in [
        ("report", "rollup", &[QUERIES[2], QUERIES[2]][..]),
        ("app_rw", "web", &[QUERIES[1]][..]),
    ] {
        let app = format!("{}:{suffix}", ctx.app);
        let mut client = ctx.target.connect("pagila", role, &app)?;
        client.batch_execute("SET work_mem = '64kB'")?;
        for sql in queries {
            client
                .batch_execute(sql)
                .with_context(|| format!("running {sql} as {role}"))?;
        }
    }
    // An extended-protocol client: Postgres releases its temp files after the statement context
    // is gone, so the log line names no statement.
    let app = format!("{}:extended", ctx.app);
    let mut client = ctx.target.connect("pagila", "app_rw", &app)?;
    client.batch_execute("SET work_mem = '64kB'")?;
    client
        .query(EXTENDED_QUERY, &[&0_i64])
        .context("running the extended-protocol query")?;
    // A spill logged under a cursor FETCH whose DECLARE is in the log. Each statement goes in its
    // own message, as a driver sends them.
    let app = format!("{}:cursor", ctx.app);
    let mut client = ctx.target.connect("pagila", "report", &app)?;
    client.batch_execute("SET work_mem = '64kB'")?;
    for sql in [
        "BEGIN".to_owned(),
        format!("DECLARE c_logged CURSOR FOR {CURSOR_QUERY}"),
        "FETCH ALL FROM c_logged".to_owned(),
        "COMMIT".to_owned(),
    ] {
        client.batch_execute(&sql)?;
    }
    // The same, with the DECLARE kept out of the log: the FETCH is all the report has.
    let app = format!("{}:cursor-quiet", ctx.app);
    let mut client = ctx.target.connect("pagila", "postgres", &app)?;
    client.batch_execute("SET work_mem = '64kB'")?;
    for sql in [
        "BEGIN".to_owned(),
        "SET LOCAL log_min_duration_statement = -1".to_owned(),
        format!("DECLARE c_quiet CURSOR FOR {CURSOR_QUERY}"),
        "SET LOCAL log_min_duration_statement = 0".to_owned(),
        "FETCH ALL FROM c_quiet".to_owned(),
        "COMMIT".to_owned(),
    ] {
        client.batch_execute(&sql)?;
    }
    Ok(())
}
