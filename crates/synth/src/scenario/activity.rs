//! Background work and abrupt endings: checkpoints, autovacuum, temp tables, vanishing clients,
//! idle timeouts and foreign-key errors.

use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, bail};
use postgres::error::SqlState;

use super::{Ctx, ExpectSpec, Phase, Scenario};
use crate::{manifest::EventKind, rawclient::Raw};

const TEMP_TABLES_PER_APP: u32 = 5;
const FK_ROUNDS: u32 = 3;

/// Writes, then `CHECKPOINT`s twice, so the checkpointer logs distance and estimate.
pub const CHECKPOINTS: Scenario = Scenario {
    name: "checkpoints",
    phase: Phase::Main,
    run: run_checkpoints,
    expect: &[ExpectSpec::exactly(EventKind::Checkpoint, 2)],
};

/// Turns autovacuum on for two tables with tiny thresholds and waits for it to log its work.
pub const AUTOVACUUM: Scenario = Scenario {
    name: "autovacuum",
    phase: Phase::Main,
    run: run_autovacuum,
    expect: &[],
};

/// Temporary tables created in two applications.
pub const TEMP_TABLES: Scenario = Scenario {
    name: "temp_tables",
    phase: Phase::Main,
    run: run_temp_tables,
    expect: &[],
};

/// Clients that hang up without `Terminate`, with and without an open transaction.
pub const CLIENT_ABORTS: Scenario = Scenario {
    name: "client_aborts",
    phase: Phase::Main,
    run: run_client_aborts,
    expect: &[],
};

/// Sessions ended by `idle_in_transaction_session_timeout` and `idle_session_timeout`.
pub const IDLE_TIMEOUTS: Scenario = Scenario {
    name: "idle_timeouts",
    phase: Phase::Main,
    run: run_idle_timeouts,
    expect: &[ExpectSpec::error("25P03", 1), ExpectSpec::error("57P05", 1)],
};

/// Foreign-key violations of two constraints, each repeated with other values.
pub const FK_ERRORS: Scenario = Scenario {
    name: "fk_errors",
    phase: Phase::Main,
    run: run_fk_errors,
    expect: &[ExpectSpec::error("23503", 2 * FK_ROUNDS)],
};

fn run_checkpoints(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut control = ctx.control("pagila")?;
    control.batch_execute(
        "CREATE TABLE IF NOT EXISTS synth_ckpt (id serial PRIMARY KEY, payload text)",
    )?;
    for _ in 0..2 {
        control.batch_execute(
            "INSERT INTO synth_ckpt (payload) SELECT repeat('x', 200) FROM generate_series(1, 20000)",
        )?;
        control.batch_execute("CHECKPOINT")?;
    }
    Ok(())
}

fn run_autovacuum(ctx: &mut Ctx) -> anyhow::Result<()> {
    // A session reports its table activity when it ends, or after a minute idle: do the writes in
    // one that ends, so autovacuum sees them at once.
    let mut writer = ctx.control("pagila")?;
    for table in ["synth_vac_a", "synth_vac_b"] {
        writer.batch_execute(&format!(
            "CREATE TABLE IF NOT EXISTS {table} (id int PRIMARY KEY, v text) \
             WITH (autovacuum_vacuum_threshold = 5, autovacuum_vacuum_scale_factor = 0, \
                   autovacuum_analyze_threshold = 5, autovacuum_analyze_scale_factor = 0)"
        ))?;
        writer.batch_execute(&format!(
            "INSERT INTO {table} SELECT g, repeat('y', 100) FROM generate_series(1, 2000) g \
             ON CONFLICT DO NOTHING"
        ))?;
        writer.batch_execute(&format!("DELETE FROM {table} WHERE id % 2 = 0"))?;
    }
    drop(writer);
    let mut control = ctx.control("pagila")?;
    // Autovacuum is off for the rest of the run; this scenario alone turns it on.
    // One statement each: several in one message would run in a transaction block.
    for sql in [
        "ALTER SYSTEM SET autovacuum = on",
        "ALTER SYSTEM SET autovacuum_naptime = '1s'",
        "ALTER SYSTEM SET log_autovacuum_min_duration = 0",
        "SELECT pg_reload_conf()",
    ] {
        control.batch_execute(sql)?;
    }
    let deadline = Instant::now() + Duration::from_secs(90);
    let result = loop {
        // A failed query still has to turn autovacuum off again below.
        let done: i64 = match control.query_one(
            "SELECT count(*) FROM pg_stat_user_tables \
             WHERE relname IN ('synth_vac_a', 'synth_vac_b') \
               AND last_autovacuum IS NOT NULL AND last_autoanalyze IS NOT NULL",
            &[],
        ) {
            Ok(row) => row.get(0),
            Err(err) => break Err(err.into()),
        };
        if done == 2 {
            break Ok(());
        }
        if Instant::now() > deadline {
            break Err(anyhow::anyhow!(
                "autovacuum did not reach both tables in time"
            ));
        }
        thread::sleep(Duration::from_millis(500));
    };
    for sql in [
        // Back to off, not to the default: the bootstrap turned it off for every scenario.
        "ALTER SYSTEM SET autovacuum = off",
        "ALTER SYSTEM RESET autovacuum_naptime",
        "ALTER SYSTEM RESET log_autovacuum_min_duration",
        "SELECT pg_reload_conf()",
    ] {
        control.batch_execute(sql)?;
    }
    // Let a running worker finish and log before the next scenario starts.
    thread::sleep(Duration::from_secs(2));
    result
}

fn run_temp_tables(ctx: &mut Ctx) -> anyhow::Result<()> {
    for app in [format!("{}:etl", ctx.app), format!("{}:reports", ctx.app)] {
        let mut db = ctx.target.connect("pagila", "postgres", &app)?;
        for i in 0..TEMP_TABLES_PER_APP {
            db.batch_execute(&format!(
                "CREATE TEMP TABLE synth_tmp_{i} AS SELECT film_id FROM film LIMIT 10"
            ))?;
            db.batch_execute(&format!("DROP TABLE synth_tmp_{i}"))?;
        }
    }
    Ok(())
}

fn run_client_aborts(ctx: &mut Ctx) -> anyhow::Result<()> {
    for open_transaction in [true, false, true] {
        let mut raw = Raw::connect(ctx.target.port, "pagila", "postgres", &ctx.app)?;
        raw.query("SELECT 1")?;
        if open_transaction {
            raw.query("BEGIN")?;
            raw.query("SELECT count(*) FROM film")?;
        }
        raw.hang_up();
        // The backend notices the closed socket on its own schedule.
        thread::sleep(Duration::from_millis(300));
    }
    Ok(())
}

fn run_idle_timeouts(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut in_transaction = ctx.session("pagila")?;
    in_transaction.batch_execute("SET idle_in_transaction_session_timeout = '300ms'")?;
    in_transaction.batch_execute("BEGIN")?;
    thread::sleep(Duration::from_millis(900));
    let err = in_transaction
        .batch_execute("SELECT 1")
        .err()
        .context("the session should have been ended")?;
    let code = err.code().map(SqlState::code);
    if code != Some("25P03") && !err.is_closed() {
        bail!("expected the idle-in-transaction timeout, got {err}");
    }
    let mut idle = ctx.session("pagila")?;
    idle.batch_execute("SET idle_session_timeout = '300ms'")?;
    thread::sleep(Duration::from_millis(900));
    let err = idle
        .batch_execute("SELECT 1")
        .err()
        .context("the idle session should have been ended")?;
    let code = err.code().map(SqlState::code);
    if code != Some("57P05") && !err.is_closed() {
        bail!("expected the idle-session timeout, got {err}");
    }
    Ok(())
}

fn run_fk_errors(ctx: &mut Ctx) -> anyhow::Result<()> {
    let mut db = ctx.session("pagila")?;
    for round in 0..FK_ROUNDS {
        let missing = 900_000 + i32::try_from(round).unwrap_or(0);
        for sql in [
            format!("INSERT INTO film_actor (actor_id, film_id) VALUES ({missing}, 1)"),
            format!("INSERT INTO film_actor (actor_id, film_id) VALUES (1, {missing})"),
        ] {
            let err = db
                .batch_execute(&sql)
                .err()
                .with_context(|| format!("{sql} should violate a foreign key"))?;
            if err.code() != Some(&SqlState::FOREIGN_KEY_VIOLATION) {
                bail!("{sql}: expected 23503, got {err}");
            }
        }
    }
    Ok(())
}
