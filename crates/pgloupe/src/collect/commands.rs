//! The command-line side of collection: arguments and handlers for `snap` and `doctor`.

use std::{io::Write, path::PathBuf};

use anyhow::Context as _;
use clap::{Args, ValueEnum};

use super::{
    conn::ConnSpec,
    doctor::{self, Check},
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
