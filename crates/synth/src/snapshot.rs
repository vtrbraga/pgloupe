//! `pg_stat_statements` and related snapshots as CSV.

use std::{fs::File, io, path::Path};

use anyhow::Context as _;
use postgres::Client;

use crate::session::query_scalar;

const PGSS: &str = "COPY (SELECT now() AS snapshot_at, \
    current_setting('server_version_num')::int AS server_version_num, \
    d.datname, r.rolname, s.* \
    FROM pg_stat_statements s \
    LEFT JOIN pg_database d ON d.oid = s.dbid \
    LEFT JOIN pg_roles r ON r.oid = s.userid) TO STDOUT WITH (FORMAT csv, HEADER)";

const PGSS_INFO: &str = "COPY (SELECT now() AS snapshot_at, * FROM pg_stat_statements_info) \
    TO STDOUT WITH (FORMAT csv, HEADER)";

const PG_STAT_DATABASE: &str = "COPY (SELECT now() AS snapshot_at, * FROM pg_stat_database) \
    TO STDOUT WITH (FORMAT csv, HEADER)";

/// Writes snapshot `n` (`pgss_<n>.csv`, `pgss_info_<n>.csv`, `pg_stat_database_<n>.csv`) into `dir`.
///
/// Returns the server time the snapshot was taken.
///
/// # Errors
///
/// Fails if a query or file write fails.
pub fn take(control: &mut Client, dir: &Path, n: u8) -> anyhow::Result<String> {
    let at = query_scalar(control, "SELECT now()::text")?;
    copy_to(control, &dir.join(format!("pgss_{n}.csv")), PGSS)?;
    copy_to(control, &dir.join(format!("pgss_info_{n}.csv")), PGSS_INFO)?;
    copy_to(
        control,
        &dir.join(format!("pg_stat_database_{n}.csv")),
        PG_STAT_DATABASE,
    )?;
    Ok(at)
}

fn copy_to(client: &mut Client, path: &Path, sql: &str) -> anyhow::Result<()> {
    let mut reader = client
        .copy_out(sql)
        .with_context(|| format!("copying to {}", path.display()))?;
    let mut file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    io::copy(&mut reader, &mut file).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
