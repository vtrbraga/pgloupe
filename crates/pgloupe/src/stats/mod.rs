//! `pg_stat_statements` and `pg_stat_database` snapshots and the deltas between them.

pub mod database;
pub mod delta;
pub mod render;
pub mod snapshot;
