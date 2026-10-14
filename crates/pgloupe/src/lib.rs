//! pgloupe: offline analysis of Postgres logs and `pg_stat_statements` snapshots.

pub mod error;
pub mod stats;

#[cfg(test)]
pub(crate) mod testutil;
