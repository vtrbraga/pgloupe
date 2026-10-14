//! Command-line interface.

use std::{
    io::{BufRead, IsTerminal as _, Write},
    path::PathBuf,
};

use anyhow::{Context as _, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use jiff::{Timestamp, tz::Offset};

use crate::{
    analyze::{
        pipeline::{self, AnalyzeError},
        render as analyze_render,
        report::{Report, ReportOptions, SortBy as ReportSortBy, all_statements},
    },
    combined::{self, LogData},
    log::{self, Format, Options, source, time::parse_offset_arg},
    stats::{
        delta::{self, SortBy},
        render::{self, View},
        snapshot::{Info, Snapshot},
    },
    store::{self, Status, Store, report as store_report},
};

/// A close look at your Postgres logs and query stats.
#[derive(Debug, Parser)]
#[command(version, about, arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    /// Checks the combinations clap's declarative rules cannot express.
    ///
    /// # Errors
    ///
    /// Returns a usage error (exit code 2 when printed with `exit`).
    pub fn validate(self) -> Result<Self, clap::Error> {
        use clap::{CommandFactory as _, error::ErrorKind};
        let fail = |kind, message: &str| Err(Cli::command().error(kind, message));
        let info_pair =
            |before: &Option<PathBuf>, after: &Option<PathBuf>| before.is_some() != after.is_some();
        let store_bucket = |bucket: i64| bucket % 60 != 0;
        match &self.command {
            Command::Stats {
                command: StatsCommand::Diff(args),
            } if info_pair(&args.before_info, &args.after_info) => fail(
                ErrorKind::MissingRequiredArgument,
                "--before-info and --after-info must be given together",
            ),
            Command::Logs {
                command: LogsCommand::Report(args),
            } if store_bucket(args.bucket) => fail(
                ErrorKind::ValueValidation,
                "--bucket must be a multiple of 1m with a store (it keeps one-minute buckets)",
            ),
            Command::Report(args) => {
                if args.files.is_empty() && args.store.is_none() && args.before.is_none() {
                    fail(
                        ErrorKind::MissingRequiredArgument,
                        "give log FILES, a --store, or snapshots (--before and --after)",
                    )
                } else if args.store.is_none() && (args.since.is_some() || args.until.is_some()) {
                    fail(
                        ErrorKind::MissingRequiredArgument,
                        "--since and --until need --store",
                    )
                } else if info_pair(&args.before_info, &args.after_info) {
                    fail(
                        ErrorKind::MissingRequiredArgument,
                        "--before-info and --after-info must be given together",
                    )
                } else if args.store.is_some() && store_bucket(args.bucket) {
                    fail(
                        ErrorKind::ValueValidation,
                        "--bucket must be a multiple of 1m with a store (it keeps one-minute buckets)",
                    )
                } else {
                    Ok(self)
                }
            }
            _ => Ok(self),
        }
    }
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
    /// One report from logs and, optionally, two `pg_stat_statements` snapshots: HTML, JSON or
    /// text, with the two joined per statement.
    Report(CombinedArgs),
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
    /// Add logs to a store; files already ingested are skipped and grown ones replace their old
    /// version.
    Ingest(IngestArgs),
    /// Report on a time window of a store.
    Report(StoreReportArgs),
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

#[derive(Debug, Args)]
struct IngestArgs {
    /// Log files (csvlog, jsonlog or stderr; gzip is detected automatically).
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// The store file; created when missing.
    #[arg(long)]
    store: PathBuf,
    /// Log format; `auto` looks at each file's first line.
    #[arg(long, value_enum, default_value_t = LogFormat::Auto)]
    format: LogFormat,
    /// The server's `log_line_prefix`; inferred when omitted.
    #[arg(long)]
    prefix: Option<String>,
    /// Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`.
    #[arg(long, value_parser = parse_offset_arg, allow_hyphen_values = true)]
    log_tz: Option<Offset>,
    /// Distinct statements kept per hour before the rest fold into one overflow row.
    #[arg(long, default_value_t = 50_000)]
    max_fingerprints: usize,
    /// Worker threads (default: all cores).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    threads: Option<u64>,
    /// Read and store files even when their content is already in the store.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct StoreReportArgs {
    /// The store file written by `logs ingest`.
    #[arg(long)]
    store: PathBuf,
    /// Start of the window (RFC 3339 or `YYYY-MM-DD`, UTC); rounded down to the hour.
    #[arg(long, value_parser = store_report::parse_time)]
    since: Option<Timestamp>,
    /// End of the window; rounded up to the hour.
    #[arg(long, value_parser = store_report::parse_time)]
    until: Option<Timestamp>,
    /// Metric that orders the statement table.
    #[arg(long, value_enum, default_value_t = AnalyzeBy::Total)]
    by: AnalyzeBy,
    /// Statements to list.
    #[arg(long, default_value_t = 20)]
    top: usize,
    /// Timeline resolution: a multiple of one minute.
    #[arg(long, default_value = "1m", value_parser = parse_bucket)]
    bucket: i64,
    /// Report format.
    #[arg(long = "output", value_enum, default_value_t = OutputFormat::Text)]
    output: OutputFormat,
}

#[derive(Debug, Args)]
struct CombinedArgs {
    /// Log files (csvlog, jsonlog or stderr; gzip is detected automatically), analyzed on the fly.
    #[arg(conflicts_with = "store")]
    files: Vec<PathBuf>,
    /// A store written by `logs ingest`, instead of log files.
    #[arg(long)]
    store: Option<PathBuf>,
    /// With `--store`: start of the log window (RFC 3339 or `YYYY-MM-DD`, UTC). Defaults to the
    /// snapshot interval; rounded down to the hour.
    #[arg(long, value_parser = store_report::parse_time)]
    since: Option<Timestamp>,
    /// With `--store`: end of the log window; rounded up to the hour.
    #[arg(long, value_parser = store_report::parse_time)]
    until: Option<Timestamp>,
    /// Older `pg_stat_statements` snapshot (CSV export).
    #[arg(long, requires = "after")]
    before: Option<PathBuf>,
    /// Newer snapshot.
    #[arg(long, requires = "before")]
    after: Option<PathBuf>,
    /// `pg_stat_statements_info` CSV taken with the older snapshot.
    #[arg(long, requires = "before")]
    before_info: Option<PathBuf>,
    /// `pg_stat_statements_info` CSV taken with the newer snapshot.
    #[arg(long, requires = "before")]
    after_info: Option<PathBuf>,
    /// Log format of `FILES`; `auto` looks at each file's first line.
    #[arg(long, value_enum, default_value_t = LogFormat::Auto, conflicts_with = "store")]
    format: LogFormat,
    /// The server's `log_line_prefix` for `FILES`; inferred when omitted.
    #[arg(long, conflicts_with = "store")]
    prefix: Option<String>,
    /// Offset for timezone abbreviations the log does not spell out, e.g. `-03` or `+05:30`.
    #[arg(long, value_parser = parse_offset_arg, allow_hyphen_values = true, conflicts_with = "store")]
    log_tz: Option<Offset>,
    /// Distinct statements kept before the rest fold into one overflow row (`FILES`).
    #[arg(long, default_value_t = 50_000, conflicts_with = "store")]
    max_fingerprints: usize,
    /// Worker threads for `FILES` (default: all cores).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..), conflicts_with = "store")]
    threads: Option<u64>,
    /// Metric that orders the statement table.
    #[arg(long, value_enum, default_value_t = AnalyzeBy::Total)]
    by: AnalyzeBy,
    /// Statements to list.
    #[arg(long, default_value_t = 20)]
    top: usize,
    /// Timeline resolution: `30s`, `1m`, `1h` (a multiple of one minute with `--store`).
    #[arg(long, default_value = "1m", value_parser = parse_bucket)]
    bucket: i64,
    /// Report format.
    #[arg(long = "output", value_enum, default_value_t = ReportFormat::Text)]
    output: ReportFormat,
    /// Write the report here instead of to standard output.
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ReportFormat {
    Text,
    Json,
    /// One self-contained HTML file.
    Html,
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
        Command::Logs {
            command: LogsCommand::Ingest(args),
        } => logs_ingest(args, out, err),
        Command::Logs {
            command: LogsCommand::Report(args),
        } => logs_report(args, out),
        Command::Report(args) => report(args, out, err),
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

fn render_report(
    report: &Report,
    output: OutputFormat,
    elapsed: std::time::Duration,
) -> anyhow::Result<String> {
    Ok(match output {
        OutputFormat::Text => analyze_render::text(report, Some(elapsed)),
        OutputFormat::Json => {
            let mut json = report.to_json()?;
            json.push('\n');
            json
        }
    })
}

fn logs_ingest(
    args: &IngestArgs,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<Exit> {
    let mut store = Store::create(&args.store)?;
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
        bucket_secs: 60,
        threads: args.threads.map(usize::try_from).transpose()?,
        batch_size: pipeline::Options::new().batch_size,
        hash_marks: None,
    };
    let mut failed = false;
    for outcome in store::ingest(&mut store, &args.files, &options, args.force) {
        let path = outcome.path.display();
        match &outcome.status {
            Status::Ingested => writeln!(
                out,
                "ingested   {path}  ({} records{})",
                outcome.records,
                skipped_note(outcome.skipped)
            ),
            Status::Unchanged => writeln!(out, "unchanged  {path}"),
            Status::Replaced(earlier) => writeln!(
                out,
                "replaced   {path}  ({} records{}, replaces {earlier} earlier)",
                outcome.records,
                skipped_note(outcome.skipped)
            ),
            Status::Failed(reason) => {
                failed = true;
                writeln!(out, "failed     {path}: {reason}")
            }
        }
        .context("writing output")?;
        for warning in outcome.diag.iter().flat_map(|diag| &diag.warnings) {
            let _ = writeln!(err, "warning: {path}: {warning}");
        }
    }
    Ok(if failed { Exit::Failed } else { Exit::Ok })
}

/// Prints what `AnalyzeError::PrefixNotInferred` knows: the closest candidate and unmatched lines.
fn explain_prefix_failure(error: &AnalyzeError, err: &mut dyn Write) {
    if let AnalyzeError::PrefixNotInferred { inference, .. } = error {
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
}

fn load_delta(args: &CombinedArgs) -> anyhow::Result<Option<delta::Delta>> {
    let (Some(before), Some(after)) = (&args.before, &args.after) else {
        return Ok(None);
    };
    if args.before_info.is_some() != args.after_info.is_some() {
        bail!("--before-info and --after-info must be given together");
    }
    let mut before = Snapshot::from_path(before)?;
    let mut after = Snapshot::from_path(after)?;
    if let (Some(before_info), Some(after_info)) = (&args.before_info, &args.after_info) {
        before = before.with_info(Info::from_path(before_info)?);
        after = after.with_info(Info::from_path(after_info)?);
    }
    Ok(Some(delta::diff(&before, &after)?))
}

fn report(args: &CombinedArgs, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<Exit> {
    let delta = load_delta(args)?;
    let options = ReportOptions {
        by: args.by.into(),
        top: args.top,
    };
    let mut failed = false;
    let logs = if let Some(store_path) = &args.store {
        let store = Store::open_existing(store_path)?;
        let since = args.since.or(delta.as_ref().map(|d| d.before_at));
        let until = args.until.or(delta.as_ref().map(|d| d.after_at));
        let (report, acc) =
            store_report::build_parts(&store, store_path, since, until, &options, args.bucket)?;
        Some(LogData {
            rows: all_statements(&acc),
            inputs: report.files.len(),
            report,
            source: "store",
            store: Some(store_path.display().to_string()),
        })
    } else if args.files.is_empty() {
        None
    } else {
        let pipeline_options = pipeline::Options {
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
            hash_marks: None,
        };
        let output = pipeline::run(&args.files, &pipeline_options).inspect_err(|error| {
            explain_prefix_failure(error, err);
        })?;
        let skipped: u64 = output.files.iter().map(|file| file.stats.skipped).sum();
        failed = output.acc.totals.records == 0 && skipped > 0;
        Some(LogData {
            report: Report::build(&output, &options),
            rows: all_statements(&output.acc),
            source: "files",
            inputs: args.files.len(),
            store: None,
        })
    };
    let combined = combined::build(logs, delta.as_ref(), &options);
    let rendered = match args.output {
        ReportFormat::Text => combined::text(&combined),
        ReportFormat::Json => {
            let mut json = combined.to_json()?;
            json.push('\n');
            json
        }
        ReportFormat::Html => combined::html(&combined)?,
    };
    if let Some(path) = &args.out {
        std::fs::write(path, rendered).with_context(|| format!("writing {}", path.display()))?;
    } else {
        if matches!(args.output, ReportFormat::Html) && std::io::stdout().is_terminal() {
            let _ = writeln!(err, "note: HTML on a terminal; use --out report.html");
        }
        out.write_all(rendered.as_bytes())
            .context("writing output")?;
    }
    Ok(if failed { Exit::Failed } else { Exit::Ok })
}

/// `, N skipped` when some lines could not be decoded.
fn skipped_note(skipped: u64) -> String {
    if skipped == 0 {
        String::new()
    } else {
        format!(", {skipped} skipped")
    }
}

fn logs_report(args: &StoreReportArgs, out: &mut dyn Write) -> anyhow::Result<Exit> {
    let started = std::time::Instant::now();
    let store = Store::open_existing(&args.store)?;
    let report = store_report::build(
        &store,
        &args.store,
        args.since,
        args.until,
        &ReportOptions {
            by: args.by.into(),
            top: args.top,
        },
        args.bucket,
    )?;
    let rendered = render_report(&report, args.output, started.elapsed())?;
    out.write_all(rendered.as_bytes())
        .context("writing output")?;
    Ok(Exit::Ok)
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
        hash_marks: None,
    };
    let output = match pipeline::run(&args.files, &options) {
        Ok(output) => output,
        Err(error) => {
            explain_prefix_failure(&error, err);
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
    let rendered = render_report(&report, args.output, started.elapsed())?;
    out.write_all(rendered.as_bytes())
        .context("writing output")?;
    // Non-empty input that decoded to nothing: the report above says why, but it is a failure.
    let skipped: u64 = output.files.iter().map(|file| file.stats.skipped).sum();
    Ok(if output.acc.totals.records == 0 && skipped > 0 {
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

    #[test]
    fn ingest_requires_a_store_and_files() {
        Cli::try_parse_from(["pgloupe", "logs", "ingest", "a.log"]).unwrap_err();
        Cli::try_parse_from(["pgloupe", "logs", "ingest", "--store", "s.db"]).unwrap_err();
        let cli = Cli::try_parse_from([
            "pgloupe", "logs", "ingest", "--store", "s.db", "a.log", "b.log", "--force",
        ])
        .unwrap();
        let Command::Logs {
            command: LogsCommand::Ingest(args),
        } = cli.command
        else {
            panic!("expected logs ingest");
        };
        assert_eq!(args.store, PathBuf::from("s.db"));
        assert_eq!(args.files.len(), 2);
        assert!(args.force);
        assert_eq!(args.max_fingerprints, 50_000);
    }

    #[test]
    fn report_parses_times_and_defaults() {
        let cli = Cli::try_parse_from([
            "pgloupe",
            "logs",
            "report",
            "--store",
            "s.db",
            "--since",
            "2026-10-14",
            "--until",
            "2026-10-15T00:00:00Z",
        ])
        .unwrap();
        let Command::Logs {
            command: LogsCommand::Report(args),
        } = cli.command
        else {
            panic!("expected logs report");
        };
        assert_eq!(args.since.unwrap().to_string(), "2026-10-14T00:00:00Z");
        assert_eq!(args.until.unwrap().to_string(), "2026-10-15T00:00:00Z");
        assert_eq!(args.bucket, 60);
        assert_eq!(args.top, 20);
        Cli::try_parse_from(["pgloupe", "logs", "report"]).unwrap_err();
        Cli::try_parse_from([
            "pgloupe", "logs", "report", "--store", "s", "--since", "never",
        ])
        .unwrap_err();
    }

    #[test]
    fn report_needs_a_source_and_valid_pairs() {
        let usage = |args: &[&str]| {
            Cli::try_parse_from(args)
                .and_then(Cli::validate)
                .unwrap_err()
                .to_string()
        };
        let none = usage(&["pgloupe", "report"]);
        assert!(
            none.contains("--store") && none.contains("--before"),
            "{none}"
        );
        let alone = usage(&["pgloupe", "report", "--since", "2026-10-14"]);
        assert!(!alone.contains("Usage: pgloupe report --store"), "{alone}");
        Cli::try_parse_from(["pgloupe", "report", "a.log", "--store", "s.db"]).unwrap_err();
        Cli::try_parse_from(["pgloupe", "report", "--before", "b.csv"]).unwrap_err();
        Cli::try_parse_from(["pgloupe", "report", "a.log", "--since", "2026-10-14"])
            .and_then(Cli::validate)
            .unwrap_err();
        Cli::try_parse_from([
            "pgloupe",
            "report",
            "--store",
            "s.db",
            "--since",
            "2026-10-14",
        ])
        .unwrap();
        Cli::try_parse_from(["pgloupe", "report", "--before", "b.csv", "--after", "a.csv"])
            .unwrap();
    }

    #[test]
    fn mismatched_info_files_and_store_buckets_are_usage_errors() {
        let kind = |args: &[&str]| {
            Cli::try_parse_from(args)
                .and_then(Cli::validate)
                .unwrap_err()
                .exit_code()
        };
        assert_eq!(
            kind(&[
                "pgloupe",
                "stats",
                "diff",
                "a.csv",
                "b.csv",
                "--before-info",
                "i.csv"
            ]),
            2
        );
        assert_eq!(
            kind(&[
                "pgloupe",
                "report",
                "--before",
                "b",
                "--after",
                "a",
                "--before-info",
                "i"
            ]),
            2
        );
        assert_eq!(
            kind(&["pgloupe", "report", "--store", "s.db", "--bucket", "30s"]),
            2
        );
        assert_eq!(
            kind(&[
                "pgloupe", "logs", "report", "--store", "s.db", "--bucket", "90s"
            ]),
            2
        );
        Cli::try_parse_from(["pgloupe", "report", "--store", "s.db", "--bucket", "5m"])
            .and_then(Cli::validate)
            .unwrap();
    }

    #[test]
    fn combined_report_defaults() {
        let cli = Cli::try_parse_from(["pgloupe", "report", "a.log"]).unwrap();
        let Command::Report(args) = cli.command else {
            panic!("expected report");
        };
        assert_eq!(args.top, 20);
        assert_eq!(args.bucket, 60);
        assert!(matches!(args.output, ReportFormat::Text));
        assert!(args.out.is_none());
    }
}
