//! What the server says happened, read before and after a run.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::Context as _;
use postgres::Client;
use serde::{Deserialize, Serialize};

use crate::session::query_scalar;

/// File name inside a run directory.
pub const FILE_NAME: &str = "bench-truth.json";

/// Server counters, summed over all databases.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    /// `xact_commit`.
    pub commits: i64,
    /// `xact_rollback`.
    pub rollbacks: i64,
    /// `deadlocks`.
    pub deadlocks: i64,
    /// `temp_files`.
    pub temp_files: i64,
    /// `temp_bytes`.
    pub temp_bytes: i64,
    /// Timed plus requested checkpoints.
    pub checkpoints: i64,
    /// `pg_stat_statements_info.dealloc`.
    pub dealloc: i64,
}

impl Counters {
    /// Field-wise `self - start`.
    #[must_use]
    pub const fn minus(self, start: Self) -> Self {
        Self {
            commits: self.commits - start.commits,
            rollbacks: self.rollbacks - start.rollbacks,
            deadlocks: self.deadlocks - start.deadlocks,
            temp_files: self.temp_files - start.temp_files,
            temp_bytes: self.temp_bytes - start.temp_bytes,
            checkpoints: self.checkpoints - start.checkpoints,
            dealloc: self.dealloc - start.dealloc,
        }
    }
}

/// SQL for the checkpoint count; the view changed in 17.
#[must_use]
pub const fn checkpoint_sql(pg_major: u32) -> &'static str {
    if pg_major >= 17 {
        "SELECT (num_timed + num_requested)::bigint FROM pg_stat_checkpointer"
    } else {
        "SELECT (checkpoints_timed + checkpoints_req)::bigint FROM pg_stat_bgwriter"
    }
}

fn scalar(client: &mut Client, sql: &str) -> anyhow::Result<i64> {
    query_scalar(client, sql)?
        .parse()
        .with_context(|| format!("not an integer: {sql}"))
}

/// Reads all counters.
///
/// # Errors
///
/// Fails if a query fails or returns a non-integer.
pub fn read(client: &mut Client, pg_major: u32) -> anyhow::Result<Counters> {
    let sum =
        |column: &str| format!("SELECT COALESCE(sum({column}), 0)::bigint FROM pg_stat_database");
    Ok(Counters {
        commits: scalar(client, &sum("xact_commit"))?,
        rollbacks: scalar(client, &sum("xact_rollback"))?,
        deadlocks: scalar(client, &sum("deadlocks"))?,
        temp_files: scalar(client, &sum("temp_files"))?,
        temp_bytes: scalar(client, &sum("temp_bytes"))?,
        checkpoints: scalar(client, checkpoint_sql(pg_major))?,
        dealloc: scalar(client, "SELECT dealloc FROM pg_stat_statements_info")?,
    })
}

/// Transactions one worker ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerStats {
    /// Worker name (`tpcb`, `literal`).
    pub name: String,
    /// pgbench rounds run.
    pub rounds: u32,
    /// Transactions pgbench reported over all rounds.
    pub transactions: u64,
}

/// Files and bytes of one log format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatSize {
    /// Number of files.
    pub files: u32,
    /// Total bytes.
    pub bytes: u64,
}

/// Numbers of the snapshot files `snapshot::take` wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshots {
    /// Before the workload.
    pub first: u8,
    /// Right after `pg_stat_statements_reset()`; `None` if the target was not reached before the
    /// end of the run.
    pub after_reset: Option<u8>,
    /// After the workload stopped.
    pub last: u8,
}

/// Everything `bench verify` compares pgloupe against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truth {
    /// Postgres major version.
    pub pg_major: u32,
    /// Seed (kept for the run directory name).
    pub seed: u64,
    /// pgbench scale factor.
    pub scale: u32,
    /// Requested log size in bytes.
    pub target_bytes: u64,
    /// Size of the largest format at the end.
    pub reached_bytes: u64,
    /// Per format (`log`, `csv`, `json`).
    pub formats: BTreeMap<String, FormatSize>,
    /// Wall time of the workload phase.
    pub wall_secs: u64,
    /// End counters minus start counters.
    pub delta: Counters,
    /// `dealloc` at the end (it restarts at 0 on a reset, so a delta would mislead).
    pub dealloc_at_end: i64,
    /// Per worker.
    pub workers: Vec<WorkerStats>,
    /// Snapshot numbers.
    pub snapshots: Snapshots,
}

impl Truth {
    /// Transactions all workers ran.
    #[must_use]
    pub fn transactions(&self) -> u64 {
        self.workers.iter().map(|w| w.transactions).sum()
    }

    /// Transactions of the worker called `name`, 0 if it did not run.
    #[must_use]
    pub fn transactions_of(&self, name: &str) -> u64 {
        self.workers
            .iter()
            .filter(|w| w.name == name)
            .map(|w| w.transactions)
            .sum()
    }

    /// Writes `bench-truth.json` into `dir`.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written.
    pub fn write(&self, dir: &Path) -> anyhow::Result<()> {
        let path = dir.join(FILE_NAME);
        let json = serde_json::to_string_pretty(self).context("serializing the truth file")?;
        fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
    }

    /// Reads `bench-truth.json` from `dir`.
    ///
    /// # Errors
    ///
    /// Fails if the file is missing or malformed.
    pub fn read(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join(FILE_NAME);
        let json =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&json).with_context(|| format!("parsing {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Truth {
        Truth {
            pg_major: 17,
            seed: 42,
            scale: 20,
            target_bytes: 1 << 30,
            reached_bytes: (1 << 30) + 5,
            formats: BTreeMap::from([(
                "log".to_owned(),
                FormatSize {
                    files: 4,
                    bytes: 1 << 30,
                },
            )]),
            wall_secs: 600,
            delta: Counters {
                commits: 10,
                ..Counters::default()
            },
            dealloc_at_end: 3,
            workers: vec![
                WorkerStats {
                    name: "tpcb".to_owned(),
                    rounds: 2,
                    transactions: 100,
                },
                WorkerStats {
                    name: "literal".to_owned(),
                    rounds: 2,
                    transactions: 50,
                },
            ],
            snapshots: Snapshots {
                first: 0,
                after_reset: Some(3),
                last: 5,
            },
        }
    }

    #[test]
    fn counters_subtract_field_by_field() {
        let end = Counters {
            commits: 10,
            rollbacks: 3,
            checkpoints: 7,
            ..Counters::default()
        };
        let start = Counters {
            commits: 4,
            rollbacks: 1,
            checkpoints: 2,
            ..Counters::default()
        };
        let d = end.minus(start);
        assert_eq!((d.commits, d.rollbacks, d.checkpoints), (6, 2, 5));
    }

    #[test]
    fn the_checkpoint_view_depends_on_the_version() {
        assert!(checkpoint_sql(16).contains("pg_stat_bgwriter"));
        assert!(checkpoint_sql(17).contains("pg_stat_checkpointer"));
    }

    #[test]
    fn the_truth_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("synth-truth-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let truth = sample();
        truth.write(&dir).unwrap();
        assert_eq!(Truth::read(&dir).unwrap(), truth);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn transactions_are_summed_and_looked_up_by_worker() {
        let truth = sample();
        assert_eq!(truth.transactions(), 150);
        assert_eq!(truth.transactions_of("tpcb"), 100);
        assert_eq!(truth.transactions_of("nope"), 0);
    }
}
