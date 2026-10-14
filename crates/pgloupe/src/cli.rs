//! Command-line interface.

use std::{
    io::{BufRead, Write},
    path::PathBuf,
};

use anyhow::{Context as _, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use jiff::tz::Offset;

use crate::{
    log::{self, Format, Options, source, time::parse_offset_arg},
    stats::{
        delta::{self, SortBy},
        render::{self, View},
        snapshot::{Info, Snapshot},
    },
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
    /// Work with Postgres server logs.
    Logs {
        #[command(subcommand)]
        command: LogsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum StatsCommand {
    /// Compare two snapshot CSV exports and rank the queries that changed.
    Diff(DiffArgs),
}

#[derive(Debug, Subcommand)]
enum LogsCommand {
    /// Decode a log (csvlog, jsonlog or stderr; plain or gzip) and print each record as JSON.
    Dump(DumpArgs),
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
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
}

#[derive(Debug, Args)]
struct DumpArgs {
    /// Log file; gzip is detected automatically.
    file: PathBuf,
    /// Log format; `auto` looks at the first line.
    #[arg(long, value_enum, default_value_t = LogFormat::Auto)]
    format: LogFormat,
    /// The server's `log_line_prefix`; required for stderr logs.
    #[arg(long)]
    prefix: Option<String>,
    /// Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`.
    #[arg(long, value_parser = parse_offset_arg, allow_hyphen_values = true)]
    log_tz: Option<Offset>,
    /// Stop after this many records.
    #[arg(long)]
    limit: Option<usize>,
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
enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogFormat {
    Auto,
    Csv,
    Json,
    Stderr,
}

/// How a command ended, when it did not fail with an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok,
    /// The command ran but produced nothing usable; its message is already on `err`.
    Failed,
}

/// Runs the parsed command, writing results to `out` and diagnostics to `err`.
///
/// # Errors
///
/// Fails if an input cannot be read or parsed, or the command cannot be set up.
pub fn run(cli: &Cli, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<Exit> {
    match &cli.command {
        Command::Stats {
            command: StatsCommand::Diff(args),
        } => stats_diff(args, out),
        Command::Logs {
            command: LogsCommand::Dump(args),
        } => logs_dump(args, out, err),
    }
}

fn stats_diff(args: &DiffArgs, out: &mut dyn Write) -> anyhow::Result<Exit> {
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
    let report = match args.format {
        OutputFormat::Text => render::text(&delta, &view),
        OutputFormat::Json => {
            let mut json = render::json(&delta, &view)?;
            json.push('\n');
            json
        }
    };
    out.write_all(report.as_bytes())?;
    Ok(Exit::Ok)
}

fn logs_dump(args: &DumpArgs, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<Exit> {
    let mut reader = source::open(&args.file)
        .with_context(|| format!("{}: could not open", args.file.display()))?;
    let format = match args.format {
        LogFormat::Csv => Format::Csv,
        LogFormat::Json => Format::Json,
        LogFormat::Stderr => Format::Stderr,
        LogFormat::Auto => {
            let peek = reader
                .fill_buf()
                .with_context(|| format!("{}: could not read", args.file.display()))?;
            if peek.is_empty() {
                writeln!(err, "{}", log::DecodeStats::default())?;
                return Ok(Exit::Ok);
            }
            source::sniff(peek)
        }
    };
    let options = Options {
        prefix: args.prefix.clone(),
        log_tz: args.log_tz,
    };
    let mut decoder = log::decoder_for(format, reader, &options)?;
    let mut written = 0usize;
    let mut read_error = None;
    while args.limit.is_none_or(|limit| written < limit) {
        match decoder.next_record() {
            Ok(Some(record)) => {
                serde_json::to_writer(&mut *out, &record)?;
                out.write_all(b"\n")?;
                written += 1;
            }
            Ok(None) => break,
            Err(error) => {
                read_error = Some(error);
                break;
            }
        }
    }
    let stats = decoder.stats();
    writeln!(err, "{stats}")?;
    if let Some(error) = read_error {
        return Err(anyhow::Error::new(error).context(format!("{}", args.file.display())));
    }
    Ok(if stats.records == 0 && stats.lines > 0 {
        Exit::Failed
    } else {
        Exit::Ok
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
        } = cli.command
        else {
            panic!("expected stats diff");
        };
        assert_eq!(args.top, 20);
        assert_eq!(args.min_calls, 1);
        assert!(!args.wide);
        assert!(args.before_info.is_none());
    }

    #[test]
    fn unknown_sort_is_rejected() {
        Cli::try_parse_from(["pgloupe", "stats", "diff", "a", "b", "--by", "bogus"]).unwrap_err();
    }

    #[test]
    fn dump_arguments_parse_with_defaults() {
        let cli = Cli::try_parse_from(["pgloupe", "logs", "dump", "x.log"]).unwrap();
        let Command::Logs {
            command: LogsCommand::Dump(args),
        } = cli.command
        else {
            panic!("expected logs dump");
        };
        assert!(matches!(args.format, LogFormat::Auto));
        assert!(args.prefix.is_none());
        assert!(args.log_tz.is_none());
        assert!(args.limit.is_none());
    }

    #[test]
    fn bad_log_tz_is_rejected_at_parse_time() {
        Cli::try_parse_from(["pgloupe", "logs", "dump", "x.log", "--log-tz", "BRT"]).unwrap_err();
    }
}
