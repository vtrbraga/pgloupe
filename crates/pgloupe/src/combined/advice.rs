//! Settings that limit what the report can show, and what to change.
//!
//! Two sources, never guessed: rules over the data itself (what is missing from a log, what a
//! `pg_stat_statements` pair shows), and, when a settings file is given, the doctor's checks over
//! the real values.

use std::{collections::BTreeMap, fs, path::Path};

use serde::Serialize;

use crate::{
    analyze::report::Report,
    collect::doctor::{Facts, checks},
    stats::delta::Delta,
};

/// One setting worth changing, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Advice {
    /// Stable identifier, for tools and tests. The same code as the doctor's where both apply.
    pub code: &'static str,
    /// `warn` (a section is empty or wrong) or `info` (it is poorer).
    pub severity: &'static str,
    /// What is lost and exactly what to change.
    pub message: String,
    /// `data` (inferred from the logs and statistics) or `settings` (read from the server).
    pub source: &'static str,
}

/// What the rules look at.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    pub logs: Option<&'a Report>,
    pub delta: Option<&'a Delta>,
    /// `pg_settings` by name, from `--settings` or the file `snap` wrote.
    pub settings: Option<&'a BTreeMap<String, String>>,
    /// `rds` when the data comes from Amazon RDS or Aurora.
    pub vendor: Option<&'a str>,
}

/// Reads a settings file: CSV with at least the columns `name` and `setting`, as `pg_settings`
/// exports them.
///
/// # Errors
///
/// Fails when the file cannot be read or has no `name` and `setting` columns.
pub fn read_settings(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = csv::Reader::from_reader(text.as_slice());
    let headers = reader
        .headers()
        .map_err(|e| format!("{}: {e}", path.display()))?
        .clone();
    let column = |name: &str| headers.iter().position(|h| h == name);
    let (Some(name), Some(setting)) = (column("name"), column("setting")) else {
        return Err(format!(
            "{}: expected a CSV with `name` and `setting` columns, as pg_settings exports",
            path.display()
        ));
    };
    let mut settings = BTreeMap::new();
    for record in reader.records() {
        let record = record.map_err(|e| format!("{}: {e}", path.display()))?;
        if let (Some(n), Some(v)) = (record.get(name), record.get(setting)) {
            settings.insert(n.to_owned(), v.to_owned());
        }
    }
    Ok(settings)
}

/// `rds` when the settings carry RDS parameters or the log files are named the way RDS names them.
#[must_use]
pub fn detect_vendor(input: &Input<'_>) -> Option<&'static str> {
    let rds_settings = input
        .settings
        .is_some_and(|s| s.keys().any(|k| k.starts_with("rds.")));
    let rds_files = input.logs.is_some_and(|logs| {
        logs.files.iter().any(|f| {
            let name = f.path.rsplit(['/', '\\']).next().unwrap_or_default();
            name.contains("postgresql.log.")
                && name.rsplit('.').next().is_some_and(|t| {
                    let digits: Vec<&str> = t.split('-').collect();
                    digits.len() == 4
                        && digits.iter().all(|d| d.bytes().all(|b| b.is_ascii_digit()))
                })
        })
    });
    (rds_settings || rds_files).then_some("rds")
}

/// The advice for `input`, warnings first. A rule over the settings replaces the data rule with
/// the same code, because it knows rather than infers.
#[must_use]
pub fn advise(input: &Input<'_>) -> Vec<Advice> {
    let vendor = input.vendor.or_else(|| detect_vendor(input));
    let mut found = from_data(input, vendor);
    if let Some(settings) = input.settings {
        let facts = Facts {
            version: settings
                .get("server_version_num")
                .and_then(|v| v.parse().ok())
                .or_else(|| {
                    input
                        .delta
                        .map(|d| i32::try_from(d.server_version_num).unwrap_or(0))
                })
                .unwrap_or(0),
            settings: settings.clone(),
            extension_created: input.delta.is_some()
                || settings.contains_key("pg_stat_statements.max"),
            reads_all_stats: true,
            dealloc: None,
            role: String::new(),
        };
        for check in checks(&facts, vendor) {
            found.retain(|a| a.code != check.code);
            found.push(Advice {
                code: check.code,
                severity: check.severity,
                message: check.message,
                source: "settings",
            });
        }
    }
    found.sort_by_key(|a| (a.severity != "warn", a.code));
    found
}

/// Where a setting lives, for the sentence that tells the reader to change it.
struct Place {
    rds: bool,
    home: &'static str,
}

fn push(found: &mut Vec<Advice>, code: &'static str, severity: &'static str, message: String) {
    found.push(Advice {
        code,
        severity,
        message,
        source: "data",
    });
}

fn from_data(input: &Input<'_>, vendor: Option<&str>) -> Vec<Advice> {
    let rds = vendor == Some("rds");
    let place = Place {
        rds,
        home: if rds {
            "the DB parameter group"
        } else {
            "postgresql.conf"
        },
    };
    let mut found = Vec::new();
    if let Some(logs) = input.logs {
        log_rules(logs, &place, &mut found);
    }
    if let Some(delta) = input.delta {
        statistics_rules(delta, &place, &mut found);
    }
    if let Some(logs) = input.logs
        && input.settings.is_none()
    {
        prefix_rule(logs, &place, &mut found);
    }
    found
}

fn log_rules(logs: &Report, place: &Place, found: &mut Vec<Advice>) {
    if !logs.files.is_empty() && logs.files.iter().all(|f| f.format == "pgbouncer") {
        return;
    }
    let home = place.home;
    let events = &logs.overview.events;
    let count = |kind: &str| events.get(kind).copied().unwrap_or(0);
    if logs.overview.records > 0 && count("statement") + count("execute") == 0 {
        push(
            found,
            "no_statement_timings",
            "warn",
            format!(
                "the log holds no statements with a duration, so slow statements, percentiles, the slowest executions and the log side of the join are empty. Set log_min_duration_statement in {home} (250 logs statements over 250 ms; 0 logs everything and is heavy on busy servers). It takes effect without a restart."
            ),
        );
    }
    if logs.diagnostics.truncated > 0 {
        push(
            found,
            "truncated_entries",
            "info",
            format!(
                "{} log entries were cut short by the server, so their statements and contexts are incomplete. Keep very long statements and messages out of the log, or read csvlog or jsonlog instead of stderr.",
                logs.diagnostics.truncated
            ),
        );
    }
    let (lines, records) = (logs.diagnostics.lines, logs.diagnostics.records.max(1));
    if lines >= records.saturating_mul(5) && lines > 10_000 {
        push(
            found,
            "multiline_logs",
            "info",
            format!(
                "{lines} lines make {records} entries: most of the stderr log is continuation lines. pgloupe assembles them, but csvlog or jsonlog (add to log_destination in {home}; they can be enabled alongside stderr) parse without any ambiguity."
            ),
        );
    }
    if let Some(top) = logs.messages.first()
        && top.count >= 1_000
        && top.count.saturating_mul(2) >= logs.overview.records
    {
        push(
            found,
            "log_chatter",
            "info",
            format!(
                "{} of {} log records ({}%) are one message, “{}”. If application code writes it (RAISE LOG, for instance), lowering its level to DEBUG or NOTICE removes most of the log and leaves room for what matters.",
                top.count,
                logs.overview.records,
                top.count * 100 / logs.overview.records.max(1),
                top.shape
            ),
        );
    }
    for checkpoint in &logs.checkpoints {
        let forced = checkpoint.reasons.get("wal").copied().unwrap_or(0);
        if checkpoint.started > 0 && forced * 20 >= checkpoint.started {
            push(
                found,
                "forced_checkpoints",
                "info",
                format!(
                    "{forced} of {} {}s were started because the WAL filled up, not by the timer: checkpoints bunch writes together and slow statements down. Raise max_wal_size in {home} (it takes effect without a restart).",
                    checkpoint.started, checkpoint.kind
                ),
            );
        }
    }
}

fn statistics_rules(delta: &Delta, place: &Place, found: &mut Vec<Advice>) {
    let c = &delta.counts;
    let restart = if place.rds {
        "it needs a reboot"
    } else {
        "it needs a restart"
    };
    if let Some(evicted) = delta.evictions.filter(|e| *e > 0) {
        push(
            found,
            "pgss_evictions",
            "warn",
            format!(
                "{evicted} entries were evicted from pg_stat_statements during the window, so the least used statements are incomplete. Raise pg_stat_statements.max in {} ({restart}) to comfortably above the {} distinct statements seen.",
                place.home, c.entries
            ),
        );
    }
    if c.entries > 100 && (c.new + c.gone) >= c.entries {
        push(
            found,
            "pgss_churn",
            "info",
            format!(
                "{} of {} statements are new since the first snapshot and {} from it are gone: the statement list turns over almost completely, so a long window shows little. Take snapshots closer together (`pgloupe watch` does it every few minutes) and look for statements that are not parameterized (each distinct literal is a new entry).",
                c.new, c.entries, c.gone
            ),
        );
    }
}

fn prefix_rule(logs: &Report, place: &Place, found: &mut Vec<Advice>) {
    let prefix = logs
        .files
        .iter()
        .find_map(|f| f.prefix.clone())
        .unwrap_or_default();
    let missing: Vec<&str> = [
        ("%a", "the application"),
        ("%e", "the SQLSTATE"),
        ("%Q", "the query id"),
    ]
    .iter()
    .filter(|(escape, _)| !prefix.is_empty() && !prefix.contains(escape))
    .map(|(_, what)| *what)
    .collect();
    if missing.is_empty() {
        return;
    }
    let fix = if place.rds {
        "RDS fixes log_line_prefix, so these stay empty there."
    } else {
        "A prefix such as '%m [%p] %q%u@%d/%a %e ' carries them."
    };
    push(
        found,
        "line_prefix",
        "info",
        format!(
            "the log_line_prefix ({prefix:?}) has no {}: those columns will be empty in the report. {fix}",
            missing.join(", ")
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::{
        acc::Accumulators,
        pipeline::Output,
        report::{ReportOptions, SortBy},
    };

    fn empty_report() -> Report {
        Report::build(
            &Output {
                acc: Accumulators::new(10, 60),
                files: Vec::new(),
            },
            &ReportOptions {
                by: SortBy::Total,
                top: 1,
            },
        )
    }

    fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_log_with_records_and_no_statements_asks_for_durations() {
        let mut report = empty_report();
        report.overview.records = 500;
        let input = Input {
            logs: Some(&report),
            delta: None,
            settings: None,
            vendor: None,
        };
        let advice = advise(&input);
        assert_eq!(advice[0].code, "no_statement_timings");
        assert_eq!(advice[0].severity, "warn");
        assert!(advice[0].message.contains("postgresql.conf"));
        let rds = advise(&Input {
            vendor: Some("rds"),
            ..input
        });
        assert!(rds[0].message.contains("parameter group"));
    }

    #[test]
    fn settings_replace_what_the_data_only_suggested() {
        let mut report = empty_report();
        report.overview.records = 500;
        let values = settings(&[
            ("log_min_duration_statement", "-1"),
            ("log_lock_waits", "off"),
            ("log_temp_files", "-1"),
            ("log_checkpoints", "on"),
            ("shared_preload_libraries", "pg_stat_statements"),
            ("server_version_num", "160013"),
            ("rds.extensions", "x"),
        ]);
        let advice = advise(&Input {
            logs: Some(&report),
            delta: None,
            settings: Some(&values),
            vendor: None,
        });
        let codes: Vec<&str> = advice.iter().map(|a| a.code).collect();
        assert_eq!(
            codes
                .iter()
                .filter(|c| **c == "no_statement_timings")
                .count(),
            1
        );
        let timings = advice
            .iter()
            .find(|a| a.code == "no_statement_timings")
            .unwrap();
        assert_eq!(timings.source, "settings");
        assert!(
            timings.message.contains("parameter group"),
            "rds is detected from rds.* settings"
        );
        assert!(codes.contains(&"no_lock_waits") && codes.contains(&"no_temp_files"));
        assert!(!codes.contains(&"no_checkpoints"));
    }

    #[test]
    fn a_settings_file_is_read_by_column_name() {
        let dir = std::env::temp_dir().join(format!("pgloupe-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let good = dir.join("s.csv");
        fs::write(&good, "name,setting,unit,source,pending_restart\nautovacuum,on,,default,f\nlog_connections,off,,configuration file,f\n").unwrap();
        let read = read_settings(&good).unwrap();
        assert_eq!(read["log_connections"], "off");
        let bad = dir.join("bad.csv");
        fs::write(&bad, "a,b\n1,2\n").unwrap();
        assert!(read_settings(&bad).unwrap_err().contains("name"));
        read_settings(&dir.join("missing.csv")).unwrap_err();
    }

    #[test]
    fn log_rules_name_truncation_multiline_chatter_and_forced_checkpoints() {
        let mut report = empty_report();
        report.overview.records = 10_000;
        report.overview.events.insert("statement".to_owned(), 5);
        report.diagnostics.truncated = 3;
        report.diagnostics.lines = 80_000;
        report.diagnostics.records = 10_000;
        report.messages.push(crate::analyze::report::MessageRow {
            shape: "sync step ? done".to_owned(),
            count: 6_000,
            first_seen: "2026-10-14T10:00:00Z".to_owned(),
            example: "sync step 1 done".to_owned(),
        });
        report
            .checkpoints
            .push(crate::analyze::report::CheckpointRow {
                kind: "checkpoint",
                started: 40,
                completed: 40,
                reasons: BTreeMap::from([("wal".to_owned(), 4_u64), ("time".to_owned(), 36)]),
                buffers: 0,
                write_ms: crate::analyze::report::Quantiles {
                    count: 0,
                    p50: None,
                    p95: None,
                    max: None,
                },
                sync_ms: crate::analyze::report::Quantiles {
                    count: 0,
                    p50: None,
                    p95: None,
                    max: None,
                },
                total_ms: crate::analyze::report::Quantiles {
                    count: 0,
                    p50: None,
                    p95: None,
                    max: None,
                },
                distance_kb: None,
            });
        let advice = advise(&Input {
            logs: Some(&report),
            delta: None,
            settings: None,
            vendor: None,
        });
        let codes: Vec<&str> = advice.iter().map(|a| a.code).collect();
        for code in [
            "truncated_entries",
            "multiline_logs",
            "log_chatter",
            "forced_checkpoints",
        ] {
            assert!(codes.contains(&code), "{code} in {codes:?}");
        }
        // Statements were logged, so durations are not asked for.
        assert!(!codes.contains(&"no_statement_timings"));
        let chatter = advice.iter().find(|a| a.code == "log_chatter").unwrap();
        assert!(chatter.message.contains("60%"), "{}", chatter.message);
    }
}
