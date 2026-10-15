//! The command-line side of collection: arguments and handlers for `snap` and `doctor`.

use std::{io::Write, path::PathBuf};

use anyhow::Context as _;
use clap::{Args, ValueEnum};

use super::{
    conn::ConnSpec,
    doctor::{self, Check},
    fetch::DirSink,
    provider::{
        LogProvider, Request,
        aiven::Aiven,
        aws::{AwsCli, AwsRds},
        azure::AzureFlexible,
        cloudwatch::AwsCloudWatch,
        gcp::GcpCloudSql,
        heroku::Heroku,
        sql::SqlLogdir,
    },
    serve, sql,
    watch::{self, Built, Cycle, CycleError, Live, Settings, SystemClock},
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
    if let Some(settings) = &snapshot.settings {
        writeln!(out, "wrote {}", settings.display())?;
    }
    if let Some(database) = &snapshot.database {
        writeln!(out, "wrote {}", database.display())?;
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

/// Where logs come from. Only `aws-rds` is confirmed against a real service; the others are
/// experimental and say so every time they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderKind {
    /// Amazon RDS and Aurora PostgreSQL, through the `aws` CLI.
    #[value(name = "aws-rds")]
    AwsRds,
    /// RDS/Aurora logs exported to `CloudWatch` Logs (EXPERIMENTAL).
    #[value(name = "aws-cloudwatch")]
    AwsCloudwatch,
    /// Azure Database for PostgreSQL Flexible Server, through `az` (EXPERIMENTAL).
    #[value(name = "azure-flexible")]
    AzureFlexible,
    /// Google Cloud SQL for PostgreSQL, through `gcloud` (EXPERIMENTAL).
    #[value(name = "gcp-cloudsql")]
    GcpCloudsql,
    /// Aiven for PostgreSQL, through `avn` (EXPERIMENTAL).
    Aiven,
    /// Heroku Postgres, through `heroku`: recent lines only (EXPERIMENTAL).
    Heroku,
    /// The server's own log directory over SQL (EXPERIMENTAL).
    Sql,
}

/// Which instance to read logs from, and how to reach the vendor.
#[derive(Debug, Args)]
pub struct TargetArgs {
    /// Log source.
    #[arg(long, value_enum)]
    provider: ProviderKind,
    /// The instance: RDS instance identifier, Azure server name, Cloud SQL instance, Aiven
    /// service, or Heroku app.
    #[arg(long)]
    instance: Option<String>,
    /// AWS region; default from the AWS CLI configuration.
    #[arg(long)]
    region: Option<String>,
    /// AWS CLI profile.
    #[arg(long)]
    profile: Option<String>,
    /// Google Cloud project (`gcp-cloudsql`) or Aiven project (`aiven`).
    #[arg(long)]
    project: Option<String>,
    /// Azure resource group (`azure-flexible`).
    #[arg(long)]
    resource_group: Option<String>,
    /// Azure subscription (`azure-flexible`).
    #[arg(long)]
    subscription: Option<String>,
    /// `CloudWatch` log group (`aws-cloudwatch`); default `/aws/rds/instance/<instance>/postgresql`.
    #[arg(long)]
    log_group: Option<String>,
    /// Path of the vendor CLI executable (default: `aws`, `az`, `gcloud`, `avn` or `heroku`).
    #[arg(long)]
    bin: Option<PathBuf>,
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
    // Values go to the vendor's CLI as separate arguments: one that starts with `-` would be read
    // as an option there.
    for (flag, value) in [
        ("--instance", &args.instance),
        ("--region", &args.region),
        ("--profile", &args.profile),
        ("--project", &args.project),
        ("--resource-group", &args.resource_group),
        ("--subscription", &args.subscription),
        ("--log-group", &args.log_group),
    ] {
        if value.as_deref().is_some_and(|v| v.starts_with('-')) {
            anyhow::bail!("{flag} must not start with '-'");
        }
    }
    let need = |value: &Option<String>, flag: &str, provider: &str| -> anyhow::Result<String> {
        value
            .clone()
            .with_context(|| format!("--provider {provider} needs {flag}"))
    };
    let program = |default: &str| args.bin.clone().unwrap_or_else(|| PathBuf::from(default));
    Ok(match args.provider {
        ProviderKind::AwsRds => Box::new(AwsRds::new(
            program("aws"),
            need(
                &args.instance,
                "--instance (the DB instance identifier)",
                "aws-rds",
            )?,
            args.region.clone(),
            args.profile.clone(),
        )),
        ProviderKind::AwsCloudwatch => {
            let group = match (&args.log_group, &args.instance) {
                (Some(group), _) => group.clone(),
                (None, Some(instance)) => AwsCloudWatch::instance_group(instance),
                (None, None) => {
                    anyhow::bail!("--provider aws-cloudwatch needs --log-group or --instance")
                }
            };
            Box::new(AwsCloudWatch::new(
                AwsCli::new(program("aws"), args.region.clone(), args.profile.clone()),
                group,
            ))
        }
        ProviderKind::AzureFlexible => Box::new(AzureFlexible::new(
            program("az"),
            need(&args.resource_group, "--resource-group", "azure-flexible")?,
            need(
                &args.instance,
                "--instance (the server name)",
                "azure-flexible",
            )?,
            args.subscription.clone(),
        )),
        ProviderKind::GcpCloudsql => Box::new(GcpCloudSql::new(
            program("gcloud"),
            need(&args.project, "--project", "gcp-cloudsql")?,
            need(
                &args.instance,
                "--instance (the Cloud SQL instance name)",
                "gcp-cloudsql",
            )?,
        )),
        ProviderKind::Aiven => Box::new(Aiven::new(
            program("avn"),
            need(&args.project, "--project", "aiven")?,
            need(&args.instance, "--instance (the service name)", "aiven")?,
        )),
        ProviderKind::Heroku => Box::new(Heroku::new(
            program("heroku"),
            need(&args.instance, "--instance (the app name)", "heroku")?,
        )),
        ProviderKind::Sql => Box::new(SqlLogdir::new(conn.context("--provider sql needs --conn")?)),
    })
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

/// `export`: doctor warnings, a snapshot and the logs, into one directory; optionally the report.
#[derive(Debug, Args)]
pub struct ExportArgs {
    #[command(flatten)]
    conn: ConnArgs,
    #[command(flatten)]
    target: TargetArgs,
    /// How far back to fetch logs (see `logs fetch --since`).
    #[arg(long, default_value = "24h")]
    since: String,
    /// Directory for everything: `snapshots/`, `logs/` and the fetch state.
    #[arg(long)]
    out: PathBuf,
    /// Also write the report here (HTML) from the logs and the two newest snapshots.
    #[arg(long)]
    report: Option<PathBuf>,
    /// The server's `log_line_prefix`, for stderr logs whose prefix cannot be inferred.
    #[arg(long, requires = "report")]
    prefix: Option<String>,
}

fn vendor_of(kind: ProviderKind) -> Option<&'static str> {
    matches!(kind, ProviderKind::AwsRds | ProviderKind::AwsCloudwatch).then_some("rds")
}

/// Runs `export`.
///
/// # Errors
///
/// Fails at the first step that cannot be done, naming it; what earlier steps wrote stays.
pub fn export(args: &ExportArgs, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<Exit> {
    let spec = args.conn.spec()?;
    writeln!(err, "1/4 checking the server ({})", spec.describe())?;
    let mut client = spec.connect().context("step 1 (connect)")?;
    let facts = doctor::gather(&mut client).context("step 1 (doctor)")?;
    for check in doctor::checks(&facts, vendor_of(args.target.provider)) {
        writeln!(
            err,
            "  [{}] {}: {}",
            check.severity, check.code, check.message
        )?;
    }
    writeln!(err, "2/4 taking a snapshot")?;
    let snapshots = args.out.join("snapshots");
    let snapshot = sql::take(&mut client, &snapshots).context("step 2 (snapshot)")?;
    writeln!(out, "wrote {}", snapshot.pgss.display())?;
    drop(client);

    writeln!(err, "3/4 fetching logs")?;
    let provider = build_provider(&args.target, Some(spec))?;
    let since = parse_since(&args.since, jiff::Timestamp::now()).map_err(anyhow::Error::msg)?;
    let logs = args.out.join("logs");
    fetch_with(provider.as_ref(), &logs, since, out, err).context("step 3 (logs)")?;

    if let Some(report) = &args.report {
        writeln!(err, "4/4 building the report")?;
        build_report(&snapshots, &logs, report, args.prefix.as_deref(), out, err)
            .context("step 4 (report)")?;
    } else {
        writeln!(
            err,
            "done. Next: pgloupe report {}/* --before <older snapshot> --after <newer snapshot> --output html --out report.html",
            logs.display()
        )?;
    }
    Ok(Exit::Ok)
}

fn build_report(
    snapshots: &std::path::Path,
    logs: &std::path::Path,
    report: &std::path::Path,
    prefix: Option<&str>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<()> {
    use clap::Parser as _;
    let log_files: Vec<String> = std::fs::read_dir(logs)
        .with_context(|| format!("reading {}", logs.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .map(|p| p.display().to_string())
        .collect();
    let snaps = super::watch::snapshot_files(snapshots);
    if log_files.is_empty() && snaps.len() < 2 {
        writeln!(
            err,
            "note: no logs fetched yet and only one snapshot, so there is nothing to report; run export again later"
        )?;
        return Ok(());
    }
    let mut argv: Vec<String> = vec!["pgloupe".to_owned(), "report".to_owned()];
    argv.extend(log_files);
    if let Some(prefix) = prefix {
        argv.extend(["--prefix".to_owned(), prefix.to_owned()]);
    }
    if let [.., older, newer] = snaps.as_slice() {
        argv.extend([
            "--before".to_owned(),
            older.display().to_string(),
            "--after".to_owned(),
            newer.display().to_string(),
        ]);
        let info = |p: &PathBuf| {
            p.with_file_name(p.file_name().map_or_else(String::new, |n| {
                n.to_string_lossy().replacen("pgss_", "pgss_info_", 1)
            }))
        };
        let (bi, ai) = (info(older), info(newer));
        if bi.exists() && ai.exists() {
            argv.extend([
                "--before-info".to_owned(),
                bi.display().to_string(),
                "--after-info".to_owned(),
                ai.display().to_string(),
            ]);
        }
    } else {
        writeln!(
            err,
            "note: only one snapshot so far; run export again later to add the statistics side"
        )?;
    }
    argv.extend([
        "--output".to_owned(),
        "html".to_owned(),
        "--out".to_owned(),
        report.display().to_string(),
    ]);
    let cli = crate::cli::Cli::try_parse_from(&argv)
        .and_then(crate::cli::Cli::validate)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    crate::cli::run(&cli, out, err)?;
    writeln!(out, "report: {}", report.display())?;
    Ok(())
}

/// A span like `5m`, `24h` or `7d` as a duration.
///
/// # Errors
///
/// Fails if `text` is not a whole number followed by `s`, `m`, `h`, `d` or `w`.
pub fn parse_span(text: &str) -> Result<std::time::Duration, String> {
    let text = text.trim();
    let bad = || format!("{text:?} is not a span like 30s, 5m, 24h or 7d");
    let unit = text.chars().last().ok_or_else(bad)?;
    let n: u64 = text[..text.len() - unit.len_utf8()]
        .parse()
        .map_err(|_unparsed| bad())?;
    let per = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3_600,
        'd' => 86_400,
        'w' => 604_800,
        _ => return Err(bad()),
    };
    Ok(std::time::Duration::from_secs(n.saturating_mul(per)))
}

/// `watch`: keep a report fresh and serve it.
#[derive(Debug, Args)]
pub struct WatchArgs {
    #[command(flatten)]
    conn: ConnArgs,
    #[command(flatten)]
    target: TargetArgs,
    /// Time between updates (`30s`, `5m`, `1h`).
    #[arg(long, default_value = "5m")]
    interval: String,
    /// How far back the report looks (`24h`, `7d`).
    #[arg(long, default_value = "24h")]
    window: String,
    /// Address to serve the dashboard on (loopback only unless `--allow-remote`).
    #[arg(long, default_value = "127.0.0.1:8080")]
    listen: String,
    /// Allow `--listen` on an address other machines can reach. The page holds statement text.
    #[arg(long)]
    allow_remote: bool,
    /// Where snapshots, fetched logs and the store are kept.
    #[arg(long, default_value = "pgloupe-watch")]
    state_dir: PathBuf,
    /// Snapshots to keep (288 is 24 hours at 5 minutes).
    #[arg(long, default_value_t = 288)]
    keep_snapshots: usize,
    /// The server's `log_line_prefix`, if the logs are stderr and it cannot be inferred.
    #[arg(long)]
    prefix: Option<String>,
    /// Also write the HTML here after every update.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Run one cycle and exit instead of looping and serving.
    #[arg(long)]
    once: bool,
}

/// The shortest `watch` interval.
const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Writes the HTML to a file after each good cycle.
struct Writing<'a> {
    inner: &'a mut dyn Cycle,
    out: Option<&'a PathBuf>,
}

impl Cycle for Writing<'_> {
    fn run(&mut self) -> Result<Built, CycleError> {
        let mut built = self.inner.run()?;
        if let Some(path) = self.out {
            let tmp = path.with_extension("html.part");
            // A file that cannot be written is worth a note; the live report is still good.
            if let Err(e) =
                std::fs::write(&tmp, &built.page).and_then(|()| std::fs::rename(&tmp, path))
            {
                built.notes.push(format!("write: {}: {e}", path.display()));
            }
        }
        Ok(built)
    }
}

/// Runs `watch`.
///
/// # Errors
///
/// Fails on bad options, when the address cannot be served, or (with `--once`) when the cycle
/// fails; the loop itself keeps going through failed cycles.
pub fn watch(args: &WatchArgs, out: &mut dyn Write, err: &mut dyn Write) -> anyhow::Result<Exit> {
    let spec = args.conn.spec()?;
    let provider = build_provider(&args.target, Some(spec.clone()))?;
    if provider.experimental() {
        writeln!(
            err,
            "warning: provider {} is experimental (not yet confirmed against the real service)",
            provider.name()
        )?;
    }
    let interval = parse_span(&args.interval).map_err(anyhow::Error::msg)?;
    let window = parse_span(&args.window).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        interval >= MIN_INTERVAL,
        "--interval must be at least {} s: each cycle queries the server and the vendor API",
        MIN_INTERVAL.as_secs()
    );
    anyhow::ensure!(!window.is_zero(), "--window must be longer than zero");
    std::fs::create_dir_all(&args.state_dir)
        .with_context(|| format!("creating {}", args.state_dir.display()))?;
    let options = crate::analyze::pipeline::Options {
        prefix: args.prefix.clone(),
        ..crate::analyze::pipeline::Options::new()
    };
    let mut live = Live::new(
        spec,
        provider,
        args.state_dir.clone(),
        args.state_dir.join("store.db"),
        window,
        args.keep_snapshots,
        options,
    );
    let mut cycle = Writing {
        inner: &mut live,
        out: args.out.as_ref(),
    };
    let shared: serve::Shared = std::sync::Arc::default();
    if let Ok(mut state) = shared.write() {
        // Tells a page apart from the reports of an earlier run, whose numbers start again.
        state.run_id = format!("{:x}", jiff::Timestamp::now().as_nanosecond());
    }
    let mut log = |line: &str| drop(writeln!(std::io::stderr(), "{line}"));
    if args.once {
        let settings = Settings {
            interval,
            max_cycles: Some(1),
        };
        watch::run(settings, &mut cycle, &SystemClock, &shared, &mut log);
        let published = shared
            .read()
            .map_err(|_poisoned| anyhow::anyhow!("state lock poisoned"))?
            .clone();
        return match published.last_error {
            Some(error) if published.generation == 0 => anyhow::bail!("{error}"),
            _ => {
                writeln!(
                    out,
                    "one cycle done{}",
                    args.out
                        .as_ref()
                        .map_or_else(String::new, |p| format!("; report: {}", p.display()))
                )?;
                Ok(Exit::Ok)
            }
        };
    }
    let addr = serve::parse_listen(&args.listen, args.allow_remote)?;
    if args.allow_remote {
        writeln!(
            err,
            "warning: serving on {addr}: anyone who can reach it can read the statements in the report"
        )?;
    }
    let server = serve::bind(addr)?;
    writeln!(
        out,
        "serving http://{}/ (updates every {})",
        server.addr, args.interval
    )?;
    out.flush()?;
    server.spawn(&shared);
    watch::run(
        Settings {
            interval,
            max_cycles: None,
        },
        &mut cycle,
        &SystemClock,
        &shared,
        &mut log,
    );
    Ok(Exit::Ok)
}
