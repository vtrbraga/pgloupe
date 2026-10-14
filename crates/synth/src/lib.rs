//! Synthetic Postgres log and `pg_stat_statements` corpus generator for pgloupe tests.

pub mod bootstrap;
pub mod cli;
pub mod container;
pub mod dataset;
pub mod generate;
pub mod manifest;
pub mod scenario;
pub mod session;
pub mod snapshot;
