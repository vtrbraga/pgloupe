//! Synthetic Postgres log and `pg_stat_statements` corpus generator for pgloupe tests.

pub mod bench;
pub mod bootstrap;
pub mod cli;
pub mod container;
pub mod dataset;
pub mod generate;
pub mod locales;
pub mod manifest;
pub mod rawclient;
pub mod scenario;
pub mod session;
pub mod snapshot;
