//! The SQLite store: a manifest of ingested logs and their per-hour statistics.

mod ingest;
mod manifest;
pub mod report;
mod schema;

use std::path::Path;

pub use ingest::{FileOutcome, Status, ingest};
pub use manifest::{Input, StoredDiag};

use rusqlite::{Connection, OpenFlags, ffi::ErrorCode};
use thiserror::Error;

/// Version written to `PRAGMA user_version`; a store with another number is refused.
pub const SCHEMA_VERSION: i32 = 2;

/// Written to `PRAGMA application_id` (the bytes `pglp`), so another program's database that
/// happens to carry our version number is not mistaken for a store.
const APPLICATION_ID: i32 = 0x7067_6C70;

/// Why a store could not be opened or used.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("{path}: not a pgloupe store")]
    NotAStore { path: String },
    #[error(
        "{path}: store schema {found} is newer than this pgloupe supports ({supported}); upgrade pgloupe"
    )]
    Newer {
        path: String,
        found: i32,
        supported: i32,
    },
    #[error(
        "{path}: store schema {found} is older than this pgloupe uses ({supported}); delete the file and ingest again"
    )]
    Outdated {
        path: String,
        found: i32,
        supported: i32,
    },
    #[error("{path}: is a directory, not a store file")]
    IsDirectory { path: String },
    #[error("{path}: no such store; create it with `pgloupe logs ingest`")]
    Missing { path: String },
    #[error("{path}: store is locked by another process")]
    Locked { path: String },
    #[error("store database error")]
    Sql(#[from] rusqlite::Error),
    #[error("store holds data this version cannot read")]
    Json(#[from] serde_json::Error),
    #[error("store holds a malformed content hash")]
    BadHash,
    #[error("store data could not be compressed or decompressed")]
    Compression(#[from] std::io::Error),
}

/// An open store.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    path: String,
}

/// Turns the SQLite errors that mean something to a user into [`StoreError`]s.
pub(crate) fn sql_error(err: rusqlite::Error, path: &str) -> StoreError {
    if let rusqlite::Error::SqliteFailure(failure, _) = &err {
        match failure.code {
            ErrorCode::NotADatabase => {
                return StoreError::NotAStore {
                    path: path.to_owned(),
                };
            }
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => {
                return StoreError::Locked {
                    path: path.to_owned(),
                };
            }
            _ => {}
        }
    }
    StoreError::Sql(err)
}

impl Store {
    /// Opens the store at `path`, creating and initializing it when the file does not exist or is
    /// empty.
    ///
    /// # Errors
    ///
    /// Fails if the file is not a pgloupe store or is newer than this build understands.
    pub fn create(path: &Path) -> Result<Store, StoreError> {
        Store::open(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            true,
        )
    }

    /// Opens a store that must already exist; never creates or initializes anything.
    ///
    /// # Errors
    ///
    /// Fails if there is no file, or it is not a usable pgloupe store.
    pub fn open_existing(path: &Path) -> Result<Store, StoreError> {
        if !path.exists() {
            return Err(StoreError::Missing {
                path: path.display().to_string(),
            });
        }
        Store::open(path, OpenFlags::SQLITE_OPEN_READ_WRITE, false)
    }

    fn open(path: &Path, flags: OpenFlags, init: bool) -> Result<Store, StoreError> {
        let name = path.display().to_string();
        if path.is_dir() {
            return Err(StoreError::IsDirectory { path: name });
        }
        let conn = Connection::open_with_flags(path, flags).map_err(|e| sql_error(e, &name))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| sql_error(e, &name))?;
        // A file that is not a database fails on the first query with SQLITE_NOTADB.
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| sql_error(e, &name))?;
        let application_id: i32 = conn
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(|e| sql_error(e, &name))?;
        let tables: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))
            .map_err(|e| sql_error(e, &name))?;
        let ours = application_id == APPLICATION_ID;
        let has_inputs: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'inputs'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| sql_error(e, &name))?;
        match (version, tables) {
            (SCHEMA_VERSION, _) if ours => {}
            (0, 0) if init => {
                let created_by = format!("pgloupe {}", env!("CARGO_PKG_VERSION"));
                conn.pragma_update(None, "journal_mode", "WAL")
                    .map_err(|e| sql_error(e, &name))?;
                conn.execute_batch(&format!(
                    "BEGIN; {} INSERT INTO meta VALUES ('created_by', '{created_by}'); \
                     PRAGMA application_id = {APPLICATION_ID}; \
                     PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;",
                    schema::SCHEMA
                ))
                .map_err(|e| sql_error(e, &name))?;
            }
            (found, _) if found > SCHEMA_VERSION && ours => {
                return Err(StoreError::Newer {
                    path: name,
                    found,
                    supported: SCHEMA_VERSION,
                });
            }
            (1, _) if has_inputs > 0 => {
                return Err(StoreError::Outdated {
                    path: name,
                    found: 1,
                    supported: SCHEMA_VERSION,
                });
            }
            _ => return Err(StoreError::NotAStore { path: name }),
        }
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(|e| sql_error(e, &name))?;
        Ok(Store { conn, path: name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pgloupe-store-{}-{name}.db", std::process::id()))
    }

    #[test]
    fn create_makes_a_current_store_and_reopens() {
        let path = temp("create");
        let _ = std::fs::remove_file(&path);
        drop(Store::create(&path).unwrap());
        drop(Store::open_existing(&path).unwrap());
        drop(Store::create(&path).unwrap());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_missing_store_is_not_created_by_open_existing() {
        let path = temp("missing");
        let _ = std::fs::remove_file(&path);
        assert!(matches!(
            Store::open_existing(&path).unwrap_err(),
            StoreError::Missing { .. }
        ));
        assert!(!path.exists());
    }

    #[test]
    fn a_text_file_is_not_a_store_and_is_left_alone() {
        let path = temp("garbage");
        let text = b"this is not sqlite, but it is long enough to be a header ......";
        std::fs::write(&path, text).unwrap();
        assert!(matches!(
            Store::create(&path).unwrap_err(),
            StoreError::NotAStore { .. }
        ));
        assert!(matches!(
            Store::open_existing(&path).unwrap_err(),
            StoreError::NotAStore { .. }
        ));
        assert_eq!(std::fs::read(&path).unwrap(), text);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_foreign_sqlite_database_is_not_a_store() {
        let path = temp("foreign");
        let _ = std::fs::remove_file(&path);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("CREATE TABLE t(x)", [])
            .unwrap();
        assert!(matches!(
            Store::create(&path).unwrap_err(),
            StoreError::NotAStore { .. }
        ));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_foreign_database_with_our_version_number_is_not_a_store() {
        let path = temp("foreign-versioned");
        let _ = std::fs::remove_file(&path);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE t(x)", []).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .unwrap();
        drop(conn);
        assert!(matches!(
            Store::create(&path).unwrap_err(),
            StoreError::NotAStore { .. }
        ));
        assert!(matches!(
            Store::open_existing(&path).unwrap_err(),
            StoreError::NotAStore { .. }
        ));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_store_from_schema_1_is_refused_with_advice() {
        let path = temp("schema1");
        let _ = std::fs::remove_file(&path);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(schema::SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);
        let err = Store::create(&path).unwrap_err();
        assert!(
            matches!(err, StoreError::Outdated { found: 1, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("ingest again"), "{err}");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_directory_is_not_a_store() {
        let dir = std::env::temp_dir().join(format!("pgloupe-store-{}-dir", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            Store::create(&dir).unwrap_err(),
            StoreError::IsDirectory { .. }
        ));
        assert!(matches!(
            Store::open_existing(&dir).unwrap_err(),
            StoreError::IsDirectory { .. }
        ));
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn a_newer_schema_is_refused_untouched() {
        let path = temp("newer");
        let _ = std::fs::remove_file(&path);
        drop(Store::create(&path).unwrap());
        rusqlite::Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        let err = Store::create(&path).unwrap_err();
        assert!(
            matches!(
                err,
                StoreError::Newer {
                    found: 99,
                    supported: SCHEMA_VERSION,
                    ..
                }
            ),
            "{err}"
        );
        assert!(matches!(
            Store::open_existing(&path).unwrap_err(),
            StoreError::Newer { .. }
        ));
        std::fs::remove_file(&path).unwrap();
    }
}
