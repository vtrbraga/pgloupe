//! The SQL that reads `pg_stat_statements`, and the snapshot files it becomes.

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use jiff::Timestamp;
use postgres::Client;

use super::CollectError;

/// The statistics, with the database and role names the ids stand for.
pub const PGSS: &str = "COPY (SELECT now() AS snapshot_at, \
    current_setting('server_version_num')::int AS server_version_num, \
    d.datname, r.rolname, s.* \
    FROM pg_stat_statements s \
    LEFT JOIN pg_database d ON d.oid = s.dbid \
    LEFT JOIN pg_roles r ON r.oid = s.userid) TO STDOUT WITH (FORMAT csv, HEADER)";

/// The extension's own reset time and eviction counter (Postgres 14 and later).
pub const PGSS_INFO: &str = "COPY (SELECT now() AS snapshot_at, * FROM pg_stat_statements_info) \
    TO STDOUT WITH (FORMAT csv, HEADER)";

/// The server's settings, so a report can say which ones limit it without asking the server again.
pub const SETTINGS: &str = "COPY (SELECT name, setting, unit, source, pending_restart \
    FROM pg_settings ORDER BY name) TO STDOUT WITH (FORMAT csv, HEADER)";

/// Where one snapshot went.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The statistics file.
    pub pgss: PathBuf,
    /// The info file, on servers that have the view.
    pub info: Option<PathBuf>,
    /// The settings file written beside it, when the role could read them.
    pub settings: Option<PathBuf>,
    /// Server time of the snapshot.
    pub taken_at: Timestamp,
    /// Entries whose query text the role may not read.
    pub unreadable_text: u64,
}

fn query_error(step: &'static str, error: &postgres::Error) -> CollectError {
    let mut message = error.to_string();
    if let Some(db) = error.as_db_error() {
        db.message().clone_into(&mut message);
        if let Some(hint) = db.hint() {
            message = format!("{message} ({hint})");
        }
    }
    CollectError::Query { step, message }
}

fn write_atomically(
    client: &mut Client,
    sql: &str,
    path: &Path,
    step: &'static str,
) -> Result<(), CollectError> {
    let io_error = |source| CollectError::Io {
        path: path.display().to_string(),
        source,
    };
    let tmp = path.with_extension("csv.part");
    let mut reader = client.copy_out(sql).map_err(|e| query_error(step, &e))?;
    let mut file = File::create(&tmp).map_err(io_error)?;
    io::copy(&mut reader, &mut file).map_err(io_error)?;
    drop(reader);
    fs::rename(&tmp, path).map_err(io_error)
}

/// `pgss_20261014T102009Z.csv`: sortable, and unique per second.
fn stamp(at: Timestamp) -> String {
    at.strftime("%Y%m%dT%H%M%SZ").to_string()
}

/// Writes one snapshot (`pgss_<time>.csv`, and `pgss_info_<time>.csv` where the server has the
/// info view) into `dir`.
///
/// # Errors
///
/// Fails if `pg_stat_statements` is not usable (the message says how to enable it), a query
/// fails, or a file cannot be written.
pub fn take(client: &mut Client, dir: &Path) -> Result<Snapshot, CollectError> {
    // One transaction, so the statistics, their info row and the settings describe one moment.
    client
        .batch_execute("BEGIN READ ONLY ISOLATION LEVEL REPEATABLE READ")
        .map_err(|e| query_error("starting the snapshot transaction", &e))?;
    let taken = take_in(client, dir);
    let end = if taken.is_ok() { "COMMIT" } else { "ROLLBACK" };
    drop(client.batch_execute(end));
    taken
}

fn take_in(client: &mut Client, dir: &Path) -> Result<Snapshot, CollectError> {
    fs::create_dir_all(dir).map_err(|source| CollectError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    let version: i32 = client
        .query_one("SELECT current_setting('server_version_num')::int", &[])
        .map_err(|e| query_error("reading the server version", &e))?
        .get(0);
    let loaded = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements')",
            &[],
        )
        .map_err(|e| query_error("looking for pg_stat_statements", &e))?
        .get::<_, bool>(0);
    if !loaded {
        return Err(CollectError::Query {
            step: "pg_stat_statements",
            message: "the extension is not created in this database: add it to shared_preload_libraries (on RDS, the parameter group; a reboot is needed) and run CREATE EXTENSION pg_stat_statements".to_owned(),
        });
    }
    let taken_at: Timestamp = {
        let row = client
            .query_one("SELECT extract(epoch FROM now())::float8", &[])
            .map_err(|e| query_error("reading the server time", &e))?;
        let seconds: f64 = row.get(0);
        #[expect(clippy::cast_possible_truncation, reason = "whole seconds of an epoch")]
        Timestamp::from_second(seconds as i64).map_err(|e| CollectError::Query {
            step: "reading the server time",
            message: e.to_string(),
        })?
    };
    let name = stamp(taken_at);
    let pgss = dir.join(format!("pgss_{name}.csv"));
    write_atomically(client, PGSS, &pgss, "reading pg_stat_statements")?;
    let info = if version >= 140_000 {
        let path = dir.join(format!("pgss_info_{name}.csv"));
        write_atomically(client, PGSS_INFO, &path, "reading pg_stat_statements_info")?;
        Some(path)
    } else {
        None
    };
    // Settings help the report but are not worth failing a snapshot for.
    let settings_path = dir.join(format!("settings_{name}.csv"));
    let settings = write_atomically(client, SETTINGS, &settings_path, "reading pg_settings")
        .ok()
        .map(|()| settings_path);
    let unreadable_text: i64 = client
        .query_one(
            "SELECT count(*) FROM pg_stat_statements WHERE query = '<insufficient privilege>'",
            &[],
        )
        .map_err(|e| query_error("checking query text privileges", &e))?
        .get(0);
    Ok(Snapshot {
        pgss,
        info,
        settings,
        taken_at,
        unreadable_text: u64::try_from(unreadable_text).unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_sort_by_time_and_carry_no_separators() {
        let at: Timestamp = "2026-10-14T10:20:09Z".parse().unwrap();
        assert_eq!(stamp(at), "20261014T102009Z");
        let later: Timestamp = "2026-10-14T10:25:09Z".parse().unwrap();
        assert!(stamp(at) < stamp(later));
    }

    #[test]
    fn the_snapshot_sql_only_reads() {
        for sql in [PGSS, PGSS_INFO, SETTINGS] {
            let lower = sql.to_ascii_lowercase();
            assert!(lower.starts_with("copy (select"), "{sql}");
            assert!(lower.contains("to stdout"));
            for verb in ["insert ", "update ", "delete ", "reset", "drop ", "alter "] {
                assert!(!lower.contains(verb), "{verb} in {sql}");
            }
        }
    }
}
