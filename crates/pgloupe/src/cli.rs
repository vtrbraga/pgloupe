//! Command-line interface.

use std::path::PathBuf;

use anyhow::bail;
use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::stats::{
    delta::{self, SortBy},
    render::{self, View},
    snapshot::{Info, Snapshot},
};

/// A close look at your Postgres logs and query stats.
#[derive(Debug, Parser)]
#[command(version, about, arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Work with `pg_stat_statements` snapshots.
    Stats {
        #[command(subcommand)]
        command: StatsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum StatsCommand {
    /// Compare two snapshot CSV exports and rank the queries that changed.
    Diff(DiffArgs),
}

#[derive(Debug, Args)]
struct DiffArgs {
    /// Older snapshot: CSV export of `pg_stat_statements`.
    before: PathBuf,
    /// Newer snapshot.
    after: PathBuf,
    /// `pg_stat_statements_info` CSV taken with the older snapshot.
    #[arg(long)]
    before_info: Option<PathBuf>,
    /// `pg_stat_statements_info` CSV taken with the newer snapshot.
    #[arg(long)]
    after_info: Option<PathBuf>,
    /// Metric that orders the table.
    #[arg(long, value_enum, default_value_t = By::Total)]
    by: By,
    /// Rows to show.
    #[arg(long, default_value_t = 20)]
    top: usize,
    /// With `--by mean`, skip queries with fewer calls than this in the window.
    #[arg(long, default_value_t = 1)]
    min_calls: u64,
    /// Show full query text in the table.
    #[arg(long)]
    wide: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum By {
    /// Δ total execution time.
    Total,
    /// Δ calls.
    Calls,
    /// Mean execution time inside the window.
    Mean,
}

impl From<By> for SortBy {
    fn from(by: By) -> Self {
        match by {
            By::Total => SortBy::Total,
            By::Calls => SortBy::Calls,
            By::Mean => SortBy::Mean,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
}

/// Runs the parsed command and returns what to print on stdout.
///
/// # Errors
///
/// Fails if an input cannot be read or parsed, or the snapshots cannot be compared.
pub fn run(cli: &Cli) -> anyhow::Result<String> {
    match &cli.command {
        Command::Stats {
            command: StatsCommand::Diff(args),
        } => stats_diff(args),
    }
}

fn stats_diff(args: &DiffArgs) -> anyhow::Result<String> {
    if args.before_info.is_some() != args.after_info.is_some() {
        bail!("--before-info and --after-info must be given together");
    }
    let mut before = Snapshot::from_path(&args.before)?;
    let mut after = Snapshot::from_path(&args.after)?;
    if let (Some(before_info), Some(after_info)) = (&args.before_info, &args.after_info) {
        before = before.with_info(Info::from_path(before_info)?);
        after = after.with_info(Info::from_path(after_info)?);
    }
    let delta = delta::diff(&before, &after)?;
    let view = View {
        by: args.by.into(),
        top: args.top,
        min_calls: args.min_calls,
        wide: args.wide,
    };
    Ok(match args.format {
        Format::Text => render::text(&delta, &view),
        Format::Json => {
            let mut out = render::json(&delta, &view)?;
            out.push('\n');
            out
        }
    })
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn diff_arguments_parse_with_defaults() {
        let cli = Cli::try_parse_from(["pgloupe", "stats", "diff", "a.csv", "b.csv"]).unwrap();
        let Command::Stats {
            command: StatsCommand::Diff(args),
        } = cli.command;
        assert_eq!(args.top, 20);
        assert_eq!(args.min_calls, 1);
        assert!(!args.wide);
        assert!(args.before_info.is_none());
    }

    #[test]
    fn unknown_sort_is_rejected() {
        Cli::try_parse_from(["pgloupe", "stats", "diff", "a", "b", "--by", "bogus"]).unwrap_err();
    }
}
