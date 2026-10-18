//! pgloupe: offline analysis of Postgres logs and `pg_stat_statements` snapshots.
//!
//! This crate is the engine of the `pgloupe` command. It is published so the command can be
//! installed with `cargo install`; its modules carry no stability promise and may change in any
//! release.

pub mod analyze;
pub mod cli;
pub mod collect;
pub mod combined;
pub mod error;
mod hash;
pub mod log;
pub mod stats;
pub mod store;

#[cfg(test)]
pub(crate) mod testutil;
