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
        let Command::Generate(generate) = Cli::try_parse_from(args).unwrap().command;
        generate
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
