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

/// Version written to `PRAGMA user_version`; a store with a larger number is refused.
pub const SCHEMA_VERSION: i32 = 1;

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
        let conn = Connection::open_with_flags(path, flags).map_err(|e| sql_error(e, &name))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| sql_error(e, &name))?;
        // A file that is not a database fails on the first query with SQLITE_NOTADB.
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| sql_error(e, &name))?;
        let tables: i64 = conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |row| row.get(0))
            .map_err(|e| sql_error(e, &name))?;
        match (version, tables) {
            (SCHEMA_VERSION, _) => {}
            (0, 0) if init => {
                let created_by = format!("pgloupe {}", env!("CARGO_PKG_VERSION"));
                conn.pragma_update(None, "journal_mode", "WAL")
                    .map_err(|e| sql_error(e, &name))?;
                conn.execute_batch(&format!(
                    "BEGIN; {} INSERT INTO meta VALUES ('created_by', '{created_by}'); \
                     PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;",
                    schema::V1
                ))
                .map_err(|e| sql_error(e, &name))?;
            }
            (found, _) if found > SCHEMA_VERSION => {
                return Err(StoreError::Newer {
                    path: name,
                    found,
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
    fn create_makes_a_version_1_store_and_reopens() {
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
                    supported: 1,
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
