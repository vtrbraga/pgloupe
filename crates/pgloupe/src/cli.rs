//! Command-line interface.

use std::{
    io::{BufRead, Write},
    path::PathBuf,
};

use anyhow::{Context as _, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use jiff::tz::Offset;

use crate::{
    analyze::{
        pipeline::{self, AnalyzeError},
        render as analyze_render,
        report::{Report, ReportOptions, SortBy as ReportSortBy},
    },
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
    /// Summarize one or more logs: statements with percentiles, errors, locks, temp files,
    /// connections, checkpoints and autovacuum.
    Analyze(AnalyzeArgs),
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
struct AnalyzeArgs {
    /// Log files (csvlog, jsonlog or stderr; gzip is detected automatically).
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Log format; `auto` looks at each file's first line.
    #[arg(long, value_enum, default_value_t = LogFormat::Auto)]
    format: LogFormat,
    /// The server's `log_line_prefix`; inferred when omitted.
    #[arg(long)]
    prefix: Option<String>,
    /// Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`.
    #[arg(long, value_parser = parse_offset_arg, allow_hyphen_values = true)]
    log_tz: Option<Offset>,
    /// Metric that orders the statement table.
    #[arg(long, value_enum, default_value_t = AnalyzeBy::Total)]
    by: AnalyzeBy,
    /// Statements to list.
    #[arg(long, default_value_t = 20)]
    top: usize,
    /// Timeline resolution: `30s`, `1m`, `1h`.
    #[arg(long, default_value = "1m", value_parser = parse_bucket)]
    bucket: i64,
    /// Distinct statements kept before the rest fold into one overflow row.
    #[arg(long, default_value_t = 50_000)]
    max_fingerprints: usize,
    /// Worker threads (default: all cores).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    threads: Option<u64>,
    /// Report format.
    #[arg(long = "output", value_enum, default_value_t = OutputFormat::Text)]
    output: OutputFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AnalyzeBy {
    /// Total execution time.
    Total,
    /// Number of calls.
    Calls,
    /// 95th percentile of the duration.
    P95,
    /// Slowest single call.
    Max,
}

impl From<AnalyzeBy> for ReportSortBy {
    fn from(by: AnalyzeBy) -> Self {
        match by {
            AnalyzeBy::Total => ReportSortBy::Total,
            AnalyzeBy::Calls => ReportSortBy::Calls,
            AnalyzeBy::P95 => ReportSortBy::P95,
            AnalyzeBy::Max => ReportSortBy::Max,
        }
    }
}

/// `30s`, `5m` or `2h` as seconds (at least one second).
fn parse_bucket(text: &str) -> Result<i64, String> {
    let bad = || format!("{text:?} is not a duration like 30s, 1m or 1h");
    let unit = text.chars().last().ok_or_else(bad)?;
    let number = &text[..text.len() - unit.len_utf8()];
    let n: i64 = number.parse().map_err(|_err| bad())?;
    let secs = match unit {
        's' => n,
        'm' => n.saturating_mul(60),
        'h' => n.saturating_mul(3_600),
        _ => return Err(bad()),
    };
    if secs < 1 {
        return Err("the bucket must be at least one second".to_owned());
    }
    Ok(secs)
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
        Command::Logs {
            command: LogsCommand::Analyze(args),
        } => logs_analyze(args, out, err),
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
        // A prefix only makes sense for stderr logs.
        LogFormat::Auto if args.prefix.is_some() => Format::Stderr,
        LogFormat::Auto => {
            let peek = reader
                .fill_buf()
                .with_context(|| format!("{}: could not read", args.file.display()))?;
            if peek.is_empty() {
                let _ = writeln!(err, "{}", log::DecodeStats::default());
                return Ok(Exit::Ok);
            }
            if peek.iter().all(u8::is_ascii_whitespace) {
                // Nothing but blank lines: any decoder reads it as an empty log.
                Format::Json
            } else if peek.contains(&0) {
                bail!(
                    "{}: looks like binary data, not a log; pass --format to force a decoder",
                    args.file.display()
                );
            } else {
                source::sniff(peek)
            }
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
                serde_json::to_writer(&mut *out, &record).context("writing output")?;
                out.write_all(b"\n").context("writing output")?;
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
    let _ = writeln!(err, "{stats}");
    if let Some(error) = read_error {
        return Err(anyhow::Error::new(error).context(format!("{}", args.file.display())));
    }
    Ok(if stats.records == 0 && stats.skipped > 0 {
        Exit::Failed
    } else {
        Exit::Ok
    })
}

fn logs_analyze(
    args: &AnalyzeArgs,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<Exit> {
    let started = std::time::Instant::now();
    let options = pipeline::Options {
        format: match args.format {
            LogFormat::Auto => None,
            LogFormat::Csv => Some(Format::Csv),
            LogFormat::Json => Some(Format::Json),
            LogFormat::Stderr => Some(Format::Stderr),
        },
        prefix: args.prefix.clone(),
        log_tz: args.log_tz,
        max_fingerprints: args.max_fingerprints,
        bucket_secs: args.bucket,
        threads: args.threads.map(usize::try_from).transpose()?,
        batch_size: pipeline::Options::new().batch_size,
    };
    let output = match pipeline::run(&args.files, &options) {
        Ok(output) => output,
        Err(error) => {
            if let AnalyzeError::PrefixNotInferred { inference, .. } = &error {
                if let Some((candidate, matched)) = &inference.best {
                    let _ = writeln!(
                        err,
                        "closest candidate: {candidate} ({matched} of {} sample lines)",
                        inference.sampled
                    );
                }
                let _ = writeln!(err, "unmatched lines start with:");
                for example in &inference.unmatched {
                    let _ = writeln!(err, "  {example}");
                }
            }
            return Err(error.into());
        }
    };
    let report = Report::build(
        &output,
        &ReportOptions {
            by: args.by.into(),
            top: args.top,
        },
    );
    let rendered = match args.output {
        OutputFormat::Text => analyze_render::text(&report, Some(started.elapsed())),
        OutputFormat::Json => {
            let mut json = report.to_json()?;
            json.push('\n');
            json
        }
    };
    out.write_all(rendered.as_bytes())
        .context("writing output")?;
    Ok(Exit::Ok)
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

    #[test]
    fn buckets_parse() {
        assert_eq!(parse_bucket("30s"), Ok(30));
        assert_eq!(parse_bucket("1m"), Ok(60));
        assert_eq!(parse_bucket("2h"), Ok(7_200));
        for bad in ["0s", "5", "m", "", "1d", "-1m"] {
            parse_bucket(bad).unwrap_err();
        }
    }

    #[test]
    fn analyze_arguments_parse_with_defaults() {
        let cli = Cli::try_parse_from(["pgloupe", "logs", "analyze", "a.log", "b.log"]).unwrap();
        let Command::Logs {
            command: LogsCommand::Analyze(args),
        } = cli.command
        else {
            panic!("expected logs analyze");
        };
        assert_eq!(args.files.len(), 2);
        assert_eq!(args.top, 20);
        assert_eq!(args.bucket, 60);
        assert_eq!(args.max_fingerprints, 50_000);
        assert!(args.threads.is_none());
    }
}
