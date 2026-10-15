//! Command-line arguments.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// Synthetic Postgres corpus generator for pgloupe.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Generate a corpus into `<out>/<pg>-<seed>/`.
    Generate(GenerateArgs),
    /// Rewrite the message-translation table pgloupe reads, from the PostgreSQL images.
    ExtractLocales(ExtractLocalesArgs),
    /// Fill a log directory with a large benchmark load, or check a run against its ground truth.
    #[command(subcommand)]
    Bench(BenchCommand),
}

/// Subcommands of `bench`.
#[derive(Debug, Subcommand)]
pub enum BenchCommand {
    /// Run the load until the logs reach `--target-size`.
    Run(BenchRunArgs),
    /// Run pgloupe on a finished run and compare it with the server's own counters.
    Verify(BenchVerifyArgs),
}

/// Arguments for `bench run`.
#[derive(Debug, clap::Args)]
pub struct BenchRunArgs {
    /// Postgres major version.
    #[arg(long, value_enum, default_value = "17")]
    pub pg: PgMajor,
    /// Seed (part of the run directory name).
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    /// Stop when the smallest log format reaches this size (`512M`, `1G`, `10G`).
    #[arg(long, default_value = "1G", value_parser = checked_size)]
    pub target_size: String,
    /// Concurrent pgbench clients over all workers.
    #[arg(long, default_value_t = 16)]
    pub clients: u32,
    /// pgbench scale factor (rows in `pgbench_accounts` = 100000 × scale).
    #[arg(long, default_value_t = 20)]
    pub scale: u32,
    /// Seconds between periodic snapshots.
    #[arg(long, default_value_t = 300)]
    pub snapshot_secs: u64,
    /// Seconds between forced checkpoints.
    #[arg(long, default_value_t = 180)]
    pub checkpoint_secs: u64,
    /// Replace an existing run directory.
    #[arg(long)]
    pub force: bool,
    /// Root directory for runs.
    #[arg(long, default_value = "data/bench")]
    pub out: PathBuf,
    /// Directory for cached downloads.
    #[arg(long, default_value = "data/cache")]
    pub cache: PathBuf,
}

impl BenchRunArgs {
    /// Output directory for this run: `<out>/<pg>-<seed>-<size>`.
    #[must_use]
    pub fn run_dir(&self) -> PathBuf {
        self.out.join(format!(
            "{}-{}-{}",
            self.pg.number(),
            self.seed,
            self.target_size
        ))
    }

    /// `--target-size` in bytes.
    ///
    /// # Errors
    ///
    /// Fails if the size does not parse; clap has already checked it for parsed arguments.
    pub fn target_bytes(&self) -> anyhow::Result<u64> {
        crate::bench::size::parse_size(&self.target_size)
    }
}

/// Arguments for `bench verify`.
#[derive(Debug, clap::Args)]
pub struct BenchVerifyArgs {
    /// Run directory written by `bench run`.
    pub run_dir: PathBuf,
    /// The pgloupe binary to test.
    #[arg(long, default_value = "target/release/pgloupe")]
    pub pgloupe: PathBuf,
    /// Thread counts to time (comma-separated).
    #[arg(long, value_delimiter = ',', default_value = "1,16")]
    pub threads: Vec<u32>,
}

/// Keeps the text as typed (it names the run directory) after checking that it is a size.
fn checked_size(text: &str) -> Result<String, String> {
    crate::bench::size::parse_size(text)
        .map(|_| text.to_owned())
        .map_err(|e| format!("{e:#}"))
}

/// Arguments for `extract-locales`.
#[derive(Debug, clap::Args)]
pub struct ExtractLocalesArgs {
    /// Where to write the table.
    #[arg(long, default_value = "crates/pgloupe/src/log/locales.tsv")]
    pub out: PathBuf,
}

/// Postgres major versions the generator can target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PgMajor {
    /// Postgres 14.
    #[value(name = "14")]
    V14,
    /// Postgres 15.
    #[value(name = "15")]
    V15,
    /// Postgres 16.
    #[value(name = "16")]
    V16,
    /// Postgres 17.
    #[value(name = "17")]
    V17,
    /// Postgres 18.
    #[value(name = "18")]
    V18,
}

impl PgMajor {
    /// Major version number, e.g. `17`.
    #[must_use]
    pub fn number(self) -> u32 {
        match self {
            Self::V14 => 14,
            Self::V15 => 15,
            Self::V16 => 16,
            Self::V17 => 17,
            Self::V18 => 18,
        }
    }
}

/// Arguments for `generate`.
#[derive(Debug, clap::Args)]
pub struct GenerateArgs {
    /// Postgres major version.
    #[arg(long, value_enum, default_value = "17")]
    pub pg: PgMajor,
    /// Seed for the workload RNG.
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    /// Run only these scenarios (comma-separated names).
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// Regenerate even if a complete corpus exists.
    #[arg(long)]
    pub force: bool,
    /// Root directory for generated corpora.
    #[arg(long, default_value = "data/synth")]
    pub out: PathBuf,
    /// Directory for cached downloads.
    #[arg(long, default_value = "data/cache")]
    pub cache: PathBuf,
}

impl GenerateArgs {
    /// Output directory for this run: `<out>/<pg>-<seed>`.
    #[must_use]
    pub fn run_dir(&self) -> PathBuf {
        self.out.join(format!("{}-{}", self.pg.number(), self.seed))
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(args: &[&str]) -> GenerateArgs {
        match Cli::try_parse_from(args).unwrap().command {
            Command::Generate(generate) => generate,
            Command::ExtractLocales(_) | Command::Bench(_) => panic!("not a generate command"),
        }
    }

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn generate_defaults_to_pg17_seed42() {
        let args = parse(&["synth", "generate"]);
        assert_eq!(args.pg, PgMajor::V17);
        assert_eq!(args.seed, 42);
        assert_eq!(args.only, Vec::<String>::new());
        assert!(!args.force);
        assert_eq!(args.run_dir(), PathBuf::from("data/synth/17-42"));
    }

    fn parse_bench_run(args: &[&str]) -> BenchRunArgs {
        match Cli::try_parse_from(args).unwrap().command {
            Command::Bench(BenchCommand::Run(run)) => run,
            other => panic!("not bench run: {other:?}"),
        }
    }

    #[test]
    fn bench_run_defaults_to_one_gigabyte() {
        let run = parse_bench_run(&["synth", "bench", "run"]);
        assert_eq!(run.target_bytes().unwrap(), 1 << 30);
        assert_eq!((run.clients, run.scale), (16, 20));
        assert_eq!(run.run_dir(), PathBuf::from("data/bench/17-42-1G"));
    }

    #[test]
    fn bench_run_rejects_a_bad_size() {
        Cli::try_parse_from(["synth", "bench", "run", "--target-size", "nope"]).unwrap_err();
    }

    #[test]
    fn bench_verify_takes_the_run_directory() {
        let cli = Cli::try_parse_from(["synth", "bench", "verify", "data/bench/x"]).unwrap();
        let Command::Bench(BenchCommand::Verify(v)) = cli.command else {
            panic!("not bench verify");
        };
        assert_eq!(v.threads, [1, 16]);
        assert_eq!(v.run_dir, PathBuf::from("data/bench/x"));
    }

    #[test]
    fn only_splits_on_commas() {
        let args = parse(&["synth", "generate", "--only", "lock_wait,errors"]);
        assert_eq!(args.only, ["lock_wait", "errors"]);
    }

    #[test]
    fn rejects_unsupported_pg_version() {
        Cli::try_parse_from(["synth", "generate", "--pg", "13"]).unwrap_err();
    }
}
