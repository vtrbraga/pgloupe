//! Error types for reading and comparing snapshots.

use jiff::Timestamp;
use thiserror::Error;

/// A `pg_stat_statements` or `pg_stat_statements_info` CSV could not be read.
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("{file}: invalid CSV")]
    Csv {
        file: String,
        #[source]
        source: csv::Error,
    },
    #[error("{file}: could not read")]
    Io {
        file: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{file}: missing required column `{column}`")]
    MissingColumn { file: String, column: &'static str },
    #[error(
        "{file}: has `total_time` but not `total_exec_time`; pg_stat_statements before PG13 is not supported"
    )]
    LegacyColumns { file: String },
    #[error("{file}: no data rows, so the snapshot time and server version are unknown")]
    NoRows { file: String },
    #[error("{file}: line {line}: {message}")]
    BadValue {
        file: String,
        line: u64,
        message: String,
    },
    #[error("{file}: rows disagree on `{column}` ({first} vs {other})")]
    Inconsistent {
        file: String,
        column: &'static str,
        first: String,
        other: String,
    },
    #[error(
        "{file}: duplicate entry for dbid={dbid} userid={userid} queryid={queryid} toplevel={toplevel}"
    )]
    DuplicateKey {
        file: String,
        dbid: u32,
        userid: u32,
        queryid: i64,
        toplevel: bool,
    },
}

/// Two snapshots cannot be compared.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeltaError {
    #[error(
        "snapshots are from different Postgres major versions ({before} vs {after}); queryid is not stable across majors, take a new baseline"
    )]
    VersionMismatch { before: u32, after: u32 },
    #[error(
        "`after` ({after}) is not later than `before` ({before}); pass the older snapshot first"
    )]
    NotChronological { before: Timestamp, after: Timestamp },
    #[error("the info files do not match the snapshots: {reason}")]
    InconsistentInfo { reason: String },
    #[error(
        "the snapshots were exported with different columns (`{column}` is in only one); export both with the same query"
    )]
    ColumnMismatch { column: &'static str },
}
