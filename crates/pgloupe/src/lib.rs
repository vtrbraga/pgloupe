//! pgloupe: offline analysis of Postgres logs and `pg_stat_statements` snapshots.

pub mod analyze;
pub mod cli;
pub mod collect;
pub mod combined;
pub mod error;
pub mod log;
pub mod stats;
pub mod store;

#[cfg(test)]
pub(crate) mod testutil;
