//! What the server and the connecting role lack for a useful report, and how to fix it.

use std::collections::BTreeMap;

use postgres::Client;
use serde::Serialize;

use super::CollectError;

/// One thing worth fixing (or knowing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// Stable identifier, for tools and tests.
    pub code: &'static str,
    /// `warn` (a report section will be empty or wrong) or `info` (it will be poorer).
    pub severity: &'static str,
    /// What is wrong and exactly what to change.
    pub message: String,
}

/// What `doctor` learned about the server and the role.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// `server_version_num`.
    pub version: i32,
    /// `pg_settings.setting` by name.
    pub settings: BTreeMap<String, String>,
    /// `pg_stat_statements` is created in this database.
    pub extension_created: bool,
    /// The role may read other roles' query text.
    pub reads_all_stats: bool,
    /// Entries evicted so far (`pg_stat_statements_info.dealloc`), on 14+.
    pub dealloc: Option<i64>,
    /// The connected role.
    pub role: String,
}

impl Facts {
    fn setting(&self, name: &str) -> Option<&str> {
        self.settings.get(name).map(String::as_str)
    }

    fn is_on(&self, name: &str) -> bool {
        let Some(value) = self.setting(name) else {
            return false;
        };
        if matches!(value, "on" | "true" | "all" | "1" | "yes") {
            return true;
        }
        // PostgreSQL 18 turns `log_connections` into a list of stages.
        name == "log_connections"
            && value.split(',').map(str::trim).any(|stage| {
                matches!(
                    stage,
                    "receipt" | "authentication" | "authorization" | "setup_durations"
                )
            })
    }

    fn number(&self, name: &str) -> Option<i64> {
        self.setting(name).and_then(|v| v.trim().parse().ok())
    }
}

fn check(code: &'static str, severity: &'static str, message: impl Into<String>) -> Check {
    Check {
        code,
        severity,
        message: message.into(),
    }
}

/// The checks that apply to `facts`. `vendor` (for example `rds`) only changes where the advice
/// says a setting lives.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one flat list of independent checks reads better than split helpers"
)]
pub fn checks(facts: &Facts, vendor: Option<&str>) -> Vec<Check> {
    let rds = vendor == Some("rds");
    let setting_home = if rds {
        "the DB parameter group"
    } else {
        "postgresql.conf"
    };
    let mut found = Vec::new();

    // The setting is hidden from roles without `pg_read_all_settings`; absent means unknown.
    let preload = facts.setting("shared_preload_libraries");
    if preload.is_some_and(|p| !p.contains("pg_stat_statements")) {
        found.push(check(
            "pgss_not_preloaded",
            "warn",
            format!(
                "pg_stat_statements is not in shared_preload_libraries: no statistics can be collected. Add it in {setting_home} and restart the server{}.",
                if rds { " (a reboot is needed)" } else { "" }
            ),
        ));
    } else if !facts.extension_created {
        let unknown = if preload.is_none() {
            " (shared_preload_libraries is hidden from this role, so also check it is listed there; granting pg_monitor shows it)"
        } else {
            ""
        };
        found.push(check(
            "pgss_not_created",
            "warn",
            format!("the pg_stat_statements extension is not created in this database: run CREATE EXTENSION pg_stat_statements as a role that may{unknown}."),
        ));
    }
    if facts.extension_created && !facts.reads_all_stats {
        found.push(check(
            "no_query_text",
            "warn",
            format!(
                "role {} cannot read other roles' query text (pg_stat_statements shows <insufficient privilege>): GRANT pg_read_all_stats TO {};",
                facts.role, facts.role
            ),
        ));
    }

    match facts.number("log_min_duration_statement") {
        Some(-1) => found.push(check(
            "no_statement_timings",
            "warn",
            format!("log_min_duration_statement = -1: the log carries no statement durations, so slow statements, percentiles and the slowest executions will be empty. Set it in {setting_home} (250 logs statements over 250 ms; 0 logs everything and is heavy on busy servers)."),
        )),
        Some(ms) if ms >= 5_000 => found.push(check(
            "few_statement_timings",
            "info",
            format!("log_min_duration_statement = {ms} ms: only very slow statements are logged, so coverage against pg_stat_statements will be low. Consider 250."),
        )),
        _ => {}
    }
    if !facts.is_on("log_lock_waits") {
        found.push(check(
            "no_lock_waits",
            "warn",
            format!("log_lock_waits is off: lock waits and who waited for whom will not appear. Set it to on in {setting_home} (deadlock_timeout, default 1 s, is the wait before it logs)."),
        ));
    }
    if facts.number("log_temp_files") == Some(-1) {
        found.push(check(
            "no_temp_files",
            "info",
            format!("log_temp_files = -1: temp-file spills will not appear. Set it to 0 (all) or 10240 (10 MB and over) in {setting_home}."),
        ));
    }
    if !facts.is_on("log_checkpoints") {
        found.push(check(
            "no_checkpoints",
            "info",
            format!("log_checkpoints is off: the checkpoint section will be empty. Set it to on in {setting_home}."),
        ));
    }
    if facts.number("log_autovacuum_min_duration") == Some(-1) {
        found.push(check(
            "no_autovacuum",
            "info",
            format!("log_autovacuum_min_duration = -1: autovacuum runs will not appear. Set it to 1000 (ms) or 0 in {setting_home}."),
        ));
    }
    if !facts.is_on("log_connections") || !facts.is_on("log_disconnections") {
        found.push(check(
            "no_sessions",
            "info",
            format!("log_connections / log_disconnections are not both on: connections, session durations and the logins breakdown will be poor. Set both to on in {setting_home}."),
        ));
    }

    let destination = facts.setting("log_destination").unwrap_or("stderr");
    if destination.split(',').any(|d| d.trim() == "stderr") {
        let prefix = facts.setting("log_line_prefix").unwrap_or_default();
        let has = |escapes: &[&str]| escapes.iter().any(|e| prefix.contains(e));
        let mut missing = Vec::new();
        if !has(&["%m", "%t", "%n"]) {
            missing.push("a timestamp (%m or %t)");
        }
        if !has(&["%p", "%c"]) {
            missing.push("the process id (%p)");
        }
        if !has(&["%u"]) {
            missing.push("the user (%u)");
        }
        if !has(&["%d"]) {
            missing.push("the database (%d)");
        }
        if !has(&["%a"]) {
            missing.push("the application (%a)");
        }
        if !has(&["%e"]) {
            missing.push("the SQLSTATE (%e)");
        }
        if !missing.is_empty() {
            let severity = if missing
                .iter()
                .any(|m| m.starts_with("a timestamp") || m.starts_with("the process"))
            {
                "warn"
            } else {
                "info"
            };
            if rds {
                found.push(check(
                    "line_prefix",
                    "info",
                    format!(
                        "RDS fixes log_line_prefix ({prefix:?}), which lacks {}: those columns will be empty in the report.",
                        missing.join(", ")
                    ),
                ));
            } else {
                found.push(check(
                "line_prefix",
                severity,
                format!(
                    "log_line_prefix {prefix:?} lacks {}: the report cannot attribute records to them. A good prefix is '%m [%p] %q%u@%d/%a %e ' (set it in {setting_home}).",
                    missing.join(", ")
                ),
            ));
            }
        }
    }

    if facts.setting("compute_query_id") == Some("off") && facts.version >= 140_000 {
        found.push(check(
            "no_query_id",
            "info",
            "compute_query_id is off: logged statements and pg_stat_statements entries cannot be matched by query id (pgloupe matches by fingerprint, so the join still works). Set it to auto or on.",
        ));
    }
    if !facts.is_on("track_io_timing") {
        found.push(check(
            "no_io_timing",
            "info",
            format!("track_io_timing is off: pg_stat_statements has no read/write times. Set it to on in {setting_home} (small overhead on most systems)."),
        ));
    }
    if let Some(dealloc) = facts.dealloc.filter(|d| *d > 0) {
        let max = facts.setting("pg_stat_statements.max").unwrap_or("5000");
        found.push(check(
            "pgss_evictions",
            "warn",
            format!("pg_stat_statements has evicted {dealloc} entries (pg_stat_statements.max = {max}): rarely run statements are missing from the statistics. Raise pg_stat_statements.max (needs a restart)."),
        ));
    }
    found
}

fn query_error(step: &'static str, error: &postgres::Error) -> CollectError {
    CollectError::Query {
        step,
        message: error
            .as_db_error()
            .map_or_else(|| error.to_string(), |d| d.message().to_owned()),
    }
}

/// Reads the facts the checks need.
///
/// # Errors
///
/// Fails if a catalog query fails.
pub fn gather(client: &mut Client) -> Result<Facts, CollectError> {
    let mut facts = Facts::default();
    let rows = client
        .query("SELECT name, setting FROM pg_settings", &[])
        .map_err(|e| query_error("reading pg_settings", &e))?;
    for row in rows {
        facts.settings.insert(row.get(0), row.get(1));
    }
    facts.version = facts
        .setting("server_version_num")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let row = client
        .query_one(
            "SELECT current_user::text, \
             EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements'), \
             pg_has_role(current_user, 'pg_read_all_stats', 'member') OR \
               (SELECT rolsuper FROM pg_roles WHERE rolname = current_user)",
            &[],
        )
        .map_err(|e| query_error("reading the role and extension", &e))?;
    facts.role = row.get(0);
    facts.extension_created = row.get(1);
    facts.reads_all_stats = row.get(2);
    if facts.extension_created && facts.version >= 140_000 {
        facts.dealloc = client
            .query_one("SELECT dealloc FROM pg_stat_statements_info", &[])
            .ok()
            .map(|r| r.get(0));
    }
    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that has everything on.
    fn healthy() -> Facts {
        let settings = [
            ("shared_preload_libraries", "pg_stat_statements"),
            ("log_min_duration_statement", "250"),
            ("log_lock_waits", "on"),
            ("log_temp_files", "0"),
            ("log_checkpoints", "on"),
            ("log_autovacuum_min_duration", "1000"),
            ("log_connections", "on"),
            ("log_disconnections", "on"),
            ("log_destination", "stderr"),
            ("log_line_prefix", "%m [%p] %q%u@%d/%a %e "),
            ("compute_query_id", "auto"),
            ("track_io_timing", "on"),
            ("pg_stat_statements.max", "5000"),
        ];
        Facts {
            version: 170_000,
            settings: settings
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            extension_created: true,
            reads_all_stats: true,
            dealloc: Some(0),
            role: "pgloupe".to_owned(),
        }
    }

    fn codes(found: &[Check]) -> Vec<&'static str> {
        found.iter().map(|c| c.code).collect()
    }

    fn with(setting: &str, value: &str) -> Facts {
        let mut facts = healthy();
        facts.settings.insert(setting.to_owned(), value.to_owned());
        facts
    }

    #[test]
    fn a_healthy_server_has_nothing_to_fix() {
        assert_eq!(codes(&checks(&healthy(), None)), Vec::<&str>::new());
    }

    #[test]
    fn each_missing_piece_is_named_with_its_fix() {
        let cases: [(&str, &str, &str, &str); 8] = [
            (
                "shared_preload_libraries",
                "",
                "pgss_not_preloaded",
                "shared_preload_libraries",
            ),
            (
                "log_min_duration_statement",
                "-1",
                "no_statement_timings",
                "log_min_duration_statement",
            ),
            ("log_lock_waits", "off", "no_lock_waits", "log_lock_waits"),
            ("log_temp_files", "-1", "no_temp_files", "log_temp_files"),
            (
                "log_checkpoints",
                "off",
                "no_checkpoints",
                "log_checkpoints",
            ),
            (
                "log_autovacuum_min_duration",
                "-1",
                "no_autovacuum",
                "log_autovacuum_min_duration",
            ),
            ("log_connections", "off", "no_sessions", "log_connections"),
            ("track_io_timing", "off", "no_io_timing", "track_io_timing"),
        ];
        for (setting, value, code, mentions) in cases {
            let found = checks(&with(setting, value), None);
            let one = found
                .iter()
                .find(|c| c.code == code)
                .unwrap_or_else(|| panic!("{code}: {found:?}"));
            assert!(one.message.contains(mentions), "{}", one.message);
        }
    }

    #[test]
    fn a_role_without_text_privileges_and_missing_extension_are_warned() {
        let mut facts = healthy();
        facts.reads_all_stats = false;
        let found = checks(&facts, None);
        let hidden = found.iter().find(|c| c.code == "no_query_text").unwrap();
        assert_eq!(hidden.severity, "warn");
        assert!(
            hidden
                .message
                .contains("GRANT pg_read_all_stats TO pgloupe")
        );
        facts.extension_created = false;
        assert!(codes(&checks(&facts, None)).contains(&"pgss_not_created"));
    }

    #[test]
    fn a_hidden_preload_setting_is_unknown_not_missing() {
        let mut facts = healthy();
        facts.settings.remove("shared_preload_libraries");
        assert!(!codes(&checks(&facts, None)).contains(&"pgss_not_preloaded"));
        facts.extension_created = false;
        let found = checks(&facts, None);
        let note = found.iter().find(|c| c.code == "pgss_not_created").unwrap();
        assert!(note.message.contains("pg_monitor"), "{}", note.message);
    }

    #[test]
    fn postgres_18_connection_stages_count_as_logging_connections() {
        let found = checks(&with("log_connections", "receipt"), None);
        assert!(!codes(&found).contains(&"no_sessions"));
        let found = checks(&with("log_connections", ""), None);
        assert!(codes(&found).contains(&"no_sessions"));
    }

    #[test]
    fn on_rds_the_line_prefix_is_reported_as_fixed_not_as_something_to_change() {
        let found = checks(&with("log_line_prefix", "%t:%r:%u@%d:[%p]:"), Some("rds"));
        let prefix = found.iter().find(|c| c.code == "line_prefix").unwrap();
        assert_eq!(prefix.severity, "info");
        assert!(prefix.message.contains("RDS fixes"), "{}", prefix.message);
        assert!(
            !prefix.message.contains("parameter group"),
            "{}",
            prefix.message
        );
    }

    #[test]
    fn a_thin_log_line_prefix_lists_what_it_lacks() {
        let found = checks(&with("log_line_prefix", "%t "), None);
        let prefix = found.iter().find(|c| c.code == "line_prefix").unwrap();
        assert_eq!(prefix.severity, "warn");
        for lacks in ["process id", "user", "database", "application", "SQLSTATE"] {
            assert!(prefix.message.contains(lacks), "{}", prefix.message);
        }
        // Only attributes missing: informational.
        let found = checks(&with("log_line_prefix", "%m [%p] "), None);
        assert_eq!(
            found
                .iter()
                .find(|c| c.code == "line_prefix")
                .unwrap()
                .severity,
            "info"
        );
        // No stderr destination: nothing to say about a prefix.
        let mut facts = with("log_line_prefix", "x");
        facts
            .settings
            .insert("log_destination".to_owned(), "csvlog".to_owned());
        assert!(!codes(&checks(&facts, None)).contains(&"line_prefix"));
    }

    #[test]
    fn evictions_and_slow_thresholds_are_noticed() {
        let mut facts = healthy();
        facts.dealloc = Some(12);
        assert!(codes(&checks(&facts, None)).contains(&"pgss_evictions"));
        assert!(
            codes(&checks(&with("log_min_duration_statement", "10000"), None))
                .contains(&"few_statement_timings")
        );
    }

    #[test]
    fn advice_says_where_the_setting_lives_on_rds() {
        let found = checks(&with("log_lock_waits", "off"), Some("rds"));
        assert!(
            found[0].message.contains("parameter group"),
            "{}",
            found[0].message
        );
        let found = checks(&with("log_lock_waits", "off"), None);
        assert!(found[0].message.contains("postgresql.conf"));
    }
}
