//! Collecting what the report needs from a live database and its vendor: snapshots over SQL, log
//! files through vendor CLIs, and the loop that keeps a report fresh.

pub mod commands;
pub mod conn;
pub mod doctor;
pub mod fetch;
pub mod provider;
pub mod sql;

use thiserror::Error;

/// Why a collection step failed. Messages name the step and say what to try.
#[derive(Debug, Error)]
pub enum CollectError {
    /// The connection string could not be understood.
    #[error("connection string: {0}")]
    Dsn(String),
    /// The server could not be reached or refused the login.
    #[error("could not connect: {0}")]
    Connect(String),
    /// A query failed.
    #[error("{step}: {message}")]
    Query {
        /// What was being done.
        step: &'static str,
        /// What the server said, with secrets removed.
        message: String,
    },
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// The cause.
        #[source]
        source: std::io::Error,
    },
}
