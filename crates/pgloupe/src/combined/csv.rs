//! The statement table as CSV: one row per statement, log and statistics side by side.

use super::CombinedReport;

/// Column names, in order.
const HEADER: [&str; 33] = [
    "fingerprint",
    "link",
    "kind",
    "query_id",
    "query",
    "log_calls",
    "log_total_ms",
    "log_all_ms",
    "log_mean_ms",
    "log_p50_ms",
    "log_p95_ms",
    "log_p99_ms",
    "log_max_ms",
    "parse_ms",
    "bind_ms",
    "fetch_ms",
    "errors",
    "lock_waits",
    "lock_wait_ms",
    "temp_files",
    "temp_bytes",
    "first_seen",
    "last_seen",
    "stats_calls",
    "stats_total_ms",
    "stats_mean_ms",
    "stats_rows",
    "stats_share_pct",
    "shared_blks_hit",
    "shared_blks_read",
    "wal_bytes",
    "coverage",
    "flags",
];

fn num(value: f64) -> String {
    format!("{value:.3}")
}

fn opt(value: Option<f64>) -> String {
    value.map(num).unwrap_or_default()
}

/// One CSV document with a header row, for every statement the report lists.
///
/// # Errors
///
/// Fails only if the CSV writer does, which an in-memory buffer does not.
pub fn csv(report: &CombinedReport) -> Result<String, csv::Error> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(HEADER)?;
    for row in &report.statements {
        let log = row.log.as_ref();
        let stats = row.stats.as_ref();
        let phase = |pick: fn(&crate::analyze::report::PhasesRow) -> f64| {
            log.and_then(|l| l.phases.as_ref())
                .map(pick)
                .map(num)
                .unwrap_or_default()
        };
        let record: Vec<String> = vec![
            row.fingerprint.clone(),
            row.link.to_owned(),
            log.map(|l| l.kind.to_owned()).unwrap_or_default(),
            log.and_then(|l| l.query_id.clone()).unwrap_or_default(),
            row.text.clone(),
            log.map(|l| l.calls.to_string()).unwrap_or_default(),
            log.map(|l| num(l.total_ms)).unwrap_or_default(),
            log.map(|l| num(l.all_ms)).unwrap_or_default(),
            opt(log.and_then(|l| l.mean_ms)),
            opt(log.and_then(|l| l.p50_ms)),
            opt(log.and_then(|l| l.p95_ms)),
            opt(log.and_then(|l| l.p99_ms)),
            opt(log.and_then(|l| l.max_ms)),
            phase(|p| p.parse.total_ms),
            phase(|p| p.bind.total_ms),
            phase(|p| p.fetch.total_ms),
            log.map(|l| l.errors.to_string()).unwrap_or_default(),
            log.map(|l| l.lock_waits.to_string()).unwrap_or_default(),
            log.map(|l| num(l.lock_wait_ms)).unwrap_or_default(),
            log.map(|l| l.temp_files.to_string()).unwrap_or_default(),
            log.map(|l| l.temp_bytes.to_string()).unwrap_or_default(),
            log.and_then(|l| l.first_seen.clone()).unwrap_or_default(),
            log.and_then(|l| l.last_seen.clone()).unwrap_or_default(),
            stats.map(|s| s.calls.to_string()).unwrap_or_default(),
            stats.map(|s| num(s.total_exec_ms)).unwrap_or_default(),
            opt(stats.and_then(|s| s.mean_ms)),
            stats.map(|s| s.rows.to_string()).unwrap_or_default(),
            stats.map(|s| num(s.share_pct)).unwrap_or_default(),
            stats
                .map(|s| s.shared_blks_hit.to_string())
                .unwrap_or_default(),
            stats
                .map(|s| s.shared_blks_read.to_string())
                .unwrap_or_default(),
            stats.map(|s| s.wal_bytes.to_string()).unwrap_or_default(),
            opt(row.coverage),
            stats.map(|s| s.flags.join(" ")).unwrap_or_default(),
        ];
        writer.write_record(&record)?;
    }
    let bytes = writer
        .into_inner()
        .map_err(csv::IntoInnerError::into_error)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
