//! One-time setup: roles, databases, extension, Pagila and pgbench data.

use anyhow::Context as _;

use crate::{
    container::PgContainer,
    dataset::PAGILA_FILES,
    session::{Target, query_scalar},
};

/// Login roles used by the `connections` scenario.
pub const ROLES: &[&str] = &["app_rw", "app_ro", "report"];

/// Creates roles and databases and loads data with statement logging off.
///
/// Returns the server time when loading finished.
///
/// # Errors
///
/// Fails if any setup statement or load command fails.
pub fn load(pg: &PgContainer, target: Target) -> anyhow::Result<String> {
    let mut control = target.control("postgres")?;
    for role in ROLES {
        control
            .batch_execute(&format!("CREATE ROLE {role} LOGIN"))
            .with_context(|| format!("creating role {role}"))?;
    }
    // CREATE DATABASE cannot run inside a multi-statement (implicit transaction) query.
    control
        .batch_execute("CREATE DATABASE pagila")
        .context("creating database pagila")?;
    control
        .batch_execute("CREATE DATABASE bench")
        .context("creating database bench")?;
    control
        .batch_execute("CREATE EXTENSION pg_stat_statements")
        .context("creating pg_stat_statements extension")?;

    for (file, _) in PAGILA_FILES {
        let path = format!("/dataset/{file}");
        pg.exec_unlogged(&[
            "psql",
            "-U",
            "postgres",
            "-d",
            "pagila",
            "-v",
            "ON_ERROR_STOP=1",
            "-q",
            "-f",
            path.as_str(),
        ])
        .with_context(|| format!("loading {file}"))?;
    }
    pg.exec_unlogged(&["pgbench", "-i", "-q", "-s", "1", "-U", "postgres", "bench"])
        .context("initializing pgbench")?;

    let mut pagila = target.control("pagila")?;
    pagila
        .batch_execute(
            "GRANT SELECT ON ALL TABLES IN SCHEMA public TO app_ro, report;
         GRANT SELECT, INSERT, UPDATE ON ALL TABLES IN SCHEMA public TO app_rw;
         GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO app_rw;",
        )
        .context("granting privileges on pagila")?;
    // Autovacuum stays off so it cannot add log lines to other scenarios. It is switched off here,
    // not on the command line, because a command-line setting cannot be changed by ALTER SYSTEM
    // and the `autovacuum` scenario has to turn it back on.
    control
        .batch_execute("ALTER SYSTEM SET autovacuum = off")
        .context("turning autovacuum off")?;
    control
        .batch_execute("SELECT pg_reload_conf()")
        .context("reloading the configuration")?;
    query_scalar(&mut control, "SELECT now()::text")
}
