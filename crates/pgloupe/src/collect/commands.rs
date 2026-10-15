//! The command-line side of collection: arguments and handlers for `snap` and `doctor`.

use std::{io::Write, path::PathBuf};

use anyhow::Context as _;
use clap::{Args, ValueEnum};

use super::{
    conn::ConnSpec,
    doctor::{self, Check},
    fetch::DirSink,
    provider::{LogProvider, Request, aws::AwsRds, sql::SqlLogdir},
    sql,
};
use crate::cli::Exit;

/// How to reach the server.
#[derive(Debug, Args)]
pub struct ConnArgs {
    /// Connection string: `postgres://user@host:5432/db` or `key=value` pairs. The password comes
    /// from the string, `PGPASSWORD` (or `--password-env`), or `~/.pgpass`; prefer those to
    /// putting it on the command line.
    #[arg(long, env = "PGLOUPE_DSN", hide_env_values = true)]
    conn: String,
    /// Environment variable that holds the password (default `PGPASSWORD`).
    #[arg(long)]
    password_env: Option<String>,
    /// CA certificate (PEM) to trust; turns on full certificate and host name checking. For RDS,
    /// the regional bundle from AWS.
    #[arg(long)]
    sslrootcert: Option<PathBuf>,
}

impl ConnArgs {
    fn spec(&self) -> anyhow::Result<ConnSpec> {
        Ok(ConnSpec::parse(
            &self.conn,
            self.password_env.as_deref(),
            self.sslrootcert.clone(),
        )?)
    }
}

/// `snap`: write one snapshot of `pg_stat_statements`.
#[derive(Debug, Args)]
pub struct SnapArgs {
    #[command(flatten)]
    conn: ConnArgs,
    /// Directory the snapshot files go to (created if missing).
    #[arg(long)]
    out: PathBuf,
}

/// Vendors whose advice differs.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Vendor {
    /// Amazon RDS and Aurora PostgreSQL: settings live in the parameter group.
    Rds,
}

/// `doctor`: what the server and role lack for a useful report.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[command(flatten)]
    conn: ConnArgs,
    /// Say where settings live for this vendor.
    #[arg(long, value_enum)]
    vendor: Option<Vendor>,
    /// Output format.
    #[arg(long = "output", value_enum, default_value_t = DoctorFormat::Text)]
    output: DoctorFormat,
}

/// Output formats of `doctor`.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DoctorFormat {
    /// Human-readable.
    Text,
    /// One JSON document.
    Json,
}

/// Runs `snap`.
///
/// # Errors
///
/// Fails when the server cannot be reached, `pg_stat_statements` is unusable or a file cannot be
/// written.
pub fn snap(args: &SnapArgs, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<Exit> {
    let spec = args.conn.spec()?;
    writeln!(err, "connecting to {}", spec.describe())?;
    let mut client = spec.connect()?;
    let snapshot = sql::take(&mut client, &args.out)?;
    writeln!(out, "wrote {}", snapshot.pgss.display())?;
    match &snapshot.info {
        Some(info) => writeln!(out, "wrote {}", info.display())?,
        None => writeln!(
            err,
            "note: this server has no pg_stat_statements_info (Postgres 13 and older), so a reset between snapshots cannot be proven"
        )?,
    }
    if snapshot.unreadable_text > 0 {
        writeln!(
            err,
            "warning: {} entries have no readable query text (<insufficient privilege>): GRANT pg_read_all_stats to the connecting role",
            snapshot.unreadable_text
        )?;
    }
    let earlier = previous_snapshot(&args.out, &snapshot.pgss)?;
    if let Some(before) = earlier {
        writeln!(
            out,
            "compare with the previous one: pgloupe stats diff {} {}",
            before.display(),
            snapshot.pgss.display()
        )?;
    }
    Ok(Exit::Ok)
}

/// The newest snapshot in `dir` older than `current`.
fn previous_snapshot(
    dir: &std::path::Path,
    current: &std::path::Path,
) -> anyhow::Result<Option<PathBuf>> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("pgss_2")
                    && std::path::Path::new(n)
                        .extension()
                        .is_some_and(|e| e == "csv")
            })
        })
        .collect();
    names.sort();
    Ok(names.into_iter().rev().find(|p| p.as_path() < current))
}

/// Runs `doctor`.
///
/// # Errors
///
/// Fails when the server cannot be reached or a catalog query fails.
pub fn doctor(args: &DoctorArgs, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<Exit> {
    let spec = args.conn.spec()?;
    writeln!(err, "connecting to {}", spec.describe())?;
    let mut client = spec.connect()?;
    let facts = doctor::gather(&mut client)?;
    let vendor = args.vendor.map(|Vendor::Rds| "rds");
    let found = doctor::checks(&facts, vendor);
    match args.output {
        DoctorFormat::Json => {
            let doc = serde_json::json!({
                "kind": "pgloupe.doctor",
                "server_version_num": facts.version,
                "role": facts.role,
                "checks": found,
            });
            writeln!(out, "{}", serde_json::to_string_pretty(&doc)?)?;
        }
        DoctorFormat::Text => write_text(out, &facts, &found)?,
    }
    Ok(Exit::Ok)
}

fn write_text(out: &mut dyn Write, facts: &doctor::Facts, found: &[Check]) -> std::io::Result<()> {
    writeln!(out, "server {} as {}", facts.version, facts.role)?;
    if found.is_empty() {
        writeln!(
            out,
            "nothing to fix: the server logs and collects what the report needs"
        )?;
        return Ok(());
    }
    for check in found {
        writeln!(
            out,
            "[{}] {}: {}",
            check.severity, check.code, check.message
        )?;
    }
    Ok(())
}

/// Where logs come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderKind {
    /// Amazon RDS and Aurora PostgreSQL, through the `aws` CLI.
    #[value(name = "aws-rds")]
    AwsRds,
    /// The server's own log directory over SQL (EXPERIMENTAL).
    Sql,
}

/// Which instance to read logs from, and how to reach the vendor.
#[derive(Debug, Args)]
pub struct TargetArgs {
    /// Log source.
    #[arg(long, value_enum)]
    provider: ProviderKind,
    /// The instance identifier (`aws-rds`).
    #[arg(long)]
    instance: Option<String>,
    /// AWS region (`aws-rds`); default from the AWS CLI configuration.
    #[arg(long)]
    region: Option<String>,
    /// AWS CLI profile (`aws-rds`).
    #[arg(long)]
    profile: Option<String>,
    /// Path of the `aws` executable.
    #[arg(long, default_value = "aws")]
    aws_bin: PathBuf,
}

/// `logs fetch`: download a time range of log files.
#[derive(Debug, Args)]
pub struct FetchArgs {
    #[command(flatten)]
    target: TargetArgs,
    /// Connection for `--provider sql`.
    #[arg(long, env = "PGLOUPE_DSN", hide_env_values = true)]
    conn: Option<String>,
    /// Environment variable that holds the password (default `PGPASSWORD`).
    #[arg(long)]
    password_env: Option<String>,
    /// CA certificate (PEM) to trust.
    #[arg(long)]
    sslrootcert: Option<PathBuf>,
    /// Start: `30m`, `24h`, `7d`, `2w` back from now, or an RFC 3339 time or `YYYY-MM-DD` (UTC).
    #[arg(long, default_value = "24h")]
    since: String,
    /// Directory the files go to (created if missing); a rerun continues where this stopped.
    #[arg(long)]
    out: PathBuf,
}

/// A point in time: a span back from `now` (`24h`) or an absolute time.
///
/// # Errors
///
/// Fails if `text` is neither.
pub fn parse_since(text: &str, now: jiff::Timestamp) -> Result<jiff::Timestamp, String> {
    let text = text.trim();
    if let Some(unit) = text.chars().last()
        && let Ok(n) = text[..text.len() - unit.len_utf8()].parse::<i64>()
    {
        let seconds = match unit {
            's' => Some(1),
            'm' => Some(60),
            'h' => Some(3_600),
            'd' => Some(86_400),
            'w' => Some(604_800),
            _ => None,
        };
        if let Some(per) = seconds {
            return now
                .checked_sub(jiff::SignedDuration::from_secs(n.saturating_mul(per)))
                .map_err(|e| e.to_string());
        }
    }
    crate::store::report::parse_time(text).map_err(|_unparsed| {
        format!("{text:?} is not a span like 24h or 7d, nor an RFC 3339 time or a date")
    })
}

/// Builds the provider named by `args`.
///
/// # Errors
///
/// Fails when a required option for that provider is missing.
pub fn build_provider(
    args: &TargetArgs,
    conn: Option<ConnSpec>,
) -> anyhow::Result<Box<dyn LogProvider>> {
    match args.provider {
        ProviderKind::AwsRds => {
            let instance = args
                .instance
                .clone()
                .context("--provider aws-rds needs --instance (the DB instance identifier)")?;
            Ok(Box::new(AwsRds::new(
                args.aws_bin.clone(),
                instance,
                args.region.clone(),
                args.profile.clone(),
            )))
        }
        ProviderKind::Sql => Ok(Box::new(SqlLogdir::new(
            conn.context("--provider sql needs --conn")?,
        ))),
    }
}

/// Runs `logs fetch`.
///
/// # Errors
///
/// Fails when the provider cannot be set up, its tool is missing or not signed in, or a step of
/// the download fails (what was fetched stays).
pub fn fetch(args: &FetchArgs, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<Exit> {
    let conn = args
        .conn
        .as_deref()
        .map(|dsn| ConnSpec::parse(dsn, args.password_env.as_deref(), args.sslrootcert.clone()))
        .transpose()?;
    let provider = build_provider(&args.target, conn)?;
    let since = parse_since(&args.since, jiff::Timestamp::now()).map_err(anyhow::Error::msg)?;
    fetch_with(provider.as_ref(), &args.out, since, out, err)
}

/// Checks `provider`, then fetches into `dir`, reporting on `out` and `err`.
///
/// # Errors
///
/// See [`fetch`].
pub fn fetch_with(
    provider: &dyn LogProvider,
    dir: &std::path::Path,
    since: jiff::Timestamp,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<Exit> {
    if provider.experimental() {
        writeln!(
            err,
            "warning: provider {} is experimental (not yet confirmed against the real service)",
            provider.name()
        )?;
    }
    provider.check()?;
    let mut sink = DirSink::open(dir, provider.name())?;
    let mut state = sink.state();
    let request = Request { since, until: None };
    let summary = provider.fetch(&request, &mut sink, &mut state)?;
    for name in &summary.restarted {
        writeln!(
            err,
            "note: {name} is smaller than before (rotated?); fetched again from the start"
        )?;
    }
    writeln!(
        out,
        "fetched {} file(s), {} bytes into {}",
        summary.files.len(),
        summary.bytes,
        dir.display()
    )?;
    if !summary.files.is_empty() {
        writeln!(
            out,
            "analyze them: pgloupe logs analyze {}/*",
            dir.display()
        )?;
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> jiff::Timestamp {
        "2026-10-14T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn spans_and_absolute_times_are_understood() {
        let at = |text| parse_since(text, now()).unwrap().to_string();
        assert_eq!(at("30m"), "2026-10-14T11:30:00Z");
        assert_eq!(at("24h"), "2026-10-13T12:00:00Z");
        assert_eq!(at("7d"), "2026-10-07T12:00:00Z");
        assert_eq!(at("2w"), "2026-09-30T12:00:00Z");
        assert_eq!(at("2026-10-01"), "2026-10-01T00:00:00Z");
        assert_eq!(at("2026-10-01T05:00:00Z"), "2026-10-01T05:00:00Z");
        parse_since("soon", now()).unwrap_err();
        parse_since("12x", now()).unwrap_err();
    }
}
